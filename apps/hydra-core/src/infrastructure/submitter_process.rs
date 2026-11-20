//! Explicit operator-selected Python submitters, executed with Core's OS privileges.
use super::operations::HostRuntimeSettings;
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 65536;
const TRUNCATION: &[u8] = b"\n[output truncated]\n";

#[derive(Debug)]
pub(crate) struct ScriptOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub cancelled: bool,
}
#[derive(Clone)]
pub(crate) struct ForcadRuntimeConfig {
    pub ip: String,
    pub port: u16,
    pub team_token: String,
}
struct ScriptRequest {
    settings: HostRuntimeSettings,
    source: String,
    packages: Vec<String>,
    flags: Vec<String>,
    batch_id: String,
    timeout: Duration,
    cancel: CancellationToken,
}
impl ScriptOutput {
    fn empty() -> Self {
        Self {
            success: false,
            exit_code: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
            timed_out: false,
            cancelled: false,
        }
    }
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct ProcessGroup(i32);
impl ProcessGroup {
    fn kill(&self) {
        let _ = killpg(Pid::from_raw(self.0), Signal::SIGKILL);
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}
fn private_directory(path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        path.is_absolute(),
        "submitter work directory must be absolute"
    );
    if !path.exists() {
        std::fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.mode() & 0o077 == 0
            && metadata.uid() == nix::unistd::geteuid().as_raw(),
        "submitter work directory must be private and owned by Core user"
    );
    Ok(())
}
fn append_bounded(output: &mut Vec<u8>, bytes: &[u8]) {
    let count = OUTPUT_LIMIT.saturating_sub(output.len()).min(bytes.len());
    output.extend_from_slice(&bytes[..count]);
    if count < bytes.len() {
        output.truncate(OUTPUT_LIMIT - TRUNCATION.len());
        output.extend_from_slice(TRUNCATION);
    }
}

pub(crate) async fn run(
    settings: HostRuntimeSettings,
    source: String,
    packages: Vec<String>,
    flags: Vec<String>,
    batch_id: String,
    timeout: Duration,
    cancel: CancellationToken,
) -> anyhow::Result<ScriptOutput> {
    run_inner(
        ScriptRequest {
            settings,
            source,
            packages,
            flags,
            batch_id,
            timeout,
            cancel,
        },
        None,
    )
    .await
}

pub(crate) async fn run_forcad(
    settings: HostRuntimeSettings,
    flags: Vec<String>,
    batch_id: String,
    timeout: Duration,
    cancel: CancellationToken,
    config: ForcadRuntimeConfig,
) -> anyhow::Result<ScriptOutput> {
    let source = include_str!("../modules/flags/templates/forcad_submitter.py").to_string();
    run_inner(
        ScriptRequest {
            settings,
            source,
            packages: vec![],
            flags,
            batch_id,
            timeout,
            cancel,
        },
        Some(config),
    )
    .await
}

async fn run_inner(
    request: ScriptRequest,
    forcad: Option<ForcadRuntimeConfig>,
) -> anyhow::Result<ScriptOutput> {
    let ScriptRequest {
        settings,
        source,
        packages,
        flags,
        batch_id,
        timeout,
        cancel,
    } = request;
    anyhow::ensure!(settings.uv.is_absolute(), "uv path must be absolute");
    anyhow::ensure!(
        !source.is_empty() && source.len() <= 2 * 1024 * 1024,
        "invalid submitter source size"
    );
    anyhow::ensure!(
        !flags.is_empty()
            && flags.len() <= if forcad.is_some() { 100 } else { 256 }
            && flags.iter().map(String::len).sum::<usize>() <= 60 * 1024
            && flags
                .iter()
                .all(|flag| !flag.is_empty() && !flag.contains('\0')),
        "invalid submitter flag batch"
    );
    anyhow::ensure!(
        packages.len() <= 64
            && packages
                .iter()
                .all(|package| package.len() <= 256 && !package.contains('\0')),
        "invalid submitter dependency list"
    );
    anyhow::ensure!(
        !timeout.is_zero() && batch_id.len() <= 128 && !batch_id.contains('\0'),
        "invalid submitter attempt settings"
    );
    let mut output = ScriptOutput::empty();
    if cancel.is_cancelled() {
        output.cancelled = true;
        return Ok(output);
    }
    private_directory(&settings.work_dir)?;
    let root = settings.work_dir.join("submitter");
    private_directory(&root)?;
    let home = root.join("home");
    private_directory(&home)?;
    let cache = root.join("uv-cache");
    private_directory(&cache)?;
    let directory = root.join(format!("attempt-{}", uuid::Uuid::new_v4()));
    private_directory(&directory)?;
    let _directory = Directory(directory.clone());
    let entrypoint = directory.join("submit.py");
    let mut script = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&entrypoint)?;
    script.write_all(source.as_bytes())?;
    drop(script);
    if let Some(config) = forcad {
        anyhow::ensure!(
            config.ip.parse::<std::net::IpAddr>().is_ok() && config.port != 0,
            "invalid ForcAD address"
        );
        anyhow::ensure!(
            config.team_token.len() <= 512
                && config
                    .team_token
                    .bytes()
                    .all(|byte| (0x21..=0x7e).contains(&byte)),
            "invalid ForcAD token"
        );
        let path = directory.join("forcad.json");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        serde_json::to_writer(
            file,
            &serde_json::json!({"ip":config.ip,"port":config.port,"team_token":config.team_token}),
        )?;
    }
    let mut command = Command::new(&settings.uv);
    command.args(["run", "--no-project"]);
    for package in packages {
        command.arg("--with").arg(package);
    }
    command.args(["python", "-u"]).arg(&entrypoint);
    // The built-in adapter resolves its private config beside submit.py, leaving
    // sys.argv[1:] as the exact captured flags for the established script contract.
    command
        .args(flags)
        .current_dir(&directory)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("UV_CACHE_DIR", &cache)
        .env("UV_PYTHON_INSTALL_DIR", home.join("python"))
        .env("HYDRA_SUBMISSION_BATCH_ID", batch_id)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn()?;
    let group = ProcessGroup(
        child
            .id()
            .ok_or_else(|| anyhow::anyhow!("submitter pid missing"))? as i32,
    );
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut out_open = true;
    let mut err_open = true;
    let mut out_chunk = [0u8; 8192];
    let mut err_chunk = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + timeout;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status?,
            _ = cancel.cancelled() => {
                output.cancelled = true;
                group.kill();
                break child.wait().await?;
            }
            _ = tokio::time::sleep_until(deadline) => {
                output.timed_out = true;
                group.kill();
                break child.wait().await?;
            }
            value = stdout.read(&mut out_chunk), if out_open => {
                let n = value?; out_open = n > 0;
                append_bounded(&mut output.stdout, &out_chunk[..n]);
            }
            value = stderr.read(&mut err_chunk), if err_open => {
                let n = value?; err_open = n > 0;
                append_bounded(&mut output.stderr, &err_chunk[..n]);
            }
        }
    };
    // A successful parent can leave children holding pipes open. Kill those too.
    drop(group);
    let drain = async {
        while out_open || err_open {
            tokio::select! {
                value = stdout.read(&mut out_chunk), if out_open => {
                    let n = value?; out_open = n > 0;
                    append_bounded(&mut output.stdout, &out_chunk[..n]);
                }
                value = stderr.read(&mut err_chunk), if err_open => {
                    let n = value?; err_open = n > 0;
                    append_bounded(&mut output.stderr, &err_chunk[..n]);
                }
            }
        }
        Ok::<_, std::io::Error>(())
    };
    match tokio::time::timeout(Duration::from_millis(500), drain).await {
        Ok(result) => result?,
        Err(_) => append_bounded(&mut output.stderr, b"\n[output drain timed out]\n"),
    }
    output.exit_code = status.code();
    output.success = status.success() && !output.timed_out && !output.cancelled;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(script: &[u8]) -> (Directory, HostRuntimeSettings) {
        let directory =
            std::env::temp_dir().join(format!("hydra-submitter-test-{}", uuid::Uuid::new_v4()));
        private_directory(&directory).unwrap();
        let uv = directory.join("uv");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&uv)
            .unwrap();
        file.write_all(b"#!/bin/bash\n").unwrap();
        file.write_all(script).unwrap();
        drop(file);
        let settings = HostRuntimeSettings {
            uv,
            work_dir: directory.join("work"),
        };
        (Directory(directory), settings)
    }
    async fn execute(settings: HostRuntimeSettings) -> anyhow::Result<ScriptOutput> {
        run(
            settings,
            "print('test')\n".into(),
            vec![],
            vec!["FLAG_EXAMPLE".into()],
            "test-batch".into(),
            Duration::from_secs(3),
            CancellationToken::new(),
        )
        .await
    }

    #[tokio::test]
    async fn uv_argv_has_only_packages_script_and_flags_and_private_environment() {
        let (_directory, settings) = fixture(br#"printf '<%s>\n' "$@"
printf 'batch=%s target=%s home=%s cache=%s' "$HYDRA_SUBMISSION_BATCH_ID" "${HYDRA_TARGET_HOST-unset}" "$HOME" "$UV_CACHE_DIR" >&2
"#);
        let work_dir = settings.work_dir.clone();
        let output = run(
            settings,
            "print('test')\n".into(),
            vec!["requests".into(), "pwntools==4.14.1".into()],
            vec!["FLAG_ONE".into(), "FLAG TWO".into()],
            "batch-one".into(),
            Duration::from_secs(3),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(output.success);
        assert_eq!(output.exit_code, Some(0));
        assert!(!output.cancelled && !output.timed_out);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("<run>\n<--no-project>\n<--with>\n<requests>\n<--with>\n<pwntools==4.14.1>\n<python>\n<-u>\n"));
        assert!(stdout.ends_with("submit.py>\n<FLAG_ONE>\n<FLAG TWO>\n"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.starts_with("batch=batch-one target=unset home="));
        assert!(stderr.contains("/submitter/home cache="));
        assert!(stderr.ends_with("/submitter/uv-cache"));
        let root = work_dir.join("submitter");
        assert!(root.join("uv-cache").is_dir());
        assert!(std::fs::read_dir(root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("attempt-")
        }));
    }

    #[tokio::test]
    async fn stdout_and_stderr_are_independently_bounded_and_marked() {
        let (_directory, settings) =
            fixture(b"head -c 131072 /dev/zero\nhead -c 131072 /dev/zero >&2\n");
        let output = execute(settings).await.unwrap();
        assert!(output.success);
        assert_eq!(output.stdout.len(), OUTPUT_LIMIT);
        assert_eq!(output.stderr.len(), OUTPUT_LIMIT);
        assert!(output.stdout.ends_with(TRUNCATION));
        assert!(output.stderr.ends_with(TRUNCATION));
    }

    #[tokio::test]
    async fn nonzero_exit_is_a_failed_attempt_with_logs() {
        let (_directory, settings) = fixture(b"printf 'diagnostic' >&2\nexit 17\n");
        let output = execute(settings).await.unwrap();
        assert!(!output.success);
        assert_eq!(output.exit_code, Some(17));
        assert_eq!(output.stderr, b"diagnostic");
    }

    #[tokio::test]
    async fn timeout_and_cancel_reap_background_pipe_holders() {
        for cancelled in [false, true] {
            let (_directory, settings) = fixture(b"printf 'before-stop'; sleep 20 & wait\n");
            let cancel = CancellationToken::new();
            let token = cancel.clone();
            let task = tokio::spawn(run(
                settings,
                "print('test')".into(),
                vec![],
                vec!["FLAG_ONE".into()],
                "batch".into(),
                Duration::from_millis(if cancelled { 5000 } else { 150 }),
                cancel,
            ));
            if cancelled {
                tokio::time::sleep(Duration::from_millis(150)).await;
                token.cancel();
            }
            let output = tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(!output.success);
            assert_eq!(output.cancelled, cancelled);
            assert_eq!(output.timed_out, !cancelled);
            assert_eq!(output.exit_code, None);
            assert_eq!(output.stdout, b"before-stop");
        }
    }

    #[tokio::test]
    async fn dropping_future_kills_whole_group_and_removes_attempt() {
        let (_directory, settings) =
            fixture(b"sleep 20 & printf '%s %s\\n' \"$$\" \"$!\" >../pids; wait\n");
        let root = settings.work_dir.join("submitter");
        let task = tokio::spawn(execute(settings));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let pids = loop {
            if let Ok(text) = std::fs::read_to_string(root.join("pids")) {
                let pids = text
                    .split_whitespace()
                    .filter_map(|value| value.parse::<u32>().ok())
                    .collect::<Vec<_>>();
                if pids.len() == 2 {
                    break pids;
                }
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let running = pids.iter().any(|pid| {
                std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                    !stat
                        .rsplit_once(") ")
                        .is_some_and(|(_, tail)| tail.starts_with('Z'))
                })
            });
            if !running {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "submitter child survived future drop"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(std::fs::read_dir(root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("attempt-")
        }));
    }

    #[tokio::test]
    async fn pre_cancelled_attempt_never_spawns_uv() {
        let (_directory, settings) = fixture(b"exit 99\n");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let output = run(
            settings,
            "print('test')".into(),
            vec![],
            vec!["FLAG_ONE".into()],
            "batch".into(),
            Duration::from_secs(1),
            cancel,
        )
        .await
        .unwrap();
        assert!(output.cancelled);
        assert_eq!(output.exit_code, None);
    }

    #[test]
    fn private_directory_rejects_public_symlink_and_relative_paths() {
        assert!(private_directory(Path::new("relative")).is_err());
        assert!(private_directory(Path::new("/tmp")).is_err());
        let (directory, _) = fixture(b"exit 0\n");
        let link = directory.0.join("link");
        std::os::unix::fs::symlink(&directory.0, &link).unwrap();
        assert!(private_directory(&link).is_err());
        assert_eq!(
            std::fs::metadata(&directory.0)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
