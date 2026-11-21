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
    async fn million_audit_rows_do_not_block_appends_or_get_evicted() {
        let (directory, db) = database().await;
        db.run(move |db| {
            let tx = db.transaction()?;
            tx.execute_batch(
                "WITH RECURSIVE seq(n) AS (
                     VALUES(1)
                     UNION ALL
                     SELECT n + 1 FROM seq WHERE n < 1000000
                 )
                 INSERT INTO audit(at,actor_kind,actor_id,action,target,request_id,success,metadata)
                 SELECT n,'user','seed','test.seed','seed','seed',1,'{}' FROM seq;",
            )?;
            let actor = Actor::user("u".into());
            append(
                &tx,
                &actor,
                "auth.login",
                "user:u",
                "login-request",
                true,
                1_000_001,
            )?;
            append_change(
                &tx,
                &actor,
                "service.update",
                "service:test",
                "mutation-request",
                true,
                1_000_002,
                &serde_json::json!({"before":{"enabled":false},"after":{"enabled":true}}),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        let (rows, oldest_id, seeds, logins, mutations): (i64, i64, i64, i64, i64) = db
            .run(|db| {
                Ok(
                    db.query_row(
                        "SELECT count(*),min(id),sum(action='test.seed'),sum(action='auth.login'),sum(action='service.update') FROM audit",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                    )?,
                )
            })
            .await
            .unwrap();
        assert_eq!(rows, 1_000_002, "appends must not prune the audit history");
        assert_eq!(
            oldest_id, 1,
            "the oldest audit event should remain retained"
        );
        assert_eq!(seeds, 1_000_000, "all seeded audit events should remain");
        assert_eq!(logins, 1, "the login-style append should succeed");
        assert_eq!(mutations, 1, "the mutation append should succeed");
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
