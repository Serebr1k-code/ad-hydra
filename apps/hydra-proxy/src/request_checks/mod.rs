//! Optional, networkless checks on requests to locally defended services.
use anyhow::{Context, ensure};
use hydra_domain::Service;
use serde::{Deserialize, Serialize};
mod settings;
pub(crate) use settings::{Policy, Settings, StaleRevision};
use std::{
    collections::HashSet,
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, RwLock},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const MAX_REQUEST: usize = 1024 * 1024;
const RESTART_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    id: String,
    services: Vec<String>,
    script: PathBuf,
    timeout_ms: u64,
    on_error: Fallback,
    #[serde(default)]
    runtime: Runtime,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Fallback {
    Allow,
    Block,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Runtime {
    #[default]
    Python,
    Wasm,
}

#[derive(Default)]
pub(crate) struct Checks {
    current: RwLock<settings::Applied>,
    path: Option<PathBuf>,
    update: Mutex<()>,
    checkers: Vec<Checker>,
    status: Mutex<Status>,
}

#[derive(Default)]
struct Status {
    errors: u64,
    last_error: String,
    last_logged: Option<Instant>,
}

struct Checker {
    policy: Config,
    state: PathBuf,
    worker: tokio::sync::Mutex<WorkerSlot>,
}

#[derive(Default)]
struct WorkerSlot {
    process: Option<Worker>,
    retry_after: Option<Instant>,
}

impl Checks {
    pub(crate) fn open(root: &Path, configs: Vec<Config>) -> anyhow::Result<Self> {
        ensure!(configs.len() <= 4, "at most four request checkers");
        if configs.is_empty() {
            return Ok(Self::default());
        }
        private_directory(root)?;
        let root = root.canonicalize()?;
        let mut ids = HashSet::new();
        let mut checks = Self::default();
        for mut policy in configs {
            ensure!(
                !policy.id.is_empty()
                    && policy.id.len() <= 32
                    && policy
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    && ids.insert(policy.id.clone()),
                "invalid or duplicate request checker ID"
            );
            ensure!(
                policy.script.is_absolute(),
                "request checker script must use an absolute path"
            );
            let meta = policy
                .script
                .symlink_metadata()
                .context("inspect request checker script")?;
            ensure!(
                meta.is_file() && meta.len() <= 256 * 1024 && meta.mode() & 0o022 == 0,
                "request checker script must be a regular file under 256 KiB without group or world write access"
            );
            let script = policy.script.canonicalize()?;
            ensure!(
                !script.starts_with(&root),
                "request checker script must be outside writable checker state"
            );
            policy.script = script;
            let state = root.join(&policy.id);
            private_directory(&state)?;
            checks.checkers.push(Checker {
                policy,
                state,
                worker: Default::default(),
            });
        }
        checks.path = Some(root.join("settings.json"));
        checks.load_settings()?;
        Ok(checks)
    }

    pub(crate) async fn allow(&self, service: &Service, bytes: &[u8]) -> bool {
        let Some((checker, timeout_ms, on_error)) = self.checker(service) else {
            return true;
        };
        if service.forwarding.is_some() {
            self.failure(checker, service, "incompatible forwarding destination");
            return false;
        }
        match checker.check(bytes, timeout_ms).await {
            Ok(allow) => allow,
            Err(reason) => {
                self.failure(checker, service, reason);
                matches!(on_error, Fallback::Allow)
            }
        }
    }

    pub(crate) fn framing_failure(&self, service: &Service) -> bool {
        self.checker(service).is_none_or(|(checker, _, on_error)| {
            self.failure(checker, service, "HTTP request framing failed");
            service.forwarding.is_none() && matches!(on_error, Fallback::Allow)
        })
    }

    fn checker(&self, service: &Service) -> Option<(&Checker, u64, Fallback)> {
        let current = self.current.read().unwrap_or_else(|e| e.into_inner());
        let &(index, timeout_ms, on_error) = current.services.get(&service.id)?;
        Some((&self.checkers[index], timeout_ms, on_error))
    }

    fn failure(&self, checker: &Checker, service: &Service, reason: &'static str) {
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        status.errors = status.errors.saturating_add(1);
        status.last_error = format!("{}: {reason}", checker.policy.id);
        let now = Instant::now();
        if status
            .last_logged
            .is_none_or(|previous| now.duration_since(previous) >= Duration::from_secs(1))
        {
            tracing::warn!(checker = %checker.policy.id, service = %service.id, reason, "request checker error");
            status.last_logged = Some(now);
        }
    }

    pub(crate) fn status(&self) -> (u64, String) {
        let status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        (status.errors, status.last_error.clone())
    }
}

fn private_directory(path: &Path) -> anyhow::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).context("create request checker state"),
    }
    let meta = path.symlink_metadata()?;
    ensure!(
        meta.is_dir() && meta.uid() == nix::unistd::geteuid().as_raw() && meta.mode() & 0o077 == 0,
        "request checker state must be a private directory owned by Proxy"
    );
    Ok(())
}

impl Checker {
    async fn check(&self, bytes: &[u8], timeout_ms: u64) -> Result<bool, &'static str> {
        if bytes.len() > MAX_REQUEST {
            return Err("request exceeds input limit");
        }
        let mut slot = self.worker.try_lock().map_err(|_| "worker busy")?;
        if slot.retry_after.is_some_and(|retry| Instant::now() < retry) {
            return Err("worker restarting");
        }
        // Removing the worker from the slot ensures cancellation drops the child
        // instead of allowing the next request to consume an abandoned decision.
        slot.retry_after = Some(Instant::now() + RESTART_DELAY);
        let mut worker = match slot.process.take() {
            Some(worker) => worker,
            None => Worker::start(&self.policy, &self.state)
                .map_err(|_| "could not start isolated worker")?,
        };
        match tokio::time::timeout(Duration::from_millis(timeout_ms), worker.decide(bytes)).await {
            Ok(Ok(allow)) => {
                slot.retry_after = None;
                slot.process = Some(worker);
                Ok(allow)
            }
            Ok(Err(_)) => Err("worker exited or returned an invalid decision"),
            Err(_) => Err("worker timed out"),
        }
    }
}

struct Worker {
    _process: Child,
    input: ChildStdin,
    output: ChildStdout,
}

impl Worker {
    fn start(config: &Config, state: &Path) -> io::Result<Self> {
        let mut command = Command::new("/usr/bin/bwrap");
        command
            .env_clear()
            .args([
                "--unshare-all",
                "--unshare-user",
                "--disable-userns",
                "--cap-drop",
                "ALL",
                "--die-with-parent",
                "--new-session",
            ])
            .args(["--dev", "/dev", "--dir", "/usr/bin"])
            .args(["--ro-bind", "/usr/lib", "/usr/lib"])
            .args(["--ro-bind-try", "/usr/lib64", "/usr/lib64"])
            .args(["--ro-bind-try", "/lib", "/lib"])
            .args(["--ro-bind-try", "/lib64", "/lib64"])
            .arg("--ro-bind")
            .arg(&config.script)
            .arg("/checker")
            .arg("--bind")
            .arg(state)
            .arg("/state");
        match config.runtime {
            Runtime::Python => {
                command.args([
                    "--ro-bind",
                    "/usr/bin/python3",
                    "/usr/bin/python3",
                    "--remount-ro",
                    "/",
                    "--chdir",
                    "/state",
                    "--",
                    "/usr/bin/python3",
                    "-I",
                    "-S",
                    "-B",
                    "-u",
                    "-c",
                    include_str!("worker.py"),
                ]);
            }
            Runtime::Wasm => add_wasm_command(&mut command)?,
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut process = command.spawn()?;
        let input = process
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("missing checker stdin"))?;
        let output = process
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing checker stdout"))?;
        Ok(Self {
            _process: process,
            input,
            output,
        })
    }

    async fn decide(&mut self, bytes: &[u8]) -> io::Result<bool> {
        self.input.write_u32(bytes.len() as u32).await?;
        self.input.write_all(bytes).await?;
        match self.output.read_u8().await? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(io::Error::other("invalid checker decision")),
        }
    }
}

fn add_wasm_command(command: &mut Command) -> io::Result<()> {
    let runtime = std::env::var_os("HYDRA_WASMTIME")
        .map(PathBuf::from)
        .or_else(|| {
            ["/usr/bin/wasmtime", "/usr/local/bin/wasmtime"]
                .into_iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
        })
        .filter(|path| path.is_absolute() && path.is_file())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Wasmtime runtime unavailable"))?;
    command
        .arg("--ro-bind")
        .arg(runtime)
        .arg("/usr/bin/wasmtime")
        .args([
            "--ro-bind",
            "/usr/bin/prlimit",
            "/usr/bin/prlimit",
            "--remount-ro",
            "/",
            "--chdir",
            "/state",
            "--",
            "/usr/bin/prlimit",
            "--fsize=4194304:4194304",
            "--nofile=128:128",
            "--",
            "/usr/bin/wasmtime",
            "run",
            "-C",
            "cache=n",
            "-C",
            "parallel-compilation=n",
            "-S",
            "inherit-network=n",
            "-W",
            "max-memory-size=67108864",
            "-W",
            "max-memories=1",
            "-W",
            "max-tables=4",
            "-W",
            "fuel=500000000",
            "--dir",
            "/state",
            "/checker",
        ]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hydra_domain::Transport;

    fn local_service() -> Service {
        Service {
            id: "local".into(),
            transport: Transport::Tcp,
            listen_address: "127.0.0.1:1234".parse().unwrap(),
            upstream_address: "127.0.0.1:2345".parse().unwrap(),
            protocol_id: "raw".into(),
            enabled: true,
            retain_history: false,
            protection: None,
            forwarding: None,
        }
    }

    #[tokio::test]
    async fn sandbox_uses_persistent_files_and_reports_failures() {
        let Ok(probe) = Command::new("/usr/bin/bwrap")
            .args([
                "--unshare-all",
                "--unshare-user",
                "--disable-userns",
                "--ro-bind",
                "/usr",
                "/usr",
                "--ro-bind-try",
                "/lib",
                "/lib",
                "--ro-bind-try",
                "/lib64",
                "/lib64",
                "/usr/bin/python3",
                "-c",
                "pass",
            ])
            .stderr(Stdio::null())
            .status()
            .await
        else {
            eprintln!("Skipping isolated checker test: bubblewrap unavailable.");
            return;
        };
        if !probe.success() {
            eprintln!("Skipping isolated checker test: user namespaces unavailable.");
            return;
        }

        let root = std::env::temp_dir().join(format!("hydra-checker-{}", uuid::Uuid::new_v4()));
        private_directory(&root).unwrap();
        let script = root.join("check.py");
        std::fs::write(
            &script,
            r#"
from pathlib import Path
import time

def check(request):
    if request == b'timeout':
        time.sleep(3)
    if request == b'exception':
        raise ValueError()
    if request == b'read':
        return Path('/state/marker').read_bytes() == b'written' and not Path('/etc/passwd').exists()
    if request == b'deny':
        return False
    if request == b'write':
        Path('marker').write_bytes(b'written')
        return True
    return False
"#,
        )
        .unwrap();
        let settings = |on_error| {
            vec![Config {
                id: "example".into(),
                services: vec!["local".into()],
                script: script.clone(),
                timeout_ms: 500,
                on_error,
                runtime: Runtime::Python,
            }]
        };
        let service = local_service();
        let directories = root.join("workers");
        let checks = Checks::open(&directories, settings(Fallback::Block)).unwrap();
        assert!(checks.allow(&service, b"write").await);
        assert!(!checks.allow(&service, b"deny").await);
        assert_eq!(checks.status().0, 0);
        assert_eq!(
            std::fs::read(directories.join("example/marker")).unwrap(),
            b"written"
        );

        drop(checks);
        let checks = Checks::open(&directories, settings(Fallback::Block)).unwrap();
        assert!(checks.allow(&service, b"read").await);
        assert!(!checks.allow(&service, b"timeout").await);
        assert_eq!(checks.status(), (1, "example: worker timed out".into()));
        drop(checks);

        let checks = Checks::open(&directories, settings(Fallback::Allow)).unwrap();
        assert!(checks.allow(&service, b"exception").await);
        assert_eq!(
            checks.status(),
            (
                1,
                "example: worker exited or returned an invalid decision".into()
            )
        );
        drop(checks);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wasi_checker_persists_files_and_reports_traps() {
        if std::env::var_os("HYDRA_WASMTIME").is_none()
            && !Path::new("/usr/bin/wasmtime").is_file()
            && !Path::new("/usr/local/bin/wasmtime").is_file()
        {
            eprintln!("Skipping WASI checker test: Wasmtime is unavailable.");
            return;
        }
        let root =
            std::env::temp_dir().join(format!("hydra-wasi-checker-{}", uuid::Uuid::new_v4()));
        private_directory(&root).unwrap();
        let script = root.join("check.wasm");
        std::fs::write(
            &script,
            wat::parse_str(include_str!("fixtures/storage.wat")).unwrap(),
        )
        .unwrap();
        let settings = || {
            vec![Config {
                id: "example".into(),
                services: vec!["local".into()],
                script: script.clone(),
                timeout_ms: 1000,
                on_error: Fallback::Block,
                runtime: Runtime::Wasm,
            }]
        };
        let directory = root.join("workers");
        let service = local_service();
        let checks = Checks::open(&directory, settings()).unwrap();
        assert!(checks.allow(&service, b"w").await, "{:?}", checks.status());
        assert_eq!(
            std::fs::read(directory.join("example/marker")).unwrap(),
            b"x"
        );
        drop(checks);

        let checks = Checks::open(&directory, settings()).unwrap();
        assert!(checks.allow(&service, b"r").await, "{:?}", checks.status());
        assert!(!checks.allow(&service, b"t").await);
        assert_eq!(checks.status().0, 1);
        drop(checks);
        std::fs::remove_dir_all(root).unwrap();
    }
}
