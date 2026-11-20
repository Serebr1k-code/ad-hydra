use rusqlite::{OptionalExtension, Transaction, params};
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
    make_room(tx, ROW_LIMIT)?;
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
    make_room(tx, ROW_LIMIT)?;
    tx.execute("INSERT INTO audit(at,actor_kind,actor_id,action,target,request_id,success,metadata) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![now,actor.kind,actor.id,action,target,request_id,success,metadata])?;
    Ok(())
}
/// Rows kept before the oldest records are evicted. A hard cap that refuses the
/// append would fail the caller's transaction, and `login` appends inside it.
const ROW_LIMIT: i64 = 1_000_000;
/// Keep the table bounded without ever failing the caller. `audit_no_delete`
/// permits deleting ids at or below `audit_retention.floor`, so eviction raises
/// the floor to the last row it removes and then deletes up to it.
fn make_room(tx: &Transaction<'_>, limit: i64) -> anyhow::Result<()> {
    let rows: i64 = tx.query_row("SELECT count(*) FROM audit", [], |r| r.get(0))?;
    if rows < limit {
        return Ok(());
    }
    // One pass frees about one percent of the oldest rows, always at least one.
    let batch = if limit < 100 { 1 } else { limit / 100 };
    let boundary: Option<i64> = tx
        .query_row(
            "SELECT id FROM audit ORDER BY id LIMIT 1 OFFSET ?1",
            [batch],
            |r| r.get(0),
        )
        .optional()?;
    let Some(boundary) = boundary else {
        return Ok(());
    };
    let floor = boundary - 1;
    tx.execute(
        "UPDATE audit_retention SET floor=CASE WHEN floor<?1 THEN ?1 ELSE floor END WHERE id=1",
        [floor],
    )?;
    let evicted = tx.execute("DELETE FROM audit WHERE id<=?1", [floor])?;
    tracing::warn!(
        evicted,
        limit,
        "audit row limit reached; evicted the oldest records"
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::db::Database;

    async fn database() -> (std::path::PathBuf, Database) {
        let directory = std::env::temp_dir().join(format!("hydra-audit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db = Database::open(directory.join("db")).await.unwrap();
        (directory, db)
    }

    #[tokio::test]
    async fn the_row_cap_evicts_the_oldest_rows_instead_of_failing_the_caller() {
        let (directory, db) = database().await;
        let limit = 16;
        db.run(move |db| {
            let tx = db.transaction()?;
            for at in 0..limit {
                append(
                    &tx,
                    &Actor::user("u".into()),
                    "test.event",
                    "target",
                    "request",
                    true,
                    at,
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        db.run(move |db| {
            let tx = db.transaction()?;
            make_room(&tx, limit)?;
            append(
                &tx,
                &Actor::user("u".into()),
                "test.event",
                "target",
                "request",
                true,
                limit,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        let (rows, oldest, newest, floor): (i64, i64, i64, i64) = db
            .run(|db| {
                Ok(db.query_row(
                    "SELECT count(*),min(at),max(at),(SELECT floor FROM audit_retention) FROM audit",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?)
            })
            .await
            .unwrap();
        assert!(rows <= limit, "table grew past the limit: {rows}");
        assert_eq!(newest, limit, "the newest record was lost");
        assert!(oldest > 0, "the oldest records were not evicted");
        assert!(floor > 0, "the retention floor did not move");
        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn retained_rows_remain_append_only() {
        let (directory, db) = database().await;
        db.run(|db| {
            let tx = db.transaction()?;
            append(
                &tx,
                &Actor::user("u".into()),
                "test.event",
                "target",
                "request",
                true,
                0,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        let error = db
            .run(|db| Ok(db.execute("DELETE FROM audit", []).map(|_| ())?))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("append-only audit"),
            "unexpected error: {error}"
        );
        std::fs::remove_dir_all(directory).ok();
    }
}
