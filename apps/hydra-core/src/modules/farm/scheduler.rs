use super::*;
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::{OptionalExtension, params};
use tokio_util::sync::CancellationToken;
impl Farm {
    pub async fn enqueue(
        &self,
        a: Actor,
        id: String,
        revision: i64,
        key: String,
    ) -> anyhow::Result<String> {
        anyhow::ensure!(
            !key.is_empty() && key.len() <= 128,
            "idempotency key required"
        );
        self.auth
            .db
            .run(move |db| {
                let tx = db.transaction()?;
                let result = enqueue_tx(&tx, &a, &id, revision, &key, false)?;
                tx.commit()?;
                Ok(result)
            })
            .await
    }
    /// A retry is a new persisted attempt using the original immutable version and manifest.
    pub async fn retry(&self, actor: Actor, origin: String, key: String) -> anyhow::Result<String> {
        anyhow::ensure!(
            !key.is_empty() && key.len() <= 128,
            "idempotency key required"
        );
        self.auth.db.run(move |db| {
            let tx = db.transaction()?;
            let request = format!("retry:{origin}:{}", actor.id);
            if let Some((id, old)) = tx.query_row("SELECT id,request_hash FROM farm_runs WHERE idempotency_key=?1", [&key], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))).optional()? {
                anyhow::ensure!(old == request, "idempotency conflict"); return Ok(id);
            }
            let (exploit, version, manifest, targets, status): (String,String,String,String,String) = tx.query_row("SELECT exploit_id,version,manifest,target_ids,status FROM farm_runs WHERE id=?1", [&origin], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            anyhow::ensure!(!["queued", "running"].contains(&status.as_str()), "active run cannot be retried");
            anyhow::ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM farm_exploits WHERE id=?1 AND enabled=1)", [&exploit], |r| r.get::<_,bool>(0))?, "exploit disabled or missing");
            anyhow::ensure!(tx.query_row("SELECT count(*) FROM farm_runs", [], |r| r.get::<_,i64>(0))? < 5000, "run history quota exhausted");
            for id in serde_json::from_str::<Vec<String>>(&targets)? {
                anyhow::ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM farm_targets WHERE id=?1 AND enabled=1)", [id], |r| r.get::<_,bool>(0))?, "target unavailable");
            }
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at,idempotency_key,request_hash,origin_run_id) VALUES(?1,?2,?3,?4,?5,?6,'queued',?7,?8,?9,?10)", params![id,exploit,version,manifest,targets,actor.id,now(),key,request,origin])?;
            audit::append_change(&tx,&actor,"farm.run.retry",&id,&key,true,now(),&serde_json::json!({"origin_run_id":origin}))?;
            tx.commit()?; Ok(id)
        }).await
    }
    async fn enqueue_scheduled(
        &self,
        user: String,
        id: String,
        revision: i64,
        key: String,
    ) -> anyhow::Result<String> {
        self.auth
            .db
            .run(move |db| {
                let tx = db.transaction()?;
                let run = enqueue_tx(&tx, &Actor::user(user), &id, revision, &key, true)?;
                tx.commit()?;
                Ok(run)
            })
            .await
    }
    pub async fn cancel(&self, a: Actor, id: String) -> anyhow::Result<()> {
        let run = id.clone();
        self.auth.db.run(move|db|{let tx=db.transaction()?;let n=tx.execute("UPDATE farm_runs SET status='cancelled',finished_at=?2 WHERE id=?1 AND status IN ('queued','running')",params![id,now()])?;anyhow::ensure!(n==1,"run not cancellable");audit::append(&tx,&a,"farm.run.cancel",&id,"farm",true,now())?;tx.commit()?;Ok(())}).await?;
        if let Some((id, token)) = &*self.active.lock().await
            && id == &run
        {
            token.cancel();
        }
        Ok(())
    }
    pub async fn background(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tokio::select! {_=shutdown.cancelled()=>break,_=interval.tick()=>{if let Err(e)=self.tick(&shutdown).await {tracing::warn!(error=%e,"farm scheduler tick failed");}}}
        }
    }
    async fn tick(&self, shutdown: &CancellationToken) -> anyhow::Result<()> {
        let due=self.auth.db.run(|db| {Ok(db.prepare("SELECT id,revision,initiated_by,next_run FROM farm_exploits WHERE enabled=1 AND interval_seconds IS NOT NULL AND next_run<=?1 ORDER BY next_run LIMIT 64")?.query_map([now()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?)))?.collect::<Result<Vec<_>,_>>()?)}).await?;
        for (id, revision, user, due_at) in due {
            let key = format!("scheduled-{id}-{due_at}");
            if self
                .enqueue_scheduled(user.clone(), id.clone(), revision, key)
                .await
                .is_ok()
            {
                self.auth.db.run(move |db| {let tx=db.transaction()?;tx.execute("UPDATE farm_exploits SET next_run=?2+interval_seconds WHERE id=?1 AND revision=?3 AND next_run=?4",params![id,now(),revision,due_at])?;system_audit(&tx,"farm.schedule.advance",&id,&user,true)?;tx.commit()?;Ok(())}).await?;
            }
        }
        let job=self.auth.db.run(|db|{let tx=db.transaction()?;let row=tx.query_row("SELECT id,exploit_id,version,manifest,target_ids,initiated_by FROM farm_runs WHERE status='queued' ORDER BY created_at,id LIMIT 1",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))).optional()?;let Some((run_id,exploit_id,version,manifest,target_ids,initiated_by))=row else{return Ok(None)};let artifact:Option<String>=tx.query_row("SELECT path FROM farm_versions WHERE exploit_id=?1 AND version=?2",params![exploit_id,version],|r|r.get(0)).optional()?;
 let mut targets=Vec::new();let mut valid=artifact.is_some();for id in serde_json::from_str::<Vec<String>>(&target_ids)? {let t=tx.query_row("SELECT id,name,host,port,transport,enabled,revision FROM farm_targets WHERE id=?1 AND enabled=1",[id],|r|Ok(Target{id:r.get(0)?,name:r.get(1)?,host:r.get(2)?,port:r.get(3)?,transport:r.get(4)?,enabled:r.get(5)?,revision:r.get(6)?})).optional()?;if let Some(t)=t {targets.push(t);}else{valid=false;}}
 if !valid {tx.execute("UPDATE farm_runs SET status='failed',stderr='artifact or enabled target unavailable',finished_at=?2 WHERE id=?1",params![run_id,now()])?;system_audit(&tx,"farm.run.rejected",&run_id,&initiated_by,false)?;tx.commit()?;return Ok(None)}
 tx.execute("UPDATE farm_runs SET status='running',started_at=?2 WHERE id=?1",params![run_id,now()])?;system_audit(&tx,"farm.run.start",&run_id,&initiated_by,true)?;tx.commit()?;Ok(Some(Job{run_id,exploit_id,version,artifact:artifact.unwrap().into(),manifest:serde_json::from_str(&manifest)?,targets,initiated_by}))}).await?;
        if let Some(job) = job {
            let id = job.run_id.clone();
            let actor = Actor::user(job.initiated_by.clone());
            let cancel = shutdown.child_token();
            *self.active.lock().await = Some((id.clone(), cancel.clone()));
            // Cancellation may commit between claiming the row and publishing the token.
            let run = id.clone();
            if self
                .auth
                .db
                .run(move |db| {
                    Ok(db.query_row(
                        "SELECT status='cancelled' FROM farm_runs WHERE id=?1",
                        [run],
                        |r| r.get::<_, bool>(0),
                    )?)
                })
                .await?
            {
                cancel.cancel();
            }

            let timeout = std::time::Duration::from_secs(job.manifest.timeout_seconds + 5);
            let output = tokio::select! {_=shutdown.cancelled()=>{cancel.cancel();Ok(RunOutput{status:"interrupted".into(),stdout:Vec::new(),stderr:b"shutdown interrupted execution".to_vec(),exit_code:None})},value=tokio::time::timeout(timeout,self.runner.run(job,cancel.clone()))=>match value{Ok(v)=>v,Err(_)=>{cancel.cancel();Ok(RunOutput{status:"timeout".into(),stdout:Vec::new(),stderr:b"sandbox runner timeout".to_vec(),exit_code:None})}}};
            *self.active.lock().await = None;
            self.auth.db.run(move|db|{let tx=db.transaction()?;let (status,out,err,code)=match output {Ok(mut o)=>{o.stdout.truncate(65536);o.stderr.truncate(65536);let s=if ["succeeded","failed","timeout","cancelled","interrupted"].contains(&o.status.as_str()){o.status}else{"failed".into()};(s,bounded_text(&o.stdout),bounded_text(&o.stderr),o.exit_code)},Err(e)=>("failed".into(),String::new(),e.to_string().chars().take(1024).collect(),None)};tx.execute("UPDATE farm_runs SET status=?2,stdout=?3,stderr=?4,exit_code=?5,finished_at=?6 WHERE id=?1 AND status='running'",params![id,status,out,err,code,now()])?;system_audit(&tx,"farm.run.finish",&id,&actor.id,true)?;tx.commit()?;Ok(())}).await?;
        }
        Ok(())
    }
}
fn enqueue_tx(
    tx: &rusqlite::Transaction<'_>,
    a: &Actor,
    id: &str,
    revision: i64,
    key: &str,
    scheduled: bool,
) -> anyhow::Result<String> {
    let request = format!("{id}:{revision}:{}:{scheduled}", a.id);
    if let Some((run, old)) = tx
        .query_row(
            "SELECT id,request_hash FROM farm_runs WHERE idempotency_key=?1",
            [key],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    {
        anyhow::ensure!(old == request, "idempotency conflict");
        return Ok(run);
    }
    let(version,manifest,targets)=tx.query_row("SELECT version,manifest,target_ids FROM farm_exploits WHERE id=?1 AND revision=?2 AND enabled=1",params![id,revision],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;
    anyhow::ensure!(
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM farm_versions WHERE exploit_id=?1 AND version=?2)",
            params![id, version],
            |r| r.get::<_, bool>(0)
        )?,
        "version not uploaded"
    );
    for target in serde_json::from_str::<Vec<String>>(&targets)? {
        anyhow::ensure!(
            tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM farm_targets WHERE id=?1 AND enabled=1)",
                [target],
                |r| r.get::<_, bool>(0)
            )?,
            "target unavailable"
        );
    }
    let count: i64 = tx.query_row("SELECT count(*) FROM farm_runs", [], |r| r.get(0))?;
    anyhow::ensure!(count < 5000, "run history/queue quota exhausted");
    let run = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at,idempotency_key,request_hash) VALUES(?1,?2,?3,?4,?5,?6,'queued',?7,?8,?9)",params![run,id,version,manifest,targets,a.id,now(),key,request])?;
    if scheduled {
        system_audit(tx, "farm.run.enqueue", &run, &a.id, true)?;
    } else {
        audit::append(tx, a, "farm.run.enqueue", &run, key, true, now())?;
    }
    Ok(run)
}

fn bounded_text(bytes: &[u8]) -> String {
    let mut value = String::from_utf8_lossy(bytes).into_owned();
    if value.len() > 65536 {
        let mut end = 65500;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.push_str("\n[output truncated]\n");
    }
    value
}

fn system_audit(
    tx: &rusqlite::Transaction<'_>,
    action: &str,
    id: &str,
    initiated: &str,
    success: bool,
) -> anyhow::Result<()> {
    audit::append_change(
        tx,
        &Actor {
            kind: "system".into(),
            id: "farm-scheduler".into(),
        },
        action,
        id,
        id,
        success,
        now(),
        &serde_json::json!({"initiated_by":initiated}),
    )
}
