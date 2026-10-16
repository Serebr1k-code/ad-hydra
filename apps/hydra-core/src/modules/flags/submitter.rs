use super::{CheckerAdapter, Flags};
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::params;
impl Flags {
    /// Run exactly one task per Core instance. Durable claims precede external effects.
    pub async fn run_submitter(
        self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
        self.auth.db.run(|db|{let tx=db.transaction()?;
        let defensive=tx.prepare("SELECT DISTINCT f.digest FROM flags f JOIN submissions s ON s.flag_id=f.id JOIN flags own ON own.digest=f.digest AND own.category='own_exposed' WHERE s.state IN ('queued','retry','inflight') LIMIT 5000")?.query_map([],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        for digest in defensive {super::repository::quarantine_defensive(&tx,&digest,&Actor{kind:"system".into(),id:"submitter".into()},&uuid::Uuid::new_v4().to_string())?;}
        let rows=tx.prepare("SELECT id,attempts,initiated_by FROM submissions WHERE state='inflight' LIMIT 5000")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        for (id,attempts,initiated) in rows {let state=if attempts>=10 {"failed"}else{"retry"};tx.execute("UPDATE submissions SET state=?2 WHERE id=?1",params![id,state])?;
        audit::append_change(&tx,&Actor{kind:"system".into(),id:"submitter".into()},"submissions.recover",&id,&uuid::Uuid::new_v4().to_string(),true,now(),&serde_json::json!({"initiated_by":initiated,"before":{"state":"inflight","attempts":attempts},"after":{"state":state,"attempts":attempts}}))?;}
        tx.commit()?;Ok(())}).await?;
        loop {
            tokio::select! { _=cancel.cancelled()=>return Ok(()), _=tokio::time::sleep(std::time::Duration::from_millis(250))=>{} }
            if self.settings.checker.is_none() {
                continue;
            }
            if self.submit_once().await.is_err() {
                tracing::warn!("submission persistence operation failed");
            }
        }
    }
    pub async fn submit_once(&self) -> anyhow::Result<bool> {
        let Ok(_permit) = self.submission_slot.try_acquire() else {
            return Ok(false);
        };
        let Some(checker) = self.settings.checker.as_ref() else {
            return Ok(false);
        };
        let batch_size = checker.batch_size();
        let items=self.auth.db.run(move|db|{let tx=db.transaction()?;
        let items=tx.prepare("SELECT s.id,f.value,s.attempts,s.initiated_by FROM submissions s JOIN flags f ON f.id=s.flag_id WHERE s.state IN ('queued','retry') AND f.category='captured' AND NOT EXISTS(SELECT 1 FROM flags own WHERE own.digest=f.digest AND own.category='own_exposed') AND s.attempts<10 AND s.next_attempt<=?1 ORDER BY s.created_at,s.id LIMIT ?2")?.query_map(params![now(),batch_size as u32],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u32>(2)?,r.get::<_,String>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
        for (id,_,attempts,initiated) in &items {tx.execute("UPDATE submissions SET state='inflight',attempts=attempts+1 WHERE id=?1",[id])?;audit::append_change(&tx,&Actor{kind:"system".into(),id:"submitter".into()},"submissions.attempt",id,id,true,now(),&serde_json::json!({"initiated_by":initiated,"before":{"state":if *attempts==0 {"queued"}else{"retry"},"attempts":attempts},"after":{"state":"inflight","attempts":attempts+1}}))?;}tx.commit()?;Ok(items)}).await?;
        if items.is_empty() {
            return Ok(false);
        }
        let payload = items
            .iter()
            .map(|(id, value, _, _)| (id.clone(), value.clone()))
            .collect::<Vec<_>>();
        let reply = checker.submit_batch(&payload).await;
        let success = reply.as_ref().is_ok_and(|r| r.accepted);
        let mut response = reply
            .map(|r| r.response)
            .unwrap_or_else(|_| "checker unavailable or invalid response".into());
        for value in self.extract(response.as_bytes())? {
            response = response.replace(&value, "[redacted flag]");
        }
        // Also redact other configured flag values that a checker may echo.
        let vault = self
            .auth
            .db
            .run(|db| {
                Ok(db
                    .prepare("SELECT value FROM flags LIMIT 100000")?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        for value in vault {
            response = response.replace(&value, "[redacted flag]");
        }
        let response = serde_json::to_string(
            &serde_json::json!({"adapter":checker.kind(),"accepted":success,"reply":response}),
        )?;
        for (id, _value, attempts, initiated) in items {
            let response = response.clone();
            self.auth
            .db
            .run(move |db| {
                let tx = db.transaction()?;
                let state = if success {
                    "accepted"
                } else if attempts + 1 >= 10 {
                    "failed"
                } else {
                    "retry"
                };
                let updated=tx.execute(
                    "UPDATE submissions SET state=?2,response=?3,next_attempt=?4 WHERE id=?1 AND state='inflight'",
                    params![
                        id,
                        state,
                        response,
                        now() + ((1_i64 << (attempts + 1).min(10)).min(3600))
                    ],
                )?;
                if updated==0 {tx.commit()?;return Ok(())}
                audit::append_change(
                    &tx,
                    &Actor {
                        kind: "system".into(),
                        id: "submitter".into(),
                    },
                    "submissions.result",
                    &id,
                    &id,
                    success,
                    now(),
                    &serde_json::json!({"initiated_by":initiated,"before":{"state":"inflight","attempts":attempts+1},"after":{"state":state,"attempts":attempts+1}}),
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        }
        Ok(true)
    }
}
