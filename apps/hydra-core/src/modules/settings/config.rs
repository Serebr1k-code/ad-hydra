//! Public editable configuration. Existing checker credentials are write-only.
use crate::infrastructure::operations;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretUpdate {
    Keep,
    Replace { value: Option<String> },
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckerConfig {
    Http {
        address: String,
        host: String,
        path: String,
        bearer: SecretUpdate,
        #[serde(default)]
        credential_configured: bool,
    },
    Https {
        address: String,
        host: String,
        path: String,
        bearer: SecretUpdate,
        #[serde(default)]
        credential_configured: bool,
        #[serde(default)]
        ca_certificate_der: Vec<u8>,
        ca_certificate_path: Option<String>,
    },
    Tcp {
        address: String,
        secret: SecretUpdate,
        #[serde(default)]
        credential_configured: bool,
    },
    CustomTcp {
        address: String,
        request_template: String,
        accepted_prefix: String,
        secret: SecretUpdate,
        batch_size: usize,
        #[serde(default)]
        credential_configured: bool,
    },
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EditableConfig {
    #[serde(default = "hydra_domain::history::default_per_service_mib")]
    pub traffic_history_per_service_mib: u32,
    pub flag_pattern: Option<String>,
    pub auto_submit_captured: bool,
    pub execution_mode: operations::ExecutionMode,
    pub host_runtime: Option<operations::HostRuntimeSettings>,
    pub sandbox: Option<operations::SandboxSettings>,
    pub checker: Option<CheckerConfig>,
}
impl EditableConfig {
    pub fn redacted(settings: &operations::Settings) -> anyhow::Result<Self> {
        let mut value = serde_json::to_value(settings)?;
        if let Some(checker) = value.get_mut("checker").and_then(|v| v.as_object_mut()) {
            let field = if checker.contains_key("bearer") {
                "bearer"
            } else {
                "secret"
            };
            let configured = checker.get(field).is_some_and(|s| !s.is_null());
            checker.insert(field.into(), serde_json::json!({"mode":"keep"}));
            checker.insert("credential_configured".into(), configured.into());
        }
        Ok(serde_json::from_value(value)?)
    }
    pub fn resolve(&self, previous: &operations::Settings) -> anyhow::Result<operations::Settings> {
        let mut value = serde_json::to_value(self)?;
        if let Some(checker) = value.get_mut("checker").and_then(|v| v.as_object_mut()) {
            checker.remove("credential_configured");
            let field = if checker.contains_key("bearer") {
                "bearer"
            } else {
                "secret"
            };
            let update: SecretUpdate = serde_json::from_value(checker[field].clone())?;
            let secret = match update {
                SecretUpdate::Replace { value } => {
                    anyhow::ensure!(
                        value
                            .as_ref()
                            .is_none_or(|s| s.len() <= 4096 && !s.contains(['\r', '\n', '\0'])),
                        "credential must be <=4096 bytes without control delimiters"
                    );
                    serde_json::to_value(value)?
                }
                SecretUpdate::Keep => {
                    let old = serde_json::to_value(previous)?;
                    // Keep only within the same adapter; never transfer secrets between protocols.
                    if old["checker"]["kind"] == checker["kind"] {
                        old["checker"][field].clone()
                    } else {
                        serde_json::Value::Null
                    }
                }
            };
            checker.insert(field.into(), secret);
        }
        Ok(serde_json::from_value(value)?)
    }
}

pub fn validate(settings: &operations::Settings) -> anyhow::Result<()> {
    anyhow::ensure!(
        (hydra_domain::history::MIN_PER_SERVICE_MIB..=hydra_domain::history::MAX_PER_SERVICE_MIB)
            .contains(&settings.traffic_history_per_service_mib),
        "traffic history must be 1..1024 MiB per service"
    );
    anyhow::ensure!(
        !settings.auto_submit_captured || settings.checker.is_some(),
        "automatic submission requires a checker"
    );
    if let Some(pattern) = &settings.flag_pattern {
        anyhow::ensure!(pattern.len() <= 4096, "flag_pattern exceeds 4096 bytes");
        let re = regex::RegexBuilder::new(pattern)
            .size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .build()
            .map_err(|_| anyhow::anyhow!("invalid flag_pattern"))?;
        anyhow::ensure!(!re.is_match(""), "flag_pattern must not match empty input");
    }

    fn absolute(path: &std::path::Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            path.is_absolute()
                && path.as_os_str().len() <= 4096
                && !path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir)),
            "runtime paths must be absolute without parent traversal and <=4096 bytes"
        );
        Ok(())
    }
    if settings.execution_mode == operations::ExecutionMode::Host {
        anyhow::ensure!(
            settings.host_runtime.is_some(),
            "host execution requires host_runtime"
        );
    }
    if let Some(host) = &settings.host_runtime {
        absolute(&host.uv)?;
        absolute(&host.work_dir)?;
    }
    if let Some(sandbox) = &settings.sandbox {
        for path in [
            &sandbox.delegated_cgroup,
            &sandbox.runtime_dir,
            &sandbox.bwrap,
            &sandbox.ip,
            &sandbox.uv,
        ] {
            absolute(path)?;
        }
    }
    if let Some(operations::Checker::Https {
        ca_certificate_der,
        ca_certificate_path,
        ..
    }) = &settings.checker
    {
        anyhow::ensure!(ca_certificate_der.len() <= 65536, "checker CA too large");
        anyhow::ensure!(
            ca_certificate_der.is_empty() || ca_certificate_path.is_none(),
            "choose one checker CA source"
        );
        if let Some(path) = ca_certificate_path {
            absolute(path)?;
        }
    }
    let mut resolved = settings.clone();
    if let Some(operations::Checker::Https {
        ca_certificate_der,
        ca_certificate_path: Some(path),
        ..
    }) = &mut resolved.checker
    {
        use std::io::Read;
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| anyhow::anyhow!("checker CA file unavailable"))?;
        anyhow::ensure!(
            metadata.is_file() && metadata.len() <= 65536,
            "checker CA must be a regular DER file <=64 KiB"
        );
        let file = std::fs::File::open(path)
            .map_err(|_| anyhow::anyhow!("checker CA file unavailable"))?;
        file.take(65537)
            .read_to_end(ca_certificate_der)
            .map_err(|_| anyhow::anyhow!("checker CA file unavailable"))?;
    }
    if let Some(checker) = resolved.flags().checker {
        checker
            .validate()
            .map_err(|_| anyhow::anyhow!("invalid checker configuration or certificate"))?;
    }
    Ok(())
}
