//! Offline CLI mutations retain an audit trail even when the daemon is stopped.
use super::{db::Database, proxy_client};
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use hydra_domain::ServiceSnapshot;
use std::path::Path;
pub async fn apply(
    database: &Path,
    socket: &Path,
    snapshot: &ServiceSnapshot,
) -> anyhow::Result<hydra_wire::v1::ApplyResult> {
    let db = Database::open(database).await?;
    let actor = Actor::local_cli();
    let request = uuid::Uuid::new_v4().to_string();
    let metadata =
        serde_json::json!({"revision":snapshot.revision,"service_count":snapshot.services.len()});
    record(
        &db,
        actor.clone(),
        request.clone(),
        "proxy.snapshot.request",
        true,
        metadata.clone(),
    )
    .await?;
    let result = async {
        let channel = proxy_client::connect(socket).await?;
        proxy_client::apply(
            channel,
            snapshot,
            hydra_wire::v1::Actor {
                r#type: 3,
                username: actor.id.clone(),
                ..Default::default()
            },
            request.clone(),
        )
        .await
    }
    .await;
    let success = result.as_ref().is_ok_and(|r| r.applied);
    record(
        &db,
        actor,
        request,
        "proxy.snapshot.result",
        success,
        metadata,
    )
    .await?;
    result
}
async fn record(
    db: &Database,
    actor: Actor,
    request: String,
    action: &'static str,
    success: bool,
    metadata: serde_json::Value,
) -> anyhow::Result<()> {
    db.run(move |db| {
        let tx = db.transaction()?;
        audit::append_change(
            &tx,
            &actor,
            action,
            "proxy",
            &request,
            success,
            now(),
            &metadata,
        )?;
        tx.commit()?;
        Ok(())
    })
    .await
}
