mod adapters;
mod browse;
mod http;
mod imports;
mod manual;
mod repository;
mod script_config;
mod script_dispatch;
mod script_http;
mod submitter;
use crate::modules::{audit::Actor, auth::AuthState};
pub use adapters::{Checker, CheckerAdapter, CheckerReply};
pub use http::{FlagsApi, router};
use regex::Regex;
pub use repository::{Browse, Triage};
use std::sync::Arc;
#[derive(Clone, Default)]
pub struct Settings {
    pub pattern: Option<String>,
    pub auto_submit_captured: bool,
    pub checker: Option<Checker>,
    pub script_runtime: Option<crate::infrastructure::operations::HostRuntimeSettings>,
}
#[derive(Clone)]
pub struct Flags {
    pub auth: AuthState,
    settings: Arc<Settings>,
    regex: Option<Regex>,
    farm_regex: Arc<std::sync::RwLock<Option<Regex>>>,
    script: Arc<std::sync::RwLock<Option<script_config::SubmitterConfig>>>,
    configuration: Arc<tokio::sync::Mutex<()>>,
    submission_slot: Arc<tokio::sync::Semaphore>,
}
impl Flags {
    pub async fn open(auth: AuthState, settings: Settings) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !settings.auto_submit_captured || settings.checker.is_some(),
            "automatic submission requires configured checker"
        );
        let regex = settings
            .pattern
            .as_ref()
            .map(|p| {
                anyhow::ensure!(p.len() <= 4096, "flag pattern too large");
                let regex = regex::RegexBuilder::new(p)
                    .size_limit(1024 * 1024)
                    .dfa_size_limit(1024 * 1024)
                    .build()?;
                anyhow::ensure!(!regex.is_match(""), "flag regex must not match empty input");
                Ok(regex)
            })
            .transpose()?;
        if let Some(checker) = &settings.checker {
            checker.validate()?;
        }
        auth.db
            .migrate(7, include_str!("../../../../../migrations/0007_flags.sql"))
            .await?;
        auth.db
            .migrate(
                11,
                include_str!("../../../../../migrations/0011_flag_triage.sql"),
            )
            .await?;
        auth.db
            .migrate(
                17,
                include_str!("../../../../../migrations/0017_python_submitter.sql"),
            )
            .await?;
        auth.db
            .run(|db| {
                db.pragma_update(None, "secure_delete", true)?;
                Ok(())
            })
            .await?;
        auth.db
            .migrate(
                18,
                include_str!("../../../../../migrations/0018_flag_submission_controls.sql"),
            )
            .await?;
        auth.db.run(|db| {
            if repository::has_column(db,"farm_runs","service_id")? {
                db.execute("UPDATE flags SET service_id=(SELECT service_id FROM farm_runs WHERE id=flags.run_id) WHERE service_id IS NULL AND run_id IS NOT NULL",[])?;
            }
            Ok(())
        }).await?;
        auth.db
            .run(|db| {
                db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
                Ok(())
            })
            .await?;
        let script = auth.db.run(|db| script_config::load(db)).await?;
        let farm_regex = script
            .as_ref()
            .map(|s| s.validate())
            .transpose()?
            .or_else(|| regex.clone());
        Ok(Self {
            auth,
            settings: Arc::new(settings),
            regex,
            farm_regex: Arc::new(std::sync::RwLock::new(farm_regex)),
            script: Arc::new(std::sync::RwLock::new(script)),
            configuration: Arc::new(tokio::sync::Mutex::new(())),
            submission_slot: Arc::new(tokio::sync::Semaphore::new(1)),
        })
    }
    fn validate(&self, value: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !value.is_empty() && value.len() <= 256 && !value.contains('\0'),
            "invalid flag length"
        );
        let regex = self.farm_regex.read().unwrap_or_else(|e| e.into_inner());
        Self::validate_with(regex.as_ref(), value)
    }
    fn validate_with(regex: Option<&Regex>, value: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !value.is_empty() && value.len() <= 256 && !value.contains('\0'),
            "invalid flag length"
        );
        let re = regex.ok_or_else(|| anyhow::anyhow!("flag pattern not configured"))?;
        anyhow::ensure!(
            re.find(value)
                .is_some_and(|m| m.start() == 0 && m.end() == value.len()),
            "invalid flag"
        );
        Ok(())
    }
    pub fn extract(&self, input: &[u8]) -> anyhow::Result<Vec<String>> {
        anyhow::ensure!(input.len() <= 65536, "scan input too large");
        let regex = self.farm_regex.read().unwrap_or_else(|e| e.into_inner());
        let Some(regex) = regex.as_ref() else {
            return Ok(Vec::new());
        };
        // Byte matching avoids replacement/normalization of malformed surrounding output.
        let regex = regex::bytes::RegexBuilder::new(regex.as_str())
            .size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .build()?;
        Ok(regex
            .find_iter(input)
            .filter(|m| !m.is_empty() && m.len() <= 256)
            .take(256)
            .filter_map(|m| std::str::from_utf8(m.as_bytes()).ok().map(str::to_owned))
            .collect())
    }

    pub async fn record_captured(
        &self,
        value: String,
        target_id: String,
        run_id: Option<String>,
        actor: Actor,
        request_id: String,
    ) -> anyhow::Result<String> {
        self.validate(&value)?;
        let auto = true;
        self.insert(
            value,
            "captured".into(),
            Some(target_id),
            run_id,
            actor,
            request_id,
            None,
            auto,
        )
        .await
    }
    pub async fn ingest_proxy(&self, batch: &hydra_wire::v1::EventBatch) -> anyhow::Result<()> {
        anyhow::ensure!(batch.events.len() <= 256, "batch too large");
        // Each committed event is replay safe; caller ACKs only after all writes succeed.
        for event in &batch.events {
            if let Some(hydra_wire::v1::event::Payload::Flag(f)) = &event.payload {
                if f.source_kind == "own_exposed" {
                    let key = format!("{}:{}", batch.proxy_epoch_id, event.event_id);
                    anyhow::ensure!(
                        key.len() <= 512 && !event.event_id.is_empty(),
                        "invalid event identity"
                    );
                    let flow = event.flow_id.clone();
                    let service = event.service_id.clone();
                    self.auth.db.run(move|db| { let tx=db.transaction()?;
                        if tx.query_row("SELECT count(*) FROM flag_events",[],|r|r.get::<_,i64>(0))? >= 1000000 {
                            tx.execute("DELETE FROM flag_events WHERE rowid IN (SELECT rowid FROM flag_events ORDER BY rowid LIMIT 1000)",[])?;
                        }
                        tx.execute("INSERT OR IGNORE INTO flag_events(event_id,flag_id,flow_id,service_id) VALUES(?1,NULL,?2,?3)",rusqlite::params![key,flow,service])?;
                        tx.commit()?;Ok(()) }).await?;
                    continue;
                }
                Self::validate_with(self.regex.as_ref(), &f.flag_value)?;
                let category = if f.source_kind == "own_exposed" {
                    "own_exposed"
                } else {
                    "unknown"
                };
                let key = format!("{}:{}", batch.proxy_epoch_id, event.event_id);
                anyhow::ensure!(
                    key.len() <= 512 && !event.event_id.is_empty(),
                    "invalid event identity"
                );
                self.insert(
                    f.flag_value.clone(),
                    category.into(),
                    None,
                    None,
                    Actor {
                        kind: "system".into(),
                        id: "proxy".into(),
                    },
                    key.clone(),
                    Some(key),
                    false,
                )
                .await?;
                let flow = event.flow_id.clone();
                let service = event.service_id.clone();
                let key = format!("{}:{}", batch.proxy_epoch_id, event.event_id);
                self.auth
                    .db
                    .run(move |db| {
                        db.execute(
                            "UPDATE flag_events SET flow_id=?2,service_id=?3 WHERE event_id=?1",
                            rusqlite::params![key, flow, service],
                        )?;
                        Ok(())
                    })
                    .await?;
            }
        }
        Ok(())
    }
    pub async fn record_farm_captured(
        &self,
        value: String,
        target_id: String,
        run_id: String,
        initiated_by: String,
    ) -> anyhow::Result<String> {
        let run = run_id.clone();
        let target = target_id.clone();
        let initiated = initiated_by.clone();
        self.auth
            .db
            .run(move |db| {
                let (targets, owner): (String, String) = db.query_row(
                    "SELECT target_ids,initiated_by FROM farm_runs WHERE id=?1",
                    [run],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let targets: Vec<String> = serde_json::from_str(&targets)?;
                anyhow::ensure!(
                    targets.contains(&target) && owner == initiated,
                    "run provenance mismatch"
                );
                Ok(())
            })
            .await?;
        self.record_captured(
            value,
            target_id,
            Some(run_id),
            Actor {
                kind: "system".into(),
                id: initiated_by,
            },
            uuid::Uuid::new_v4().to_string(),
        )
        .await
    }
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod script_tests;
