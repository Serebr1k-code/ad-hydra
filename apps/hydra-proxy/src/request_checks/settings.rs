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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManagedManifest {
    pub programs: Vec<ManagedEntry>,
    pub settings: Settings,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct ManagedEntry {
    pub id: String,
    pub file: String,
}

pub(crate) struct ProgramInfo {
    pub id: String,
    pub runtime: String,
    pub managed: bool,
    pub source: String,
}
pub(crate) struct ProgramsSnapshot {
    pub revision: u64,
    pub programs: Vec<ProgramInfo>,
}

#[derive(Default)]
pub(super) struct Applied {
    pub settings: Settings,
    pub services: HashMap<String, (std::sync::Arc<super::Checker>, u64, Fallback)>,
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
    pub(crate) fn programs_snapshot(&self) -> anyhow::Result<ProgramsSnapshot> {
        let _lock = self.update.lock().unwrap_or_else(|e| e.into_inner());
        let checkers = self.checkers.read().unwrap_or_else(|e| e.into_inner());
        let programs = checkers
            .iter()
            .map(|c| {
                Ok(ProgramInfo {
                    id: c.policy.id.clone(),
                    runtime: match c.policy.runtime {
                        super::Runtime::Python => "python",
                        super::Runtime::Wasm => "wasm",
                    }
                    .into(),
                    managed: c.managed,
                    source: if c.managed {
                        std::fs::read_to_string(&c.policy.script)?
                    } else {
                        String::new()
                    },
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let revision = self
            .current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .settings
            .revision;
        Ok(ProgramsSnapshot { revision, programs })
    }

    pub(crate) fn put_program(
        &self,
        revision: u64,
        id: String,
        source: String,
    ) -> anyhow::Result<Settings> {
        let _lock = self.update.lock().unwrap_or_else(|e| e.into_inner());
        super::validate_id(&id)?;
        ensure!(source.len() <= 256 * 1024, "checker source exceeds 256 KiB");
        let existing = self.settings();
        if existing.revision != revision {
            return Err(StaleRevision.into());
        }
        let root = self
            .root
            .as_ref()
            .context("checker directory unavailable")?;
        let managed = root.join("managed");
        let checkers = self.checkers.read().unwrap_or_else(|e| e.into_inner());
        let existing_index = checkers.iter().position(|c| c.policy.id == id);
        if let Some(index) = existing_index {
            ensure!(
                checkers[index].managed,
                "configured checker programs are read-only"
            );
        } else {
            ensure!(checkers.len() < 4, "at most four request checkers");
        }
        let path = managed.join(format!("{id}-{}.py", uuid::Uuid::new_v4()));
        atomic_source(&path, source.as_bytes())?;
        let mut provisional = ProvisionalSource {
            path: path.clone(),
            committed: false,
        };
        let mut programs: Vec<ManagedEntry> = checkers
            .iter()
            .filter(|c| c.managed)
            .map(|c| ManagedEntry {
                id: c.policy.id.clone(),
                file: c
                    .policy
                    .script
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            })
            .collect();
        if let Some(entry) = programs.iter_mut().find(|p| p.id == id) {
            entry.file = path.file_name().unwrap().to_string_lossy().into_owned();
        } else {
            programs.push(ManagedEntry {
                id: id.clone(),
                file: path.file_name().unwrap().to_string_lossy().into_owned(),
            });
        }
        drop(checkers);
        let script = path.canonicalize()?;
        let checker = std::sync::Arc::new(super::Checker {
            policy: super::Config {
                id: id.clone(),
                services: vec![],
                script,
                timeout_ms: 250,
                on_error: Fallback::Block,
                runtime: super::Runtime::Python,
                managed: true,
            },
            state: root.join(&id),
            managed: true,
            retired: std::sync::atomic::AtomicBool::new(false),
            worker: Default::default(),
        });
        super::private_directory(&checker.state)?;
        let mut registry = self
            .checkers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let replaced = if let Some(index) = registry.iter().position(|c| c.policy.id == id) {
            let old = registry[index].clone();
            registry[index] = checker;
            Some(old)
        } else {
            registry.push(checker);
            None
        };
        let mut settings = existing;
        if !settings.policies.iter().any(|p| p.id == id) {
            settings.policies.push(Policy {
                id,
                services: vec![],
                timeout_ms: 250,
                on_error: Fallback::Block,
            });
        }
        settings.revision = settings
            .revision
            .checked_add(1)
            .context("request checker settings revision exhausted")?;
        let applied = self.compile_with(settings, &registry)?;
        save_manifest(
            &root.join("managed.json"),
            &ManagedManifest {
                programs,
                settings: applied.settings.clone(),
            },
        )?;
        provisional.committed = true;
        if let Some(old) = replaced {
            old.retired
                .store(true, std::sync::atomic::Ordering::Release);
        }
        *self.checkers.write().unwrap_or_else(|e| e.into_inner()) = registry;
        let saved = applied.settings.clone();
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = applied;
        Ok(saved)
    }

    pub(crate) fn delete_program(&self, revision: u64, id: &str) -> anyhow::Result<Settings> {
        let _lock = self.update.lock().unwrap_or_else(|e| e.into_inner());
        let mut settings = self.settings();
        if settings.revision != revision {
            return Err(StaleRevision.into());
        }
        let index = self
            .checkers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .position(|c| c.policy.id == id)
            .context("unknown checker")?;
        ensure!(
            self.checkers.read().unwrap_or_else(|e| e.into_inner())[index].managed,
            "configured checker programs are read-only"
        );
        settings.policies.retain(|p| p.id != id);
        settings.revision = settings
            .revision
            .checked_add(1)
            .context("request checker settings revision exhausted")?;
        let checkers = self.checkers.read().unwrap_or_else(|e| e.into_inner());
        let kept: Vec<_> = checkers
            .iter()
            .filter(|c| c.policy.id != id)
            .cloned()
            .collect();
        let deleted = checkers[index].clone();
        drop(checkers);
        let root = self.root.as_ref().unwrap();
        let programs: Vec<ManagedEntry> = kept
            .iter()
            .filter(|c| c.managed)
            .map(|c| ManagedEntry {
                id: c.policy.id.clone(),
                file: c
                    .policy
                    .script
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            })
            .collect();
        let applied = self.compile_with(settings, &kept)?;
        save_manifest(
            &root.join("managed.json"),
            &ManagedManifest {
                programs,
                settings: applied.settings.clone(),
            },
        )?;
        deleted
            .retired
            .store(true, std::sync::atomic::Ordering::Release);
        *self.checkers.write().unwrap_or_else(|e| e.into_inner()) = kept;
        let saved = applied.settings.clone();
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = applied;
        Ok(saved)
    }

    pub(super) fn load_settings(&mut self) -> anyhow::Result<()> {
        let mut settings = Settings {
            revision: 0,
            policies: self
                .checkers
                .get_mut()
                .unwrap()
                .iter()
                .map(|entry| Policy {
                    id: entry.policy.id.clone(),
                    services: entry.policy.services.clone(),
                    timeout_ms: entry.policy.timeout_ms,
                    on_error: entry.policy.on_error,
                })
                .collect(),
        };
        let manifest_path = self.root.as_ref().map(|root| root.join("managed.json"));
        if let Some(path) = manifest_path.as_ref().filter(|path| path.exists()) {
            let metadata = path.symlink_metadata()?;
            ensure!(
                metadata.is_file() && metadata.len() <= 2 * 1024 * 1024,
                "invalid managed checker manifest"
            );
            let manifest: ManagedManifest = serde_json::from_slice(&std::fs::read(path)?)?;
            settings.revision = manifest.settings.revision;
            for policy in &mut settings.policies {
                if let Some(updated) = manifest
                    .settings
                    .policies
                    .iter()
                    .find(|saved| saved.id == policy.id)
                {
                    *policy = updated.clone();
                }
            }
        } else if let Some(path) = &self.path {
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
        if settings.revision != self.settings().revision {
            return Err(StaleRevision.into());
        }
        settings.revision = settings
            .revision
            .checked_add(1)
            .context("request checker settings revision exhausted")?;
        let applied = self.compile(settings)?;
        let programs = self
            .checkers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|c| c.managed)
            .map(|c| ManagedEntry {
                id: c.policy.id.clone(),
                file: c
                    .policy
                    .script
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            })
            .collect();
        save_manifest(
            &self.root.as_ref().unwrap().join("managed.json"),
            &ManagedManifest {
                programs,
                settings: applied.settings.clone(),
            },
        )?;
        let saved = applied.settings.clone();
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = applied;
        Ok(saved)
    }

    fn compile(&self, settings: Settings) -> anyhow::Result<Applied> {
        let checkers = self.checkers.read().unwrap_or_else(|e| e.into_inner());
        self.compile_with(settings, &checkers)
    }

    fn compile_with(
        &self,
        mut settings: Settings,
        checkers: &[std::sync::Arc<super::Checker>],
    ) -> anyhow::Result<Applied> {
        ensure!(
            settings.policies.len() == checkers.len(),
            "every configured checker must have one policy"
        );
        settings.policies.sort_by(|a, b| a.id.cmp(&b.id));
        let mut seen = HashSet::new();
        let mut services = HashMap::new();
        for policy in &settings.policies {
            let checker = checkers
                .iter()
                .find(|checker| checker.policy.id == policy.id)
                .cloned()
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
                            .insert(
                                id.clone(),
                                (checker.clone(), policy.timeout_ms, policy.on_error)
                            )
                            .is_none(),
                    "invalid service ID or more than one checker assigned to a service"
                );
            }
        }
        Ok(Applied { settings, services })
    }
}

fn atomic_source(path: &Path, source: &[u8]) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        f.write_all(source)?;
        f.sync_all()?;
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

struct ProvisionalSource {
    path: std::path::PathBuf,
    committed: bool,
}
impl Drop for ProvisionalSource {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
fn save_manifest(path: &Path, manifest: &ManagedManifest) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(manifest)?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
