use super::Flags;
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RunInput {
    pub count: u32,
}
#[derive(Clone, Serialize, ToSchema)]
pub struct RunRequest {
    pub id: String,
    pub count: u32,
    pub actor_id: String,
    pub status: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub flag_count: u32,
}
impl Flags {
    pub async fn request_manual_run(&self, actor: Actor, count: u32) -> anyhow::Result<RunRequest> {
        anyhow::ensure!((1..=5000).contains(&count), "choose 1..5000 flags");
        let _guard = self.configuration.lock().await;
        let config = self
            .script
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| anyhow::anyhow!("save a Python submitter first"))?;
        anyhow::ensure!(
            !config.source.trim().is_empty() && self.settings.script_runtime.is_some(),
            "save Python source and configure the uv runtime first"
        );
        let request = RunRequest {
            id: uuid::Uuid::new_v4().to_string(),
            count,
            actor_id: actor.id.clone(),
            status: "pending".into(),
            created_at: now(),
            finished_at: None,
            flag_count: 0,
        };
        let result = request.clone();
        self.auth.db.run(move|db| {let tx=db.transaction()?;
            anyhow::ensure!(!tx.query_row("SELECT EXISTS(SELECT 1 FROM flag_submitter_requests WHERE status IN ('pending','running'))",[],|r|r.get::<_,bool>(0))?,"a manual submission is already pending or running");
            tx.execute("INSERT INTO flag_submitter_requests(id,count,actor_id,status,created_at) VALUES(?1,?2,?3,'pending',?4)",params![request.id,count,actor.id,request.created_at])?;
            audit::append_change(&tx,&actor,"submitter.run.request",&request.id,&request.id,true,now(),&serde_json::json!({"count":count,"configuration_revision":config.revision}))?;
            tx.commit()?;Ok(())}).await?;
        Ok(result)
    }
    pub(super) async fn next_manual(&self) -> anyhow::Result<Option<RunRequest>> {
        self.auth.db.run(|db| Ok(db.query_row("SELECT id,count,actor_id,status,created_at,finished_at,flag_count FROM flag_submitter_requests WHERE status='pending' ORDER BY created_at,rowid LIMIT 1",[],read_request).optional()?)).await
    }
    pub(super) async fn latest_manual(&self) -> anyhow::Result<Option<RunRequest>> {
        self.auth.db.run(|db| Ok(db.query_row("SELECT id,count,actor_id,status,created_at,finished_at,flag_count FROM flag_submitter_requests ORDER BY rowid DESC LIMIT 1",[],read_request).optional()?)).await
    }
    pub(super) async fn finish_manual(
        &self,
        request: RunRequest,
        flags: u32,
    ) -> anyhow::Result<()> {
        self.auth.db.run(move|db| {let tx=db.transaction()?;
            tx.execute("UPDATE flag_submitter_requests SET status='finished',finished_at=?2,flag_count=?3 WHERE id=?1",params![request.id,now(),flags])?;
            audit::append_change(&tx,&Actor::user(request.actor_id),"submitter.run.finish",&request.id,&request.id,true,now(),&serde_json::json!({"requested":request.count,"attempted":flags}))?;
            tx.execute("DELETE FROM flag_submitter_requests WHERE status NOT IN ('pending','running') AND id NOT IN (SELECT id FROM flag_submitter_requests ORDER BY rowid DESC LIMIT 500)",[])?;
            tx.commit()?;Ok(())}).await
    }
}
fn read_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRequest> {
    Ok(RunRequest {
        id: row.get(0)?,
        count: row.get(1)?,
        actor_id: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        finished_at: row.get(5)?,
        flag_count: row.get(6)?,
    })
}
