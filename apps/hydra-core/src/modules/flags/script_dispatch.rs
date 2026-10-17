//! Durable 30-second dispatch windows; one bounded Python child at a time.
use super::{Flags, script_config::SubmitterConfig};
use crate::{
    infrastructure::submitter_process,
    modules::{
        audit::{self, Actor},
        auth::now,
    },
};
use rusqlite::params;
use tokio_util::sync::CancellationToken;

struct Item {
    id: String,
    value: String,
    attempts: u32,
    initiated: String,
}
fn actor() -> Actor {
    Actor {
        kind: "system".into(),
        id: "submitter".into(),
    }
}
impl Flags {
    pub(super) async fn dispatch_cycle(&self, cancel: CancellationToken) -> anyhow::Result<bool> {
        let Ok(_permit) = self.submission_slot.try_acquire() else {
            return Ok(false);
        };
        let config_guard = self.configuration.lock().await;
        let config = self
            .script
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if config.as_ref().is_some_and(|c| !c.enabled)
            || (config.is_none() && self.settings.checker.is_none())
            || cancel.is_cancelled()
        {
            return Ok(false);
        }
        let claimed = self.auth.db.run(|db| {
            let tx = db.transaction()?;
            let updated = tx.execute("UPDATE flag_submitter_clock SET next_batch_at=?1 WHERE id=1 AND next_batch_at<=?2",params![now()+30,now()])?;
            if updated > 0 { audit::append(&tx,&actor(),"submitter.window","python-submitter",&uuid::Uuid::new_v4().to_string(),true,now())?; }
            tx.commit()?; Ok(updated>0)
        }).await?;
        drop(config_guard);
        if !claimed {
            return Ok(false);
        }
        if let Some(config) = config {
            // Snapshot IDs once. New captures and retries wait for the next window.
            let ids = self.auth.db.run(|db| {
                Ok(db.prepare("SELECT s.id FROM submissions s JOIN flags f ON f.id=s.flag_id WHERE s.state IN ('queued','retry') AND s.next_attempt<=?1 AND s.attempts<10 AND f.category='captured' AND NOT EXISTS(SELECT 1 FROM flags own WHERE own.digest=f.digest AND own.category='own_exposed') ORDER BY s.created_at,s.id LIMIT 5000")?.query_map([now()],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?)
            }).await?;
            for chunk in ids.chunks(240) {
                // At most60KiB of flag payload; each flag is at most256 bytes.
                if cancel.is_cancelled() {
                    break;
                }
                self.script_batch(config.clone(), chunk.to_vec(), cancel.clone())
                    .await?;
            }
        } else {
            // Retain operator-configured legacy adapters, collected in the same window.
            for _ in 0..5000 {
                if cancel.is_cancelled() || !self.legacy_submit_once().await? {
                    break;
                }
            }
        }
        Ok(true)
    }
    async fn script_batch(
        &self,
        config: SubmitterConfig,
        ids: Vec<String>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        // Serialize each claim with configuration changes. A running child finishes,
        // but disabling or editing prevents the next chunk using the old revision.
        let configuration_guard = self.configuration.lock().await;
        if !self
            .script
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|current| current.enabled && current.revision == config.revision)
        {
            return Ok(());
        }
        let batch_id = uuid::Uuid::new_v4().to_string();
        let batch = batch_id.clone();
        let revision = config.revision;
        let items=self.auth.db.run(move|db| {
            let tx=db.transaction()?;
            let mut items=Vec::new();
            for id in ids {
                use rusqlite::OptionalExtension;
                let item=tx.query_row("SELECT s.id,f.value,s.attempts,s.initiated_by FROM submissions s JOIN flags f ON f.id=s.flag_id WHERE s.id=?1 AND s.state IN ('queued','retry') AND s.attempts<10 AND f.category='captured' AND NOT EXISTS(SELECT 1 FROM flags own WHERE own.digest=f.digest AND own.category='own_exposed')",[id],|r|Ok(Item{id:r.get(0)?,value:r.get(1)?,attempts:r.get(2)?,initiated:r.get(3)?})).optional()?;
                if let Some(item)=item {
                    tx.execute("UPDATE submissions SET state='inflight',attempts=attempts+1 WHERE id=?1",[&item.id])?;
                    audit::append_change(&tx,&actor(),"submissions.attempt",&item.id,&batch,true,now(),&serde_json::json!({"batch_id":batch,"initiated_by":item.initiated,"attempts":item.attempts+1}))?;
                    items.push(item);
                }
            }
            if !items.is_empty() {
                tx.execute("INSERT INTO flag_submitter_batches(id,revision,flag_count,status,started_at) VALUES(?1,?2,?3,'running',?4)",params![batch,revision,items.len() as u32,now()])?;
                audit::append(&tx,&actor(),"submitter.batch.start",&batch,&batch,true,now())?;
            }
            tx.commit()?;Ok(items)
        }).await?;
        drop(configuration_guard);
        if items.is_empty() {
            return Ok(());
        }
        let result = if let Some(runtime) = self.settings.script_runtime.clone() {
            submitter_process::run(
                runtime,
                config.source,
                config.python_packages,
                items.iter().map(|i| i.value.clone()).collect(),
                batch_id.clone(),
                std::time::Duration::from_secs(config.timeout_seconds),
                cancel,
            )
            .await
        } else {
            Err(anyhow::anyhow!("host Python runtime is unavailable"))
        };
        let (success, status, code, out, err) = match result {
            Ok(output) => (
                output.success,
                if output.cancelled {
                    "interrupted"
                } else if output.timed_out {
                    "timeout"
                } else if output.success {
                    "accepted"
                } else {
                    "failed"
                },
                output.exit_code,
                output.stdout,
                output.stderr,
            ),
            // Detailed spawn errors can contain paths/argv. Keep diagnostics secret-free.
            Err(_) => (
                false,
                "failed",
                None,
                Vec::new(),
                b"Python submitter unavailable; check the configured uv runtime and packages"
                    .to_vec(),
            ),
        };
        let out = self.redact_script_log(&out).await?;
        let err = self.redact_script_log(&err).await?;
        let out = crate::modules::farm::runtimes::encode_log(out.as_bytes())?;
        let err = crate::modules::farm::runtimes::encode_log(err.as_bytes())?;
        self.auth.db.run(move|db| {
            let tx=db.transaction()?;
            for item in items {
                let state=if success {"accepted"} else if item.attempts+1>=10 {"failed"} else {"retry"};
                let changed=tx.execute("UPDATE submissions SET state=?2,response=?3,next_attempt=?4 WHERE id=?1 AND state='inflight'",params![item.id,state,serde_json::json!({"adapter":"python","batch_id":batch_id,"accepted":success,"exit_code":code}).to_string(),now()+30])?;
                if changed>0 { audit::append_change(&tx,&actor(),"submissions.result",&item.id,&batch_id,success,now(),&serde_json::json!({"initiated_by":item.initiated,"state":state,"attempts":item.attempts+1,"batch_id":batch_id}))?; }
            }
            tx.execute("UPDATE flag_submitter_batches SET status=?2,finished_at=?3,exit_code=?4,stdout_gzip=?5,stderr_gzip=?6 WHERE id=?1",params![batch_id,status,now(),code,out,err])?;
            audit::append(&tx,&actor(),"submitter.batch.finish",&batch_id,&batch_id,success,now())?;
            rotate_logs(&tx,&batch_id)?;
            tx.commit()?;Ok(())
        }).await
    }
    pub(super) async fn rotate_submitter_logs(&self) -> anyhow::Result<()> {
        self.auth
            .db
            .run(|db| {
                let tx = db.transaction()?;
                rotate_logs(&tx, &uuid::Uuid::new_v4().to_string())?;
                tx.commit()?;
                Ok(())
            })
            .await
    }
    async fn redact_script_log(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        let regex = self
            .farm_regex
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(regex) = regex {
            text = regex.replace_all(&text, "[redacted flag]").into_owned();
        }
        let values = self
            .auth
            .db
            .run(|db| {
                Ok(db
                    .prepare("SELECT value FROM flags LIMIT 100000")?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        for value in values {
            text = text.replace(&value, "[redacted flag]");
        }
        if text.len() > 65536 {
            let mut n = 65500;
            while !text.is_char_boundary(n) {
                n -= 1
            }
            text.truncate(n);
            text.push_str("\n[submitter output truncated]\n");
        }
        Ok(text)
    }
}

//64KiB per stream and500 rows bound storage to ~64MiB before gzip.
fn rotate_logs(tx: &rusqlite::Transaction<'_>, request: &str) -> anyhow::Result<()> {
    let removed=tx.execute("DELETE FROM flag_submitter_batches WHERE status!='running' AND (started_at<?1 OR id NOT IN (SELECT id FROM flag_submitter_batches ORDER BY rowid DESC LIMIT 500))",[now()-7*86400])?;
    if removed > 0 {
        audit::append_change(
            tx,
            &actor(),
            "submitter.logs.rotate",
            "submitter-batches",
            request,
            true,
            now(),
            &serde_json::json!({"removed":removed}),
        )?;
    }
    Ok(())
}
