use super::Flags;
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
#[derive(Serialize, ToSchema)]
pub struct FlagView {
    pub service_id: Option<String>,
    pub state: Option<String>,
    pub id: String,
    pub digest: String,
    pub category: String,
    pub target_id: Option<String>,
    pub run_id: Option<String>,
    pub initiated_by: String,
    pub created_at: String,
    pub triage: Option<String>,
}
#[derive(Serialize, ToSchema)]
pub struct SubmissionView {
    pub id: String,
    pub flag_id: String,
    pub state: String,
    pub attempts: u32,
    pub initiated_by: String,
    pub created_at: String,
    pub response: String,
}
pub(super) fn enqueue(
    tx: &Transaction<'_>,
    id: &str,
    actor: &Actor,
    request: &str,
) -> anyhow::Result<()> {
    let category: String =
        tx.query_row("SELECT category FROM flags WHERE id=?1", [id], |r| r.get(0))?;
    anyhow::ensure!(
        category == "captured" && !tx.query_row("SELECT EXISTS(SELECT 1 FROM flags own JOIN flags f ON own.digest=f.digest WHERE f.id=?1 AND own.category='own_exposed')", [id], |r| r.get::<_,bool>(0))?,
        "only captured flags may be submitted"
    );
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM submissions WHERE state IN ('queued','inflight','retry')",
        [],
        |r| r.get(0),
    )?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM submissions WHERE flag_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    if !exists {
        anyhow::ensure!(count < 5000, "submission queue full");
        tx.execute("INSERT INTO submissions(id,flag_id,next_attempt,created_at,initiated_by) VALUES(?1,?2,?3,?3,?4)",params![uuid::Uuid::new_v4().to_string(),id,now(),actor.id])?;
    }
    audit::append_change(
        tx,
        actor,
        "flags.submit",
        id,
        request,
        true,
        now(),
        &serde_json::json!({"initiated_by":actor.id,"before":{"submission_exists":exists},"after":{"submission_exists":true}}),
    )?;
    Ok(())
}
pub(super) fn quarantine_defensive(
    tx: &Transaction<'_>,
    digest: &str,
    actor: &Actor,
    request: &str,
) -> anyhow::Result<()> {
    let rows=tx.prepare("SELECT s.id,s.state,s.initiated_by FROM submissions s JOIN flags f ON f.id=s.flag_id WHERE f.digest=?1 AND s.state IN ('queued','retry','inflight')")?.query_map([digest],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (id, before, initiated) in rows {
        tx.execute("UPDATE submissions SET state='quarantined',response='defensive finding; submission prohibited' WHERE id=?1",[&id])?;
        audit::append_change(
            tx,
            actor,
            "submissions.quarantine",
            &id,
            request,
            true,
            now(),
            &serde_json::json!({"initiated_by":initiated,"before":{"state":before},"after":{"state":"quarantined"},"reason":"defensive finding"}),
        )?;
    }
    Ok(())
}
impl Flags {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn insert(
        &self,
        value: String,
        category: String,
        target: Option<String>,
        run: Option<String>,
        actor: Actor,
        request: String,
        event: Option<String>,
        auto: bool,
    ) -> anyhow::Result<String> {
        let audit_actor = if run.is_some() && actor.kind == "system" {
            Actor {
                kind: "system".into(),
                id: "farm-runner".into(),
            }
        } else {
            actor.clone()
        };
        anyhow::ensure!(
            category != "own_exposed",
            "defensive flag values are not retained"
        );
        let digest = format!("{:x}", Sha256::digest(value.as_bytes()));
        self.auth.db.run(move|db|{let tx=db.transaction()?;
            if let Some(event)=&event && let Some(id)=tx.query_row("SELECT flag_id FROM flag_events WHERE event_id=?1",[event],|r|r.get::<_,String>(0)).optional()? {return Ok(id)}
            if category=="captured" {anyhow::ensure!(!tx.query_row("SELECT EXISTS(SELECT 1 FROM flags WHERE digest=?1 AND category='own_exposed')",[&digest],|r|r.get::<_,bool>(0))?,"defensive flag cannot become captured");}
            if let Some(target)=&target {anyhow::ensure!(farm_target_enabled(&tx,target,run.as_deref())?,"target not enabled");}
            let existing=tx.query_row("SELECT id FROM flags WHERE digest=?1 AND category=?2",params![digest,category],|r|r.get::<_,String>(0)).optional()?;
            let id=existing.clone().unwrap_or_else(||uuid::Uuid::new_v4().to_string());
            let count:i64=tx.query_row("SELECT count(*) FROM flags",[],|r|r.get(0))?;anyhow::ensure!(count<100000,"flag vault full");
            let service=if let Some(run)=run.as_ref() {
                if has_column(&tx,"farm_runs","service_id")? {tx.query_row("SELECT service_id FROM farm_runs WHERE id=?1",[run],|r|r.get::<_,Option<String>>(0)).optional()?.flatten()}else{None}
            }else{None};
            tx.execute("INSERT OR IGNORE INTO flags(id,digest,category,value,target_id,run_id,initiated_by,created_at,service_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![id,digest,category,value,target,run,actor.id,now(),service])?;
            if let Some(event)=event {let n:i64=tx.query_row("SELECT count(*) FROM flag_events",[],|r|r.get(0))?;anyhow::ensure!(n<1000000,"flag event quota exhausted");tx.execute("INSERT INTO flag_events(event_id,flag_id) VALUES(?1,?2)",params![event,id])?;}
            audit::append_change(&tx,&audit_actor,"flags.record",&id,&request,true,now(),&serde_json::json!({"initiated_by":actor.id,"before":{"existed":existing.is_some()},"after":{"category":category,"target_id":target,"run_id":run}}))?;
            if category=="own_exposed" {quarantine_defensive(&tx,&digest,&audit_actor,&request)?;}
            if auto {enqueue(&tx,&id,&actor,&request)?;}
            tx.commit()?;Ok(id)
        }).await
    }
    pub async fn submit(&self, id: String, actor: Actor, request: String) -> anyhow::Result<()> {
        self.auth
            .db
            .run(move |db| {
                let tx = db.transaction()?;
                enqueue(&tx, &id, &actor, &request)?;
                tx.commit()?;
                Ok(())
            })
            .await
    }
    pub async fn list(&self, actor: Actor, request: String) -> anyhow::Result<Vec<FlagView>> {
        self.list_page(actor, request, Browse::default()).await
    }
    pub async fn submissions(
        &self,
        actor: Actor,
        request: String,
    ) -> anyhow::Result<Vec<SubmissionView>> {
        self.submissions_page(actor, request, Browse::default())
            .await
    }
    pub async fn submissions_page(
        &self,
        actor: Actor,
        request: String,
        browse: Browse,
    ) -> anyhow::Result<Vec<SubmissionView>> {
        browse.validate()?;
        self.auth.db.run(move|db|{let tx=db.transaction()?;let rows=tx.prepare("SELECT id,flag_id,state,attempts,initiated_by,strftime('%Y-%m-%dT%H:%M:%SZ',created_at,'unixepoch'),response FROM submissions WHERE (?1='' OR id LIKE ?1 OR flag_id LIKE ?1 OR state LIKE ?1) ORDER BY created_at DESC,id LIMIT ?2 OFFSET ?3")?.query_map(params![browse.pattern(),browse.limit(),browse.offset],|r|Ok(SubmissionView{id:r.get(0)?,flag_id:r.get(1)?,state:r.get(2)?,attempts:r.get(3)?,initiated_by:r.get(4)?,created_at:r.get(5)?,response:r.get(6)?}))?.collect::<Result<Vec<_>,_>>()?;audit::append(&tx,&actor,"submissions.list","submissions",&request,true,now())?;tx.commit()?;Ok(rows)}).await
    }
}

#[derive(Default, Deserialize, utoipa::IntoParams)]
#[serde(default, deny_unknown_fields)]
pub struct Browse {
    pub service_id: Option<String>,
    pub state: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub offset: u32,
    pub limit: Option<u32>,
    pub search: Option<String>,
}
impl Browse {
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.offset <= 1000000
                && self.limit.unwrap_or(100) > 0
                && self.limit.unwrap_or(100) <= 200
                && self.search.as_ref().is_none_or(|s| s.len() <= 128),
            "invalid pagination"
        );
        anyhow::ensure!(
            self.sort.as_deref().is_none_or(|v| matches!(
                v,
                "created_at" | "service" | "target" | "category" | "state" | "digest"
            )) && self
                .order
                .as_deref()
                .is_none_or(|v| matches!(v, "asc" | "desc"))
                && self.service_id.as_ref().is_none_or(|v| v.len() <= 128)
                && self.state.as_ref().is_none_or(|v| v.len() <= 32),
            "invalid sort or filter"
        );
        Ok(())
    }
    pub(super) fn limit(&self) -> u32 {
        self.limit.unwrap_or(100)
    }
    pub(super) fn pattern(&self) -> String {
        self.search
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{s}%"))
            .unwrap_or_default()
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Triage {
    pub resolution: String,
    pub target_id: Option<String>,
}
impl Flags {
    pub async fn triage(
        &self,
        id: String,
        body: Triage,
        actor: Actor,
        request: String,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            actor.kind == "user"
                && matches!(
                    body.resolution.as_str(),
                    "own_exposed" | "dismissed" | "captured"
                ),
            "invalid triage"
        );
        self.auth.db.run(move|db|{let tx=db.transaction()?;
 let (category,value):(String,String)=tx.query_row("SELECT category,value FROM flags WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?)))?;
 anyhow::ensure!(category=="unknown","only unknown findings may be triaged");
 let before:Option<String>=tx.query_row("SELECT resolution FROM flag_triage WHERE flag_id=?1",[&id],|r|r.get(0)).optional()?;
 anyhow::ensure!(before.is_none(),"finding already triaged");
 if body.resolution=="own_exposed" {
 tx.execute("UPDATE flag_events SET flag_id=NULL WHERE flag_id=?1",[&id])?;
 tx.execute("DELETE FROM submissions WHERE flag_id=?1",[&id])?;
 tx.execute("DELETE FROM flag_triage WHERE flag_id=?1",[&id])?;
 tx.execute("DELETE FROM flags WHERE id=?1",[&id])?;
 audit::append(&tx,&actor,"flags.forget_defensive",&id,&request,true,now())?;
 tx.commit()?;return Ok(())
 }
 if body.resolution=="captured" {let target=body.target_id.as_ref().ok_or_else(||anyhow::anyhow!("authorized target required"))?;
 anyhow::ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM farm_targets WHERE id=?1 AND enabled=1)",[target],|r|r.get::<_,bool>(0))?,"target not enabled");
 anyhow::ensure!(!tx.query_row("SELECT EXISTS(SELECT 1 FROM flags WHERE value=?1 AND category='own_exposed')",[&value],|r|r.get::<_,bool>(0))?,"defensive finding cannot be captured");
 tx.execute("UPDATE flags SET category='captured',target_id=?2 WHERE id=?1",params![id,target])?;
 } else if body.resolution=="own_exposed" {tx.execute("UPDATE flags SET category='own_exposed' WHERE id=?1",[&id])?;let digest:String=tx.query_row("SELECT digest FROM flags WHERE id=?1",[&id],|r|r.get(0))?;quarantine_defensive(&tx,&digest,&actor,&request)?;}
 tx.execute("INSERT INTO flag_triage VALUES(?1,?2,?3,?4,?5,?6)",params![id,body.resolution,body.target_id,actor.id,request,now()])?;
 audit::append_change(&tx,&actor,"flags.triage",&id,&request,true,now(),&serde_json::json!({"before":{"category":category,"triage":before},"after":{"resolution":body.resolution,"target_id":body.target_id}}))?;
 tx.commit()?;Ok(())}).await
    }
}

// Catalog team IDs are accepted only with the immutable run that selected them.
fn farm_target_enabled(
    tx: &Transaction<'_>,
    target: &str,
    run: Option<&str>,
) -> anyhow::Result<bool> {
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM farm_targets WHERE id=?1 AND enabled=1)",
        [target],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(true);
    }
    let Some(run) = run else { return Ok(false) };
    if !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='farm_teams')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(false);
    }
    Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM farm_teams t JOIN farm_runs r ON r.id=?2 JOIN farm_services s ON s.id=r.service_id WHERE t.id=?1 AND t.enabled=1 AND s.enabled=1 AND EXISTS(SELECT 1 FROM json_each(r.team_ids) WHERE value=t.id))",params![target,run],|r|r.get(0))?)
}

pub(super) fn has_column(
    db: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> anyhow::Result<bool> {
    // Table names are constants supplied by this feature, never request input.
    Ok(db
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == column))
}
