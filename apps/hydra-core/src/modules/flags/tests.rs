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
    assert!(rows.is_empty());
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
    // Forgetting a defensive observation does not alter an independently captured value.
    f.record_captured(
        "FLAG42".into(),
        "allowed".into(),
        None,
        actor.clone(),
        "capture".into(),
    )
    .await
    .unwrap();
    let rows = f
        .list_page(
            actor,
            "browse".into(),
            Browse {
                offset: 0,
                limit: Some(1),
                search: Some("own_exposed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(rows.is_empty());
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn defensive_values_cannot_be_inserted() {
    let (f, dir) = fixture().await;
    assert!(
        f.insert(
            "FLAG77".into(),
            "own_exposed".into(),
            None,
            None,
            Actor::user("user".into()),
            "test".into(),
            None,
            false
        )
        .await
        .is_err()
    );
    assert!(
        f.list(Actor::user("user".into()), "read".into())
            .await
            .unwrap()
            .is_empty()
    );
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn pasted_flags_keep_exact_values_queue_once_and_page_by_service() {
    let (f, dir) = fixture().await;
    f.auth.db.run(|db| {db.execute(r#"UPDATE control_state SET services='[{"id":"docs","enabled":true},{"id":"ark","enabled":true}]' WHERE id=1"#,[])?;Ok(())}).await.unwrap();
    let input = |text: &str, service: &str| super::imports::ImportFlags {
        text: text.into(),
        service_id: Some(service.into()),
        target_id: None,
    };
    let actor = Actor::user("importer".into());
    let result = f
        .import_text(
            input("noise FLAG001 FLAG002 FLAG001", "docs"),
            actor.clone(),
            "paste".into(),
        )
        .await
        .unwrap();
    assert_eq!((result.matched, result.added, result.queued), (3, 2, 2));
    f.import_text(input("FLAG003", "ark"), actor.clone(), "paste2".into())
        .await
        .unwrap();
    let raw = f
        .auth
        .db
        .run(|db| {
            Ok(db
                .prepare("SELECT value FROM flags ORDER BY rowid")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .unwrap();
    assert_eq!(raw, vec!["FLAG001", "FLAG002", "FLAG003"]);
    let first = f
        .browse_flags(
            actor.clone(),
            "page".into(),
            Browse {
                limit: Some(1),
                sort: Some("service".into()),
                order: Some("asc".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(first.items[0].value.as_deref(), Some("FLAG003"));
    assert_eq!(first.items[0].service_id.as_deref(), Some("ark"));
    let next = f
        .browse_flags(
            actor,
            "page2".into(),
            Browse {
                offset: 1,
                limit: Some(1),
                sort: Some("service".into()),
                order: Some("asc".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(next.items[0].service_id.as_deref(), Some("docs"));
    assert_eq!(f.extract(b"\xffFLAG004\xfe").unwrap(), ["FLAG004"]);
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn migration_forgets_legacy_defensive_values_and_preserves_detection_metadata() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("hydra-flag-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = Database::open(dir.join("db")).await.unwrap();
    db.run(|db|{db.execute_batch("INSERT INTO flags VALUES('old-own','digest-own','own_exposed','FLAG999',NULL,NULL,'proxy',0); INSERT INTO flag_events(event_id,flag_id,flow_id,service_id) VALUES('old-event','old-own','flow','docs'); INSERT INTO submissions(id,flag_id,next_attempt,created_at,initiated_by) VALUES('old-sub','old-own',0,0,'proxy');")?;Ok(())}).await.unwrap();
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
    flags.auth.db.run(|db|{
        assert_eq!(db.query_row("SELECT count(*) FROM flags",[],|r|r.get::<_,i64>(0))?,0);
        assert_eq!(db.query_row("SELECT count(*) FROM submissions",[],|r|r.get::<_,i64>(0))?,0);
        assert!(db.query_row("SELECT flag_id IS NULL AND flow_id='flow' AND service_id='docs' FROM flag_events WHERE event_id='old-event'",[],|r|r.get::<_,bool>(0))?);
        assert_eq!(db.query_row("SELECT count(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0))?,0);
        Ok(())
    }).await.unwrap();
    drop(flags);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn unconfigured_extraction_leaves_farm_output_usable() {
    let (flags, dir) = fixture().await;
    *flags.farm_regex.write().unwrap() = None;
    assert!(flags.extract(b"ordinary output\xff").unwrap().is_empty());
    assert!(flags.extract(b"").unwrap().is_empty());
    assert!(flags.extract(&vec![0; 65537]).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn explicit_submit_requeues_failed_flag_with_new_attempt_budget() {
    let (flags, dir) = fixture().await;
    let id = flags
        .record_captured(
            "FLAG765".into(),
            "allowed".into(),
            None,
            Actor::user("capturer".into()),
            "capture".into(),
        )
        .await
        .unwrap();
    flags.auth.db.run(|db| {db.execute("UPDATE submissions SET state='failed',attempts=10,next_attempt=9999999999,response='previous failure'",[])?;Ok(())}).await.unwrap();
    flags
        .submit(
            id.clone(),
            Actor::user("resubmitter".into()),
            "resubmit".into(),
        )
        .await
        .unwrap();
    let rows = flags
        .submissions(Actor::user("reader".into()), "read".into())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "queued");
    assert_eq!(rows[0].attempts, 0);
    assert_eq!(rows[0].initiated_by, "resubmitter");
    assert!(rows[0].response.is_empty());
    let value=flags.auth.db.run(move |db|{Ok(db.query_row("SELECT f.value,s.next_attempt,(SELECT count(*) FROM audit WHERE action='submissions.requeue' AND actor_id='resubmitter') FROM flags f JOIN submissions s ON s.flag_id=f.id WHERE f.id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,u32>(2)?)))?)}).await.unwrap();
    assert_eq!(value.0, "FLAG765");
    assert!(value.1 <= crate::modules::auth::now());
    assert_eq!(value.2, 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn farm_counts_distinguish_new_existing_and_deduplicate_observation_replays() {
    let (flags, dir) = fixture().await;
    flags.auth.db.run(|db|{for id in ["process1","process2"] {db.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at) VALUES(?1,'e','v','{}','[\"allowed\"]','actor','running',0)",[id])?;}Ok(())}).await.unwrap();
    for (run, value) in [
        ("process1", "FLAG800"),
        ("process1", "FLAG800"),
        ("process2", "FLAG800"),
        ("process2", "FLAG801"),
    ] {
        flags
            .record_farm_captured(value.into(), "allowed".into(), run.into(), "actor".into())
            .await
            .unwrap();
    }
    let counts=flags.auth.db.run(|db|{Ok(db.prepare("SELECT run_id,SUM(is_new),COUNT(*)-SUM(is_new) FROM farm_flag_observations GROUP BY run_id ORDER BY run_id")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?,r.get::<_,u32>(2)?)))?.collect::<Result<Vec<_>,_>>()?)}).await.unwrap();
    assert_eq!(
        counts,
        vec![("process1".into(), 1, 0), ("process2".into(), 1, 1)]
    );
    flags
        .auth
        .db
        .run(|db| {
            db.execute("DELETE FROM farm_runs WHERE id='process1'", [])?;
            let count: u32 = db.query_row(
                "SELECT entries FROM farm_flag_counter_budget WHERE id=1",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 2);
            Ok(())
        })
        .await
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
