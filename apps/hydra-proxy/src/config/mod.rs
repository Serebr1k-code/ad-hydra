use anyhow::{Context, ensure};
use hydra_domain::ServiceSnapshot;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_flows: usize,
    pub udp_idle_ms: u64,
    pub tcp_idle_ms: u64,
    pub connect_timeout_ms: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_flows: 1024,
            udp_idle_ms: 30_000,
            tcp_idle_ms: 300_000,
            connect_timeout_ms: 3000,
        }
    }
}
impl Limits {
    pub fn udp_idle(&self) -> Duration {
        Duration::from_millis(self.udp_idle_ms)
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_flag_pattern")]
    pub flag_pattern: Option<String>,
    #[serde(default)]
    pub(crate) request_checkers: Vec<crate::request_checks::Config>,
    #[serde(default = "socket_mode")]
    pub socket_mode: u32,
    pub socket: PathBuf,
    pub state_file: PathBuf,
    #[serde(default)]
    pub allowed_core_uid: Option<u32>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub snapshot: ServiceSnapshot,
}

fn default_flag_pattern() -> Option<String> {
    Some("[A-Z0-9]{31}=".into())
}
fn socket_mode() -> u32 {
    0o600
}
impl Config {
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        ensure!(
            tokio::fs::metadata(path).await?.len() <= 256 * 1024,
            "config exceeds 256 KiB"
        );
        let cfg: Self = serde_json::from_slice(&tokio::fs::read(path).await?)
            .context("invalid proxy config")?;
        cfg.snapshot.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            matches!(cfg.socket_mode, 0o600 | 0o660),
            "socket_mode must be 384 (0600) or 432 (0660)"
        );
        ensure!(
            (1..=8192).contains(&cfg.limits.max_flows),
            "max_flows must be 1..8192"
        );
        for v in [
            cfg.limits.udp_idle_ms,
            cfg.limits.tcp_idle_ms,
            cfg.limits.connect_timeout_ms,
        ] {
            ensure!(
                (50..=3_600_000).contains(&v),
                "timeouts must be 50..3600000 ms"
            );
        }
        ensure!(
            cfg.socket != cfg.state_file,
            "socket and state path must differ"
        );
        Ok(cfg)
    }
}
