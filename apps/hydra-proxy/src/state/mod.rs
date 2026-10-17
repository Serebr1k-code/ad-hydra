use crate::{telemetry::Telemetry, transport::Manager};
use anyhow::{bail, ensure};
use hydra_domain::{RuleSnapshot, ServiceSnapshot};
use hydra_filter_engine::CompiledSnapshot;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, Semaphore},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub actor_type: String,
    pub actor_id: String,
    pub request_id: String,
    pub applied_at_unix: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub snapshot: ServiceSnapshot,
    pub provenance: Provenance,
    #[serde(default)]
    pub rules: RuleSnapshot,
}
struct Current {
    saved: Saved,
    manager: Manager,
}
pub struct State {
    current: Mutex<Current>,
    pub telemetry: Arc<Telemetry>,
    path: PathBuf,
    commits: Arc<Semaphore>,
}
impl State {
    pub async fn start(
        path: PathBuf,
        bootstrap: ServiceSnapshot,
        mut manager: Manager,
        telemetry: Arc<Telemetry>,
    ) -> anyhow::Result<Arc<Self>> {
        let saved = match tokio::fs::metadata(&path).await {
            Ok(meta) => {
                ensure!(meta.len() <= 2 * 1024 * 1024, "saved config exceeds 2 MiB");
                serde_json::from_slice::<Saved>(&tokio::fs::read(&path).await?)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved {
                snapshot: bootstrap,
                rules: RuleSnapshot::default(),
                provenance: Provenance {
                    actor_type: "local_cli".into(),
                    actor_id: nix::unistd::geteuid().to_string(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    applied_at_unix: now(),
                },
            },
            Err(e) => return Err(e.into()),
        };
        saved.snapshot.validate().map_err(anyhow::Error::msg)?;
        let compiled = CompiledSnapshot::compile(saved.rules.clone(), &saved.snapshot.services)?;
        telemetry.replace_rules(compiled);
        let prepared = manager.prepare(&saved.snapshot).await?;
        telemetry.apply_history_services(&saved.snapshot.services);
        persist(&path, &saved).await?;
        manager.activate(&saved.snapshot, prepared).await;
        Ok(Arc::new(Self {
            current: Mutex::new(Current { saved, manager }),
            telemetry,
            path,
            commits: Arc::new(Semaphore::new(16)),
        }))
    }
    pub async fn revision(&self) -> u64 {
        self.current.lock().await.saved.snapshot.revision
    }
    pub async fn rules_revision(&self) -> u64 {
        self.current.lock().await.saved.rules.revision
    }
    pub async fn get_service(&self, id: &str) -> Option<hydra_domain::Service> {
        self.current
            .lock()
            .await
            .saved
            .snapshot
            .services
            .iter()
            .find(|s| s.id == id && s.enabled)
            .cloned()
    }
    pub async fn has_service(&self, id: &str) -> bool {
        self.current
            .lock()
            .await
            .saved
            .snapshot
            .services
            .iter()
            .any(|s| s.id == id)
    }
    pub async fn apply(
        self: &Arc<Self>,
        snapshot: ServiceSnapshot,
        provenance: Provenance,
    ) -> anyhow::Result<u64> {
        snapshot.validate().map_err(anyhow::Error::msg)?;
        // Admission happens before spawning: cancelled callers cannot accumulate
        // an unbounded queue of detached transactions.
        let permit = self
            .commits
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("snapshot commit capacity exhausted"))?;
        let state = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            state.commit(snapshot, provenance).await
        })
        .await?
    }
    async fn commit(
        &self,
        snapshot: ServiceSnapshot,
        provenance: Provenance,
    ) -> anyhow::Result<u64> {
        let mut current = self.current.lock().await;
        let old = &current.saved.snapshot;
        if snapshot.revision == old.revision && snapshot == *old {
            return Ok(old.revision);
        }
        if snapshot.revision <= old.revision {
            bail!("stale or conflicting revision");
        }
        let compiled = CompiledSnapshot::compile(current.saved.rules.clone(), &snapshot.services)?;
        let prepared = current.manager.prepare(&snapshot).await?;
        let saved = Saved {
            snapshot,
            provenance,
            rules: current.saved.rules.clone(),
        };
        // No live state changes before durable replacement; failed preparation drops new sockets.
        persist(&self.path, &saved).await?;
        self.telemetry
            .apply_history_services(&saved.snapshot.services);
        self.telemetry.replace_rules(compiled);
        current.manager.activate(&saved.snapshot, prepared).await;
        current.saved = saved;
        Ok(current.saved.snapshot.revision)
    }
    pub async fn apply_config(
        self: &Arc<Self>,
        snapshot: ServiceSnapshot,
        rules: RuleSnapshot,
        provenance: Provenance,
    ) -> anyhow::Result<u64> {
        snapshot.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            snapshot.revision == rules.revision,
            "combined revisions must agree"
        );
        let permit = self
            .commits
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("snapshot commit capacity exhausted"))?;
        let state = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut current = state.current.lock().await;
            if current.saved.snapshot == snapshot && current.saved.rules == rules {
                return Ok(snapshot.revision);
            }
            ensure!(
                snapshot.revision >= current.saved.snapshot.revision
                    && rules.revision >= current.saved.rules.revision,
                "stale combined revision"
            );
            ensure!(
                snapshot.revision > current.saved.snapshot.revision
                    || current.saved.snapshot == snapshot,
                "conflicting services"
            );
            ensure!(
                rules.revision > current.saved.rules.revision || current.saved.rules == rules,
                "conflicting rules"
            );
            let compiled = CompiledSnapshot::compile(rules.clone(), &snapshot.services)?;
            let prepared = current.manager.prepare(&snapshot).await?;
            let saved = Saved {
                snapshot,
                rules,
                provenance,
            };
            persist(&state.path, &saved).await?;
            state
                .telemetry
                .apply_history_services(&saved.snapshot.services);
            state.telemetry.replace_rules(compiled);
            current.manager.activate(&saved.snapshot, prepared).await;
            current.saved = saved;
            Ok::<_, anyhow::Error>(current.saved.snapshot.revision)
        })
        .await?
    }
    pub async fn apply_rules(
        self: &Arc<Self>,
        rules: RuleSnapshot,
        provenance: Provenance,
    ) -> anyhow::Result<u64> {
        let permit = self
            .commits
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("snapshot commit capacity exhausted"))?;
        let state = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut current = state.current.lock().await;
            let old = &current.saved.rules;
            if rules.revision == old.revision && rules == *old {
                return Ok(old.revision);
            }
            ensure!(
                rules.revision > old.revision,
                "stale or conflicting rule revision"
            );
            let compiled =
                CompiledSnapshot::compile(rules.clone(), &current.saved.snapshot.services)?;
            let saved = Saved {
                snapshot: current.saved.snapshot.clone(),
                provenance,
                rules,
            };
            persist(&state.path, &saved).await?;
            state.telemetry.replace_rules(compiled);
            current.saved = saved;
            Ok::<_, anyhow::Error>(current.saved.rules.revision)
        })
        .await?
    }
    pub async fn stop(&self) {
        // Queue fairly for all permits to drain admitted commits and prevent new
        // transactions from running after listener shutdown.
        let _drained = self.commits.clone().acquire_many_owned(16).await;
        self.commits.close();
        self.current.lock().await.manager.stop().await;
    }
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
async fn persist(path: &Path, saved: &Saved) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(".hydra-{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec(saved)?;
    ensure!(bytes.len() <= 2 * 1024 * 1024, "saved config exceeds 2 MiB");
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, path).await?;
        // Rename is the commit point. A subsequent durability error cannot
        // honestly be returned as rejection: restart already sees this snapshot.
        let durable = async { tokio::fs::File::open(parent).await?.sync_all().await }.await;
        if let Err(error) = durable {
            tracing::warn!(%error, path = %path.display(), "snapshot committed; directory fsync failed, crash durability uncertain");
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

#[cfg(test)]
mod tests;
