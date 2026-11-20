//! Durable immediate/manual dispatch; one bounded Python child at a time.
use super::{
    Flags, forcad,
    script_config::{SubmitterConfig, encode_compressed},
};
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
        let manual = self.next_manual().await?;
        if cancel.is_cancelled()
            || (manual.is_none()
                && (config.as_ref().is_some_and(|c| !c.enabled)
                    || (config.is_none() && self.settings.checker.is_none())))
        {
            return Ok(false);
        }
        let request = manual.clone();
        let claimed = self.auth.db.run(move|db| {
            let tx = db.transaction()?;
            let updated=if let Some(request)=request {
                tx.execute("UPDATE flag_submitter_requests SET status='running' WHERE id=?1 AND status='pending'",[&request.id])?
            }else{
                tx.execute("UPDATE flag_submitter_clock SET next_batch_at=?1 WHERE id=1 AND next_batch_at<=?2 AND EXISTS(SELECT 1 FROM submissions WHERE state IN ('queued','retry') AND next_attempt<=?2 AND attempts<10)",params![now()+1,now()])?
            };
            if updated>0 {audit::append(&tx,&actor(),"submitter.dispatch","python-submitter",&uuid::Uuid::new_v4().to_string(),true,now())?;}
            tx.commit()?; Ok(updated>0)
        }).await?;
        drop(config_guard);
        if !claimed {
            return Ok(false);
        }
        if let Some(config) = config {
            let limit = manual
                .as_ref()
                .map_or(config.batch_size, |request| request.count);
            let forced = manual.is_some();
            let ids = self.auth.db.run(move|db| {
                Ok(db.prepare("SELECT s.id FROM submissions s JOIN flags f ON f.id=s.flag_id WHERE s.state IN ('queued','retry') AND (?2 OR s.next_attempt<=?1) AND s.attempts<10 AND f.category='captured' ORDER BY s.created_at,s.rowid LIMIT ?3")?.query_map(params![now(),forced,limit],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?)
            }).await?;
            let mut attempted = 0;
            let effective_batch_size = if config.mode == "forcad" {
                config.batch_size.min(100)
            } else {
                config.batch_size
            };
            for chunk in ids.chunks(effective_batch_size as usize) {
                if cancel.is_cancelled() {
                    break;
                }
                attempted += self
                    .script_batch(
                        config.clone(),
                        chunk.to_vec(),
                        cancel.clone(),
                        manual.clone(),
                    )
                    .await?;
            }
            if let Some(request) = manual {
                self.finish_manual(request, attempted).await?;
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
        manual: Option<super::manual::RunRequest>,
    ) -> anyhow::Result<u32> {
        // Serialize each claim with configuration changes. A running child finishes,
        // but disabling or editing prevents the next chunk using the old revision.
        let configuration_guard = self.configuration.lock().await;
        if !self
            .script
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|current| {
                (current.enabled || manual.is_some()) && current.revision == config.revision
            })
        {
            return Ok(0);
        }
        let initiator = manual
            .as_ref()
            .map(|request| Actor::user(request.actor_id.clone()))
            .unwrap_or_else(actor);
        let audit_actor = initiator.clone();
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
                    audit::append_change(&tx,&audit_actor,"submissions.attempt",&item.id,&batch,true,now(),&serde_json::json!({"batch_id":batch,"initiated_by":item.initiated,"attempts":item.attempts+1}))?;
                    items.push(item);
                }
            }
            if !items.is_empty() {
                let input = serde_json::to_vec(&items.iter().map(|item| &item.value).collect::<Vec<_>>())?;
                tx.execute("INSERT INTO flag_submitter_batches(id,revision,flag_count,status,started_at,input_gzip) VALUES(?1,?2,?3,'running',?4,?5)",params![batch,revision,items.len() as u32,now(),encode_compressed(&input)?])?;
                audit::append(&tx,&audit_actor,"submitter.batch.start",&batch,&batch,true,now())?;
            }
            tx.commit()?;Ok(items)
        }).await?;
        drop(configuration_guard);
        if items.is_empty() {
            return Ok(0);
        }
        let flag_count = items.len() as u32;
        let forcad_mode = config.mode == "forcad";
        let result = if let Some(runtime) = self.settings.script_runtime.clone() {
            let flags = items.iter().map(|i| i.value.clone()).collect();
            let timeout = std::time::Duration::from_secs(config.timeout_seconds);
            if config.mode == "forcad" {
                submitter_process::run_forcad(
                    runtime,
                    flags,
                    batch_id.clone(),
                    timeout,
                    cancel,
                    submitter_process::ForcadRuntimeConfig {
                        ip: config.forcad_ip.clone(),
                        port: config.forcad_port,
                        team_token: config.team_token.clone(),
                    },
                )
                .await
            } else {
                submitter_process::run(
                    runtime,
                    config.source,
                    config.python_packages,
                    flags,
                    batch_id.clone(),
                    timeout,
                    cancel,
                )
                .await
            }
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
        let forcad_results = if forcad_mode {
            parse_forcad_results(&out, items.len())
        } else {
            None
        };
        let per_flag = forcad_results.clone();
        let mut batch_success = success;
        let mut status = status;
        if let Some(results) = &forcad_results {
            let complete = results.iter().all(|v| {
                matches!(
                    v.as_str(),
                    "accepted" | "already_stolen" | "own" | "invalid"
                )
            });
            let all_accepted = results
                .iter()
                .all(|v| matches!(v.as_str(), "accepted" | "already_stolen"));
            batch_success = complete && all_accepted;
            status = if !complete {
                "retry"
            } else if all_accepted {
                "accepted"
            } else {
                "completed_with_rejections"
            };
        } else if forcad_mode {
            batch_success = false;
            status = "retry";
        }
        let out = encode_compressed(&out)?;
        let err = encode_compressed(&err)?;
        let audit_actor = initiator;
        self.auth.db.run(move|db| {
            let tx=db.transaction()?;
            for (index, item) in items.into_iter().enumerate() {
                let verdict = per_flag.as_ref().and_then(|results| results.get(index)).map(String::as_str);
                let (accepted, state) = if per_flag.is_some() {
                    match verdict {
                        Some("accepted" | "already_stolen") => (true, "accepted"),
                        Some("own" | "invalid") => (false, "failed"),
                        _ => (false, if item.attempts+1>=10 {"failed"} else {"retry"}),
                    }
                } else if forcad_mode { (false, if item.attempts+1>=10 {"failed"} else {"retry"}) }
                  else if success { (true, "accepted") }
                  else { (false, if item.attempts+1>=10 {"failed"} else {"retry"}) };
                let changed=tx.execute("UPDATE submissions SET state=?2,response=?3,next_attempt=?4 WHERE id=?1 AND state='inflight'",params![item.id,state,serde_json::json!({"adapter":if per_flag.is_some(){"forcad"}else{"python"},"batch_id":batch_id,"accepted":accepted,"verdict":verdict,"exit_code":code}).to_string(),now()+30])?;
                if changed>0 { audit::append_change(&tx,&audit_actor,"submissions.result",&item.id,&batch_id,accepted,now(),&serde_json::json!({"initiated_by":item.initiated,"state":state,"attempts":item.attempts+1,"batch_id":batch_id}))?; }
            }
            tx.execute("UPDATE flag_submitter_batches SET status=?2,finished_at=?3,exit_code=?4,stdout_gzip=?5,stderr_gzip=?6 WHERE id=?1",params![batch_id,status,now(),code,out,err])?;
            audit::append(&tx,&audit_actor,"submitter.batch.finish",&batch_id,&batch_id,batch_success,now())?;
            rotate_logs(&tx,&batch_id)?;
            tx.commit()?;Ok(flag_count)
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
}

fn parse_forcad_results(stdout: &[u8], expected: usize) -> Option<Vec<String>> {
    forcad::parse_results(stdout, expected)
}

// All retained gzip blobs share a 64 MiB, 1000 batch, seven day budget.
fn rotate_logs(tx: &rusqlite::Transaction<'_>, request: &str) -> anyhow::Result<()> {
    let removed=tx.execute("DELETE FROM flag_submitter_batches WHERE status!='running' AND (started_at<?1 OR id IN (SELECT id FROM (SELECT id,row_number() OVER (ORDER BY rowid DESC) AS position,sum(coalesce(length(input_gzip),0)+coalesce(length(stdout_gzip),0)+coalesce(length(stderr_gzip),0)) OVER (ORDER BY rowid DESC ROWS UNBOUNDED PRECEDING) AS bytes FROM flag_submitter_batches) WHERE position>1000 OR bytes>67108864))",[now()-7*86400])?;
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
