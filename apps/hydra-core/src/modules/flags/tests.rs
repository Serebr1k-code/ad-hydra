use super::*;
use crate::infrastructure::{crypto::Crypto, db::Database};
async fn fixture() -> (Flags, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("hydra-flags-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = Database::open(dir.join("db")).await.unwrap();
    db.run(|db|{db.execute_batch("INSERT INTO farm_targets VALUES('allowed','allowed','127.0.0.1',80,'tcp',1,1,0,0,'user'),('disabled','disabled','127.0.0.1',80,'tcp',0,1,0,0,'user');")?;Ok(())}).await.unwrap();
    let auth = AuthState::new(
        db,
        Crypto::load(&dir.join("key")).unwrap(),
        "https://localhost".into(),
    );
    let flags = Flags::open(
        auth,
        Settings {
            pattern: Some("FLAG[0-9]+".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (flags, dir)
}
#[tokio::test]
async fn captured_dedup_allowlist_and_redaction() {
    let (f, dir) = fixture().await;
    let actor = Actor::user("user-id".into());
    assert!(
        f.record_captured(
            "FLAG123".into(),
            "disabled".into(),
            None,
            actor.clone(),
            "request".into()
        )
        .await
        .is_err()
    );
    let id = f
        .record_captured(
            "FLAG123".into(),
            "allowed".into(),
            None,
            actor.clone(),
            "request".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        id,
        f.record_captured(
            "FLAG123".into(),
            "allowed".into(),
            None,
            actor.clone(),
            "request".into()
        )
        .await
        .unwrap()
    );
    let rows = f.list(actor, "read".into()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!serde_json::to_string(&rows).unwrap().contains("FLAG123"));
    assert!(f.extract(&vec![0; 65537]).is_err());
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn defensive_findings_replay_and_never_submit() {
    let (f, dir) = fixture().await;
    let batch = hydra_wire::v1::EventBatch {
        proxy_epoch_id: "epoch".into(),
        critical: true,
        events: vec![hydra_wire::v1::Event {
            event_id: "event".into(),
            payload: Some(hydra_wire::v1::event::Payload::Flag(
                hydra_wire::v1::FlagFinding {
                    flag_value: "FLAG123".into(),
                    source_kind: "own_exposed".into(),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };
    f.ingest_proxy(&batch).await.unwrap();
    f.ingest_proxy(&batch).await.unwrap();
    let actor = Actor::user("user".into());
    let rows = f.list(actor.clone(), "read".into()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].category, "own_exposed");
    assert!(
        f.submissions(actor, "read".into())
            .await
            .unwrap()
            .is_empty()
    );
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn submission_is_idempotent_and_proxy_categories_are_ineligible() {
    let (f, dir) = fixture().await;
    let f = Flags::open(
        f.auth.clone(),
        Settings {
            pattern: Some("FLAG[0-9]+".into()),
            checker: Some(Checker::Tcp {
                address: "127.0.0.1:9".parse().unwrap(),
                secret: None,
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let actor = Actor::user("initiator".into());
    let id = f
        .record_captured(
            "FLAG1".into(),
            "allowed".into(),
            Some("run".into()),
            actor.clone(),
            "capture".into(),
        )
        .await
        .unwrap();
    f.submit(id.clone(), actor.clone(), "submit-one".into())
        .await
        .unwrap();
    f.submit(id, actor.clone(), "submit-two".into())
        .await
        .unwrap();
    let rows = f.submissions(actor.clone(), "list".into()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].attempts, 0);
    assert_eq!(rows[0].initiated_by, "initiator");
    let unknown = f
        .insert(
            "FLAG2".into(),
            "unknown".into(),
            None,
            None,
            Actor {
                kind: "system".into(),
                id: "proxy".into(),
            },
            "event".into(),
            None,
            false,
        )
        .await
        .unwrap();
    assert!(f.submit(unknown, actor, "bad-submit".into()).await.is_err());
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn unknown_triage_requires_target_and_defensive_digest_is_ineligible() {
    let (f, dir) = fixture().await;
    let actor = Actor::user("triager".into());
    let id = f
        .insert(
            "FLAG42".into(),
            "unknown".into(),
            None,
            None,
            Actor {
                kind: "system".into(),
                id: "proxy".into(),
            },
            "event".into(),
            None,
            false,
        )
        .await
        .unwrap();
    assert!(
        f.triage(
            id.clone(),
            Triage {
                resolution: "captured".into(),
                target_id: None
            },
            actor.clone(),
            "bad".into()
        )
        .await
        .is_err()
    );
    f.triage(
        id.clone(),
        Triage {
            resolution: "own_exposed".into(),
            target_id: None,
        },
        actor.clone(),
        "triage".into(),
    )
    .await
    .unwrap();
    assert!(
        f.triage(
            id,
            Triage {
                resolution: "captured".into(),
                target_id: Some("allowed".into())
            },
            actor.clone(),
            "bad".into()
        )
        .await
        .is_err()
    );
    assert!(
        f.record_captured(
            "FLAG42".into(),
            "allowed".into(),
            None,
            actor.clone(),
            "bad".into()
        )
        .await
        .is_err()
    );
    let rows = f
        .list_page(
            actor,
            "browse".into(),
            Browse {
                offset: 0,
                limit: Some(1),
                search: Some("own_exposed".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].triage.as_deref(), Some("own_exposed"));
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn defensive_observation_quarantines_previously_queued_capture() {
    let (f, dir) = fixture().await;
    let f = Flags::open(
        f.auth.clone(),
        Settings {
            pattern: Some("FLAG[0-9]+".into()),
            checker: Some(Checker::Tcp {
                address: "127.0.0.1:9".parse().unwrap(),
                secret: None,
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let actor = Actor::user("initiator".into());
    let id = f
        .record_captured(
            "FLAG77".into(),
            "allowed".into(),
            None,
            actor.clone(),
            "capture".into(),
        )
        .await
        .unwrap();
    f.submit(id, actor.clone(), "submit".into()).await.unwrap();
    f.insert(
        "FLAG77".into(),
        "own_exposed".into(),
        None,
        None,
        Actor {
            kind: "system".into(),
            id: "proxy".into(),
        },
        "defensive".into(),
        None,
        false,
    )
    .await
    .unwrap();
    let rows = f.submissions(actor, "list".into()).await.unwrap();
    assert_eq!(rows[0].state, "quarantined");
    assert_eq!(rows[0].attempts, 0);
    let count=f.auth.db.run(|db|Ok(db.query_row("SELECT count(*) FROM audit WHERE action='submissions.quarantine' AND metadata LIKE '%queued%'",[],|r|r.get::<_,i64>(0))?)).await.unwrap();
    assert_eq!(count, 1);
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}
