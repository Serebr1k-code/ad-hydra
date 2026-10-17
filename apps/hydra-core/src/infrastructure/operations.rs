//! Operator-only execution and checker configuration; never populated from HTTP input.
use crate::{
    infrastructure::sandbox,
    modules::{farm, flags},
};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub flag_pattern: Option<String>,
    #[serde(default)]
    pub auto_submit_captured: bool,
    pub checker: Option<Checker>,
    pub sandbox: Option<SandboxSettings>,
}
#[derive(Deserialize)]
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
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxSettings {
    pub delegated_cgroup: PathBuf,
    pub runtime_dir: PathBuf,
    pub bwrap: PathBuf,
    pub ip: PathBuf,
    #[serde(default = "default_uv")]
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
        let mut settings: Self = serde_json::from_slice(&std::fs::read(path)?)?;
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
            *ca_certificate_der = std::fs::read(certificate)?;
        }
        Ok(settings)
    }
    pub fn flags(&self) -> flags::Settings {
        flags::Settings {
            pattern: self.flag_pattern.clone(),
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
fn append_bounded(to: &mut Vec<u8>, bytes: &[u8]) {
    let n = (65536 - to.len()).min(bytes.len());
    to.extend_from_slice(&bytes[..n]);
    if n < bytes.len() {
        let marker = b"\n[aggregate output truncated]\n";
        to.truncate(65536 - marker.len());
        to.extend_from_slice(marker);
    }
}
pub fn runner(settings: Option<SandboxSettings>, flags: flags::Flags) -> Arc<dyn farm::Runner> {
    Arc::new(Runner { settings, flags })
}

/// Batch attribution is explicit and restricted to this immutable job's targets.
fn batch_captures(
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
        if !targets.iter().any(|t| t.id == capture.target_id) {
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
    #[test]
    fn aggregate_output_loss_is_explicit_and_bounded() {
        let mut output = Vec::new();
        append_bounded(&mut output, &vec![b'x'; 65536]);
        append_bounded(&mut output, b"extra");
        assert_eq!(output.len(), 65536);
        assert!(output.ends_with(b"[aggregate output truncated]\n"));
    }
}
