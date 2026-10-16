//! An override is retained in Core until Proxy durably acknowledges its removal.
use super::{
    Protocols,
    service::{actor, name, request},
};
use crate::{
    infrastructure::proxy_client,
    modules::{
        audit::{self, Actor},
        auth::now,
    },
};
use hydra_wire::v1::{self, proxy_control_client::ProxyControlClient};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
impl Protocols {
    pub async fn deactivate(
        &self,
        a: Actor,
        service: String,
        key: String,
    ) -> anyhow::Result<Value> {
        name(&service)?;
        anyhow::ensure!(
            (16..=128).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_graphic()),
            "Idempotency-Key must have 16..128 printable bytes"
        );
        let fingerprint = format!("deactivate:{:x}", Sha256::digest(service.as_bytes()));
        let req = audit::request_id();
        let complete=self.auth.db.run({let a=a.clone();let service=service.clone();let key=key.clone();let fingerprint=fingerprint.clone();let req=req.clone();move|db|{
            let tx=db.transaction()?;
            let old=tx.query_row("SELECT fingerprint FROM protocol_operations WHERE actor=?1 AND key=?2",params![a.id,key],|r|r.get::<_,String>(0)).optional()?;
            if let Some(old)=old {
                anyhow::ensure!(old==fingerprint || old==format!("{fingerprint}:done"),"idempotency key conflict");
                if old.ends_with(":done") {
                    audit::append(&tx,&a,"protocol.deactivate.replay",&service,&req,true,now())?;
                    tx.commit()?;
                    return Ok(true);
                }
            } else {
                let count:i64=tx.query_row("SELECT count(*) FROM protocol_operations",[],|r|r.get(0))?;
                anyhow::ensure!(count<10000,"operation quota exhausted");
                tx.execute("INSERT INTO protocol_operations VALUES(?1,?2,?3)",params![a.id,key,fingerprint])?;
            }
            tx.execute("UPDATE protocol_activations SET applied=0,error='deactivation pending',updated_by=?2,updated_at=?3 WHERE service_id=?1",params![service,a.id,now()])?;
            audit::append_change(&tx,&a,"protocol.deactivate.request",&service,&req,true,now(),&json!({"source":"http","after":{"override":"removal_pending"}}))?;
            tx.commit()?;
            Ok(false)
        }}).await?;
        if complete {
            return Ok(json!({"service_id":service,"applied":true,"error":null}));
        }
        let result = async {
            let channel = proxy_client::connect(&self.socket).await?;
            let response = ProxyControlClient::new(channel)
                .deactivate_protocol(request(v1::DeactivateProtocolRequest {
                    service_id: service.clone(),
                    request_id: req.clone(),
                    actor: Some(actor(&a, &req)),
                }))
                .await?
                .into_inner();
            anyhow::ensure!(response.applied, "{}", response.error_message);
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let error = result
            .err()
            .map(|e| e.to_string().chars().take(2048).collect::<String>());
        let applied = error.is_none();
        self.auth.db.run({let service=service.clone();let error=error.clone();move|db|{
            let tx=db.transaction()?;
            if applied {
                tx.execute("DELETE FROM protocol_activations WHERE service_id=?1",[&service])?;
                tx.execute("UPDATE protocol_operations SET fingerprint=?3 WHERE actor=?1 AND key=?2",params![a.id,key,format!("{fingerprint}:done")])?;
            } else {
                tx.execute("UPDATE protocol_activations SET applied=0,error=?2 WHERE service_id=?1",params![service,format!("deactivation pending: {}",error.as_deref().unwrap_or("not acknowledged"))])?;
            }
            audit::append_change(&tx,&a,"protocol.deactivate.result",&service,&req,applied,now(),&json!({"source":"http","after":{"override":if applied{"removed"}else{"removal_pending"}}}))?;
            tx.commit()?;
            Ok(())
        }}).await?;
        Ok(json!({"service_id":service,"applied":applied,"error":error}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        infrastructure::{crypto::Crypto, db::Database},
        modules::{auth::AuthState, services::Control},
    };
    #[tokio::test]
    async fn completed_deactivation_replays_without_removing_a_later_override() {
        use std::os::unix::fs::DirBuilderExt;
        let directory =
            std::env::temp_dir().join(format!("hydra-deactivation-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let db = Database::open(directory.join("db")).await.unwrap();
        let auth = AuthState::new(
            db.clone(),
            Crypto::load(&directory.join("key")).unwrap(),
            "https://hydra.test".into(),
        );
        let control = Control::open(auth.clone()).await.unwrap();
        let protocols = Protocols::open(
            auth,
            control,
            directory.join("nonexistent.sock"),
            directory.join("protocols"),
        )
        .await
        .unwrap();
        // In-memory routing also preserves uploading the formerly valid ID "activations".
        use tower::ServiceExt;
        let router = super::super::router(protocols.clone());
        for method in ["PUT", "DELETE"] {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(method)
                        .uri("/api/v1/protocols/activations/service")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        }
        let fingerprint = format!("deactivate:{:x}:done", Sha256::digest(b"service"));
        db.run(move|db|{
            db.execute("INSERT INTO protocol_operations VALUES('operator','completed-key-12345',?1)",[fingerprint])?;
            db.execute("INSERT INTO protocol_activations VALUES('service','later','2',1,'operator',0,NULL)",[])?;
            Ok(())
        }).await.unwrap();
        for _ in 0..2 {
            let result = protocols
                .deactivate(
                    Actor::user("operator".into()),
                    "service".into(),
                    "completed-key-12345".into(),
                )
                .await
                .unwrap();
            assert_eq!(result["applied"], true);
        }
        assert!(
            protocols
                .deactivate(
                    Actor::user("operator".into()),
                    "other-service".into(),
                    "completed-key-12345".into()
                )
                .await
                .is_err()
        );
        db.run(|db|{
            let current:String=db.query_row("SELECT protocol_id FROM protocol_activations WHERE service_id='service'",[],|r|r.get(0))?;
            assert_eq!(current,"later");
            let replays:i64=db.query_row("SELECT count(*) FROM audit WHERE action='protocol.deactivate.replay' AND actor_id='operator'",[],|r|r.get(0))?;
            assert_eq!(replays,2);
            Ok(())
        }).await.unwrap();
        drop(protocols);
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
