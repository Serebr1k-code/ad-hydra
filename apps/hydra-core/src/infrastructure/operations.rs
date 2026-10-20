//! Validated execution and checker configuration, loaded at startup from the operator file.
use crate::{
    infrastructure::{db::Database, sandbox},
    modules::{farm, flags},
};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "hydra_domain::history::default_per_service_mib")]
    pub traffic_history_per_service_mib: u32,
    #[serde(default = "hydra_compression::default_level")]
    pub traffic_history_compression_level: i32,
    pub flag_pattern: Option<String>,
    #[serde(default)]
    pub auto_submit_captured: bool,
    pub checker: Option<Checker>,
    pub sandbox: Option<SandboxSettings>,
    #[serde(default)]
    pub execution_mode: ExecutionMode,
    pub host_runtime: Option<HostRuntimeSettings>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            traffic_history_per_service_mib: hydra_domain::history::default_per_service_mib(),
            traffic_history_compression_level: hydra_compression::DEFAULT_LEVEL,
            flag_pattern: None,
            auto_submit_captured: false,
            checker: None,
            sandbox: None,
            execution_mode: ExecutionMode::default(),
            host_runtime: None,
        }
    }
}
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    #[default]
    Sandbox,
    Host,
}
#[derive(Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HostRuntimeSettings {
    #[serde(default = "default_uv")]
    #[schema(value_type = String)]
    pub uv: PathBuf,
    #[schema(value_type = String)]
    pub work_dir: PathBuf,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Checker {
    Https {
        address: SocketAddr,
        host: String,
        path: String,
        bearer: Option<String>,
        #[serde(default)]
        ca_certificate_der: Vec<u8>,
        ca_certificate_path: Option<PathBuf>,
    },
    CustomTcp {
        address: SocketAddr,
        request_template: String,
        accepted_prefix: String,
        secret: Option<String>,
        batch_size: usize,
    },
    Http {
        address: SocketAddr,
        host: String,
        path: String,
        bearer: Option<String>,
    },
    Tcp {
        address: SocketAddr,
        secret: Option<String>,
    },
}
#[derive(Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SandboxSettings {
    #[schema(value_type = String)]
    pub delegated_cgroup: PathBuf,
    #[schema(value_type = String)]
    pub runtime_dir: PathBuf,
    #[schema(value_type = String)]
    pub bwrap: PathBuf,
    #[schema(value_type = String)]
    pub ip: PathBuf,
    #[serde(default = "default_uv")]
    #[schema(value_type = String)]
    pub uv: PathBuf,
}
fn default_uv() -> PathBuf {
    "/usr/bin/uv".into()
}
impl Settings {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path)?;
        anyhow::ensure!(
            metadata.is_file()
                && metadata.mode() & 0o077 == 0
                && metadata.nlink() == 1
                && metadata.len() <= 65536,
            "operations config must be a private regular file <=64 KiB"
        );
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        let opened = file.metadata()?;
        anyhow::ensure!(
            opened.ino() == metadata.ino() && opened.dev() == metadata.dev(),
            "operations config changed while opening"
        );
        let mut bytes = Vec::new();
        file.take(65537).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 65536, "operations config exceeds 64 KiB");
        let mut settings: Self = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            (hydra_domain::history::MIN_PER_SERVICE_MIB
                ..=hydra_domain::history::MAX_PER_SERVICE_MIB)
                .contains(&settings.traffic_history_per_service_mib),
            "traffic history must be 1..1024 MiB per service"
        );
        anyhow::ensure!(
            (hydra_compression::MIN_LEVEL..=hydra_compression::MAX_LEVEL)
                .contains(&settings.traffic_history_compression_level),
            "traffic compression level must be 1..=19"
        );
        if let Some(Checker::Https {
            ca_certificate_der,
            ca_certificate_path: Some(certificate),
            ..
        }) = &mut settings.checker
        {
            anyhow::ensure!(
                ca_certificate_der.is_empty(),
                "choose one checker CA source"
            );
            let metadata = std::fs::symlink_metadata(&certificate)?;
            anyhow::ensure!(
                metadata.is_file() && metadata.len() <= 65536,
                "checker CA must be a regular DER file <=64KiB"
            );
            let file = std::fs::File::open(certificate)?;
            file.take(65537).read_to_end(ca_certificate_der)?;
            anyhow::ensure!(
                ca_certificate_der.len() <= 65536,
                "checker CA exceeds 64 KiB"
            );
        }
        Ok(settings)
    }
    pub fn flags(&self) -> flags::Settings {
        flags::Settings {
            pattern: self.flag_pattern.clone(),
            script_runtime: (self.execution_mode == ExecutionMode::Host)
                .then(|| self.host_runtime.clone())
                .flatten(),
            auto_submit_captured: self.auto_submit_captured,
            checker: self.checker.as_ref().map(|c| match c {
                Checker::Https {
                    address,
                    host,
                    path,
                    bearer,
                    ca_certificate_der,
                    ..
                } => flags::Checker::Https {
                    address: *address,
                    host: host.clone(),
                    path: path.clone(),
                    bearer: bearer.clone(),
                    ca_certificate_der: ca_certificate_der.clone(),
                },
                Checker::CustomTcp {
                    address,
                    request_template,
                    accepted_prefix,
                    secret,
                    batch_size,
                } => flags::Checker::CustomTcp {
                    address: *address,
                    request_template: request_template.clone(),
                    accepted_prefix: accepted_prefix.clone(),
                    secret: secret.clone(),
                    batch_size: *batch_size,
                },
                Checker::Http {
                    address,
                    host,
                    path,
                    bearer,
                } => flags::Checker::Http {
                    address: *address,
                    host: host.clone(),
                    path: path.clone(),
                    bearer: bearer.clone(),
                },
                Checker::Tcp { address, secret } => flags::Checker::Tcp {
                    address: *address,
                    secret: secret.clone(),
                },
            }),
        }
    }
}
pub struct Runner {
    pub settings: Option<SandboxSettings>,
    pub flags: flags::Flags,
    pub execution_mode: ExecutionMode,
    pub host_runtime: Option<HostRuntimeSettings>,
    pub db: Database,
}
impl farm::Runner for Runner {
    fn run(
        &self,
        job: farm::Job,
        cancel: CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<farm::RunOutput>> + Send>>
    {
        let settings = self.settings.clone();
        let flags = self.flags.clone();
        if self.execution_mode == ExecutionMode::Host {
            let settings = self.host_runtime.clone();
            let db = self.db.clone();
            return Box::pin(async move {
                let settings =
                    settings.ok_or_else(|| anyhow::anyhow!("host runtime not configured"))?;
                crate::infrastructure::host_process::run(settings, job, cancel, flags, db).await
            });
        }
        let db = self.db.clone();
        Box::pin(async move {
            let settings = settings
                .ok_or_else(|| anyhow::anyhow!("sandbox not configured; execution refused"))?;
            anyhow::ensure!(!job.targets.is_empty(), "job has no targets");
            let endpoints = job
                .targets
                .iter()
                .map(|t| {
                    Ok(sandbox::Endpoint {
                        id: t.id.clone(),
                        address: SocketAddr::new(t.host.parse()?, t.port),
                        transport: t.transport.clone(),
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let targets = endpoints.iter().map(|e| e.address).collect::<Vec<_>>();
            let sandbox = sandbox::Sandbox::new(sandbox::Config {
                delegated_cgroup: settings.delegated_cgroup,
                runtime_dir: settings.runtime_dir.clone(),
                bwrap: settings.bwrap,
                ip: settings.ip,
                uv: settings.uv,
                helper: std::env::current_exe()?,
                allowlist: targets.clone(),
            })?;
            let source = settings
                .runtime_dir
                .join(format!("source-{}", uuid::Uuid::new_v4()));
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            std::fs::DirBuilder::new().mode(0o700).create(&source)?;
            struct Cleanup(PathBuf);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    let _ = std::fs::remove_dir_all(&self.0);
                }
            }
            let _cleanup = Cleanup(source.clone());
            let bytes = tokio::fs::read(&job.artifact).await?;
            anyhow::ensure!(
                !bytes.is_empty() && bytes.len() <= 2 * 1024 * 1024,
                "invalid immutable artifact"
            );
            let entrypoint = "exploit";
            std::fs::write(source.join(entrypoint), bytes)?;
            std::fs::set_permissions(
                source.join(entrypoint),
                std::fs::Permissions::from_mode(0o500),
            )?;
            let mut output = farm::RunOutput {
                status: "succeeded".into(),
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: Some(0),
            };
            let batches = if job.manifest.mode == "batch" {
                vec![endpoints]
            } else {
                endpoints.into_iter().map(|e| vec![e]).collect()
            };
            let deadline =
                tokio::time::Instant::now() + Duration::from_secs(job.manifest.timeout_seconds);
            for endpoints in batches {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    output.status = "timeout".into();
                    break;
                }
                let address = endpoints[0].address;
                let target_id = endpoints[0].id.clone();
                if cancel.is_cancelled() {
                    output.status = "cancelled".into();
                    break;
                }
                let label = if job.manifest.mode == "batch" {
                    format!(
                        "\n[batch targets={}]\n",
                        endpoints
                            .iter()
                            .map(|e| e.id.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                } else {
                    let target = job
                        .targets
                        .iter()
                        .find(|t| t.id == target_id)
                        .ok_or_else(|| anyhow::anyhow!("target missing"))?;
                    format!(
                        "\n[target id={} name={:?} host={} port={}]\n",
                        target.id, target.name, target.host, target.port
                    )
                };
                for stream in ["stdout", "stderr"] {
                    farm::runtimes::chunks::append(&db, &job.run_id, stream, label.as_bytes())
                        .await?;
                }
                append_bounded(&mut output.stdout, label.as_bytes());
                append_bounded(&mut output.stderr, label.as_bytes());
                let (result_target, scope) = if job.manifest.mode == "batch" {
                    ("", "batch")
                } else {
                    (target_id.as_str(), "per_target")
                };
                farm::process_results::started(&db, &job.run_id, result_target, scope).await?;
                let result = sandbox
                    .run(
                        sandbox::Job {
                            source_dir: source.clone(),
                            entrypoint: entrypoint.into(),
                            runtime: match job.manifest.runtime.as_str() {
                                "python" => sandbox::Runtime::Python,
                                "shell" => sandbox::Runtime::Shell,
                                _ => sandbox::Runtime::Elf,
                            },
                            python_packages: job.manifest.python_packages.clone(),
                            python_environment_key: format!("{}:{}", job.exploit_id, job.version),
                            target: address,
                            endpoints,
                            timeout: remaining,
                            memory_bytes: job.manifest.memory_mib * 1024 * 1024,
                            cpu_millis_per_second: job.manifest.cpu_percent * 10,
                            pids: 32,
                        },
                        cancel.clone(),
                    )
                    .await;
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        farm::process_results::finished(
                            &db,
                            &job.run_id,
                            result_target,
                            "failed",
                            None,
                        )
                        .await?;
                        return Err(error.into());
                    }
                };
                farm::process_results::finished(
                    &db,
                    &job.run_id,
                    result_target,
                    sandbox_outcome_state(&result),
                    result.exit_code,
                )
                .await?;
                persist_worker_output(
                    &db,
                    &job.run_id,
                    &result.stdout,
                    &result.stderr,
                    result.stdout_lost_bytes,
                    result.stderr_lost_bytes,
                )
                .await?;
                let captures = if job.manifest.mode == "batch" {
                    batch_captures(&flags, &result.stdout, &job.targets)?
                        .into_iter()
                        .chain(batch_captures(&flags, &result.stderr, &job.targets)?)
                        .collect::<Vec<_>>()
                } else {
                    flags
                        .extract(&result.stdout)?
                        .into_iter()
                        .chain(flags.extract(&result.stderr)?)
                        .map(|value| (target_id.clone(), value))
                        .collect()
                };
                for (target_id, value) in captures {
                    flags
                        .record_farm_captured(
                            value,
                            target_id,
                            job.run_id.clone(),
                            job.initiated_by.clone(),
                        )
                        .await?;
                }
                output.exit_code = result.exit_code;
                if !result.success {
                    output.status = if result.cancelled {
                        "cancelled"
                    } else if result.timed_out {
                        "timeout"
                    } else {
                        "failed"
                    }
                    .into();
                }
                append_bounded(&mut output.stdout, &result.stdout);
                append_bounded(&mut output.stderr, &result.stderr);
                if result.stdout_lost_bytes > 0 || result.stderr_lost_bytes > 0 {
                    append_bounded(&mut output.stderr, b"\n[worker output truncated]\n");
                }
                if result.cancelled {
                    break;
                }
            }
            Ok(output)
        })
    }
}
fn sandbox_outcome_state(result: &sandbox::RunOutcome) -> &'static str {
    if result.cancelled {
        "cancelled"
    } else if result.timed_out {
        "timeout"
    } else if result.success {
        "succeeded"
    } else {
        "failed"
    }
}
async fn persist_worker_output(
    db: &Database,
    id: &str,
    stdout: &[u8],
    stderr: &[u8],
    stdout_lost: u64,
    stderr_lost: u64,
) -> anyhow::Result<()> {
    for (stream, bytes) in [("stdout", stdout), ("stderr", stderr)] {
        for chunk in bytes.chunks(32768) {
            farm::runtimes::chunks::append(db, id, stream, chunk).await?;
        }
    }
    if stdout_lost > 0 || stderr_lost > 0 {
        let marker = format!(
            "\n[worker output lost: stdout_bytes={stdout_lost} stderr_bytes={stderr_lost}]\n"
        );
        farm::runtimes::chunks::append(db, id, "stderr", marker.as_bytes()).await?;
    }
    Ok(())
}

pub(crate) fn append_bounded(to: &mut Vec<u8>, bytes: &[u8]) {
    let limit = farm::runtimes::chunks::PREVIEW_BYTES;
    let n = limit.saturating_sub(to.len()).min(bytes.len());
    to.extend_from_slice(&bytes[..n]);
}
pub fn runner(
    settings: Option<SandboxSettings>,
    execution_mode: ExecutionMode,
    host_runtime: Option<HostRuntimeSettings>,
    flags: flags::Flags,
    db: Database,
) -> Arc<dyn farm::Runner> {
    Arc::new(Runner {
        settings,
        execution_mode,
        host_runtime,
        flags,
        db,
    })
}

/// Batch attribution is explicit and restricted to this immutable job's targets.
pub(crate) fn batch_captures(
    flags: &flags::Flags,
    bytes: &[u8],
    targets: &[farm::Target],
) -> anyhow::Result<Vec<(String, String)>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Capture {
        target_id: String,
        flag: String,
    }
    let mut captures = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        let Ok(capture) = serde_json::from_slice::<Capture>(line) else {
            continue;
        };
        if capture.flag.len() > 256 || !targets.iter().any(|t| t.id == capture.target_id) {
            continue;
        }
        if flags
            .extract(capture.flag.as_bytes())?
            .iter()
            .any(|value| value == &capture.flag)
        {
            captures.push((capture.target_id, capture.flag));
        }
    }
    Ok(captures)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sandbox_attempt_state_uses_its_actual_result() {
        let mut result = sandbox::RunOutcome {
            success: true,
            exit_code: Some(0),
            timed_out: false,
            cancelled: false,
            stdout: vec![],
            stderr: vec![],
            stdout_lost_bytes: 0,
            stderr_lost_bytes: 0,
        };
        assert_eq!(sandbox_outcome_state(&result), "succeeded");
        result.success = false;
        result.exit_code = Some(7);
        assert_eq!(sandbox_outcome_state(&result), "failed");
        result.timed_out = true;
        assert_eq!(sandbox_outcome_state(&result), "timeout");
        result.cancelled = true;
        assert_eq!(sandbox_outcome_state(&result), "cancelled");
    }
    #[tokio::test]
    async fn batch_flags_require_full_match_and_selected_target() {
        use crate::{
            infrastructure::{crypto::Crypto, db::Database},
            modules::auth::AuthState,
        };
        use std::os::unix::fs::DirBuilderExt;
        let directory = std::env::temp_dir().join(format!("hydra-batch-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let auth = AuthState::new(
            Database::open(directory.join("db")).await.unwrap(),
            Crypto::load(&directory.join("key")).unwrap(),
            "https://localhost".into(),
        );
        let flags = flags::Flags::open(
            auth,
            flags::Settings {
                pattern: Some("FLAG_[A-Z]+".into()),
                auto_submit_captured: false,
                checker: None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let target = farm::Target {
            id: "selected".into(),
            name: "selected".into(),
            host: "192.0.2.1".into(),
            port: 9000,
            transport: "udp".into(),
            enabled: true,
            revision: 1,
        };
        let lines = br#"{"target_id":"selected","flag":"FLAG_GOOD"}
{"target_id":"unselected","flag":"FLAG_BAD"}
{"target_id":"selected","flag":"prefix FLAG_BAD"}
FLAG_UNATTRIBUTED
"#;
        assert_eq!(
            batch_captures(&flags, lines, &[target]).unwrap(),
            vec![("selected".into(), "FLAG_GOOD".into())]
        );
        drop(flags);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn sandbox_result_persistence_preserves_multiple_large_targets_and_loss_markers() {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!("hydra-worker-log-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        let db = Database::open(path.join("db")).await.unwrap();
        db.migrate(
            16,
            include_str!("../../../../migrations/0016_farm_catalog_logs.sql"),
        )
        .await
        .unwrap();
        db.migrate(
            20,
            include_str!("../../../../migrations/0020_farm_stream_logs.sql"),
        )
        .await
        .unwrap();
        db.run(|db|{db.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at) VALUES('r','e','v','{}','[]','a','running',0)",[])?;Ok(())}).await.unwrap();
        persist_worker_output(&db, "r", &vec![b'A'; 70000], b"first", 0, 0)
            .await
            .unwrap();
        persist_worker_output(&db, "r", &vec![b'B'; 70000], b"second", 12, 34)
            .await
            .unwrap();
        let actor = crate::modules::audit::Actor::user("a".into());
        let page = farm::runtimes::chunks::page(
            &db,
            actor.clone(),
            "r".into(),
            "stdout".into(),
            69995,
            16,
        )
        .await
        .unwrap();
        assert_eq!(page["text"], "AAAAABBBBBBBBBBB");
        assert_eq!(page["end_offset"], 140000);
        let page = farm::runtimes::chunks::page(&db, actor, "r".into(), "stderr".into(), 0, 65536)
            .await
            .unwrap();
        assert!(
            page["text"]
                .as_str()
                .unwrap()
                .contains("firstsecond\n[worker output lost: stdout_bytes=12 stderr_bytes=34]")
        );
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn aggregate_output_loss_is_explicit_and_bounded() {
        let mut output = Vec::new();
        append_bounded(&mut output, &vec![b'x'; 1024 * 1024]);
        append_bounded(&mut output, b"extra");
        assert_eq!(output.len(), farm::runtimes::chunks::PREVIEW_BYTES);
        assert!(output.iter().all(|b| *b == b'x'));
    }
}
