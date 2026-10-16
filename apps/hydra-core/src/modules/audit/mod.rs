use rusqlite::{Transaction, params};
use serde::Serialize;
use utoipa::ToSchema;
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Actor {
    pub kind: String,
    pub id: String,
}
impl Actor {
    pub fn user(id: String) -> Self {
        Self {
            kind: "user".into(),
            id,
        }
    }
    pub fn local_cli() -> Self {
        Self {
            kind: "local_cli".into(),
            id: std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with("Uid:"))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "unknown".into()),
        }
    }
}
/// Structured metadata is deliberately fixed: credentials and token material cannot enter audit.
pub fn append(
    tx: &Transaction<'_>,
    actor: &Actor,
    action: &str,
    target: &str,
    request_id: &str,
    success: bool,
    now: i64,
) -> anyhow::Result<()> {
    let count: i64 = tx.query_row("SELECT count(*) FROM audit", [], |r| r.get(0))?;
    anyhow::ensure!(count < 1_000_000, "audit quota exhausted");
    tx.execute("INSERT INTO audit(at,actor_kind,actor_id,action,target,request_id,success,metadata) VALUES(?1,?2,?3,?4,?5,?6,?7,'{}')",params![now,actor.kind,actor.id,action,target,request_id,success])?;
    Ok(())
}
/// Callers must provide redacted before/after metadata only; bounded to 16 KiB.
#[allow(clippy::too_many_arguments)]
pub fn append_change(
    tx: &Transaction<'_>,
    actor: &Actor,
    action: &str,
    target: &str,
    request_id: &str,
    success: bool,
    now: i64,
    metadata: &serde_json::Value,
) -> anyhow::Result<()> {
    let metadata = serde_json::to_string(metadata)?;
    anyhow::ensure!(metadata.len() <= 16384, "audit metadata too large");
    let count: i64 = tx.query_row("SELECT count(*) FROM audit", [], |r| r.get(0))?;
    anyhow::ensure!(count < 1_000_000, "audit quota exhausted");
    tx.execute("INSERT INTO audit(at,actor_kind,actor_id,action,target,request_id,success,metadata) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![now,actor.kind,actor.id,action,target,request_id,success,metadata])?;
    Ok(())
}
#[derive(Serialize, ToSchema)]
pub struct AuditEvent {
    pub id: i64,
    pub at: String,
    pub actor_kind: String,
    pub actor_id: String,
    pub action: String,
    pub target: String,
    pub request_id: String,
    pub success: bool,
    pub metadata: String,
}
pub async fn list(
    db: &crate::infrastructure::db::Database,
    actor: Actor,
    after: i64,
    request_id: String,
) -> anyhow::Result<Vec<AuditEvent>> {
    db.run(move|db|{let tx=db.transaction()?;let rows=tx.prepare("SELECT id,strftime('%Y-%m-%dT%H:%M:%SZ',at,'unixepoch'),actor_kind,actor_id,action,target,request_id,success,metadata FROM audit WHERE id>?1 ORDER BY id LIMIT 100")?.query_map([after],|r|Ok(AuditEvent{id:r.get(0)?,at:r.get(1)?,actor_kind:r.get(2)?,actor_id:r.get(3)?,action:r.get(4)?,target:r.get(5)?,request_id:r.get(6)?,success:r.get(7)?,metadata:r.get(8)?}))?.collect::<Result<Vec<_>,_>>()?;append(&tx,&actor,"audit.list","audit",&request_id,true,crate::modules::auth::now())?;tx.commit()?;Ok(rows)}).await
}

tokio::task_local! { static HTTP_REQUEST_ID:String; }
pub fn request_id() -> String {
    HTTP_REQUEST_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string())
}
pub async fn scope_request<T>(id: String, future: impl std::future::Future<Output = T>) -> T {
    HTTP_REQUEST_ID.scope(id, future).await
}
pub async fn http_attempt(
    db: &crate::infrastructure::db::Database,
    actor: Actor,
    id: String,
    path: String,
    method: String,
    status: Option<u16>,
) -> anyhow::Result<()> {
    db.run(move|db|{let tx=db.transaction()?;let success=status.is_none_or(|s|s<400);
        append_change(&tx,&actor,if status.is_some(){"http.mutation.result"}else{"http.mutation.intent"},&path,&id,success,crate::modules::auth::now(),&serde_json::json!({"source":"http","method":method,"before":{"dispatch":status.is_none()},"after":{"status":status}}))?;
        tx.commit()?;Ok(())
    }).await
}
