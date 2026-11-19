use super::{Checks, Fallback};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub id: String,
    pub services: Vec<String>,
    pub timeout_ms: u64,
    pub on_error: Fallback,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub revision: u64,
    pub policies: Vec<Policy>,
}

#[derive(Default)]
pub(super) struct Applied {
    pub settings: Settings,
    pub services: HashMap<String, (usize, u64, Fallback)>,
}

#[derive(Debug)]
pub(crate) struct StaleRevision;
impl std::fmt::Display for StaleRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request checker settings changed; reload and retry")
    }
}
impl std::error::Error for StaleRevision {}

impl Checks {
    pub(super) fn load_settings(&mut self) -> anyhow::Result<()> {
        let mut settings = Settings {
            revision: 0,
            policies: self
                .checkers
                .iter()
                .map(|entry| Policy {
                    id: entry.policy.id.clone(),
                    services: entry.policy.services.clone(),
                    timeout_ms: entry.policy.timeout_ms,
                    on_error: entry.policy.on_error,
                })
                .collect(),
        };
        if let Some(path) = &self.path {
            match std::fs::symlink_metadata(path) {
                Ok(meta) => {
                    ensure!(
                        meta.is_file() && meta.len() <= 128 * 1024,
                        "invalid request checker settings file"
                    );
                    let saved: Settings = serde_json::from_slice(&std::fs::read(path)?)?;
                    settings.revision = saved.revision;
                    for policy in &mut settings.policies {
                        if let Some(updated) = saved.policies.iter().find(|p| p.id == policy.id) {
                            *policy = updated.clone();
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("read request checker settings"),
            }
        }
        let applied = self.compile(settings)?;
        *self
            .current
            .get_mut()
            .expect("new request checker settings lock") = applied;
        Ok(())
    }

    pub(crate) fn settings(&self) -> Settings {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .settings
            .clone()
    }

    pub(crate) fn configure(&self, mut settings: Settings) -> anyhow::Result<Settings> {
        let _lock = self.update.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(self.path.is_some(), "no request checkers are registered");
        if settings.revision != self.settings().revision {
            return Err(StaleRevision.into());
        }
        settings.revision = settings
            .revision
            .checked_add(1)
            .context("request checker settings revision exhausted")?;
        let applied = self.compile(settings)?;
        let path = self
            .path
            .as_deref()
            .context("no request checker settings path")?;
        save(path, &applied.settings)?;
        let saved = applied.settings.clone();
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = applied;
        Ok(saved)
    }

    fn compile(&self, mut settings: Settings) -> anyhow::Result<Applied> {
        ensure!(
            settings.policies.len() == self.checkers.len(),
            "every configured checker must have one policy"
        );
        settings.policies.sort_by(|a, b| a.id.cmp(&b.id));
        let mut seen = HashSet::new();
        let mut services = HashMap::new();
        for policy in &settings.policies {
            let index = self
                .checkers
                .iter()
                .position(|checker| checker.policy.id == policy.id)
                .context("unknown request checker")?;
            ensure!(seen.insert(&policy.id), "duplicate request checker ID");
            ensure!(
                (25..=1000).contains(&policy.timeout_ms) && policy.services.len() <= 128,
                "request checker limits exceeded"
            );
            for id in &policy.services {
                ensure!(
                    !id.is_empty()
                        && id.len() <= 128
                        && services
                            .insert(id.clone(), (index, policy.timeout_ms, policy.on_error))
                            .is_none(),
                    "invalid service ID or more than one checker assigned to a service"
                );
            }
        }
        Ok(Applied { settings, services })
    }
}

fn save(path: &Path, settings: &Settings) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(settings)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.context("save request checker settings")
}
