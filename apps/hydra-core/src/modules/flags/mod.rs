mod adapters;
mod http;
mod repository;
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
}
#[derive(Clone)]
pub struct Flags {
    pub auth: AuthState,
    settings: Arc<Settings>,
    regex: Option<Regex>,
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
        Ok(Self {
            auth,
            settings: Arc::new(settings),
            regex,
            submission_slot: Arc::new(tokio::sync::Semaphore::new(1)),
        })
    }
    fn validate(&self, value: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !value.is_empty() && value.len() <= 256,
            "invalid flag length"
        );
        let re = self
            .regex
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("flag pattern not configured"))?;
        anyhow::ensure!(
            re.find(value)
                .is_some_and(|m| m.start() == 0 && m.end() == value.len()),
            "invalid flag"
        );
        Ok(())
    }
    pub fn extract(&self, input: &[u8]) -> anyhow::Result<Vec<String>> {
        anyhow::ensure!(input.len() <= 65536, "scan input too large");
        let text = String::from_utf8_lossy(input);
        Ok(self
            .regex
            .as_ref()
            .map(|r| {
                r.find_iter(&text)
                    .filter(|m| m.len() <= 256 && !m.is_empty())
                    .take(256)
                    .map(|m| m.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default())
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
        self.insert(
            value,
            "captured".into(),
            Some(target_id),
            run_id,
            actor,
            request_id,
            None,
            self.settings.auto_submit_captured,
        )
        .await
    }
    pub async fn ingest_proxy(&self, batch: &hydra_wire::v1::EventBatch) -> anyhow::Result<()> {
        anyhow::ensure!(batch.events.len() <= 256, "batch too large");
        // Each committed event is replay safe; caller ACKs only after all writes succeed.
        for event in &batch.events {
            if let Some(hydra_wire::v1::event::Payload::Flag(f)) = &event.payload {
                self.validate(&f.flag_value)?;
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
