use super::*;
use crate::{
    infrastructure::{crypto::Crypto, db::Database},
    modules::{audit::Actor, auth::AuthState},
};
use std::{os::unix::fs::PermissionsExt, sync::Arc};
async fn setup() -> (Farm, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("hydra-farm-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = Database::open(dir.join("db")).await.unwrap();
    let auth = AuthState::new(
        db,
        Crypto::load(&dir.join("key")).unwrap(),
        "https://localhost".into(),
    );
    (
        Farm::open(auth, dir.join("uploads"), Arc::new(UnavailableRunner))
            .await
            .unwrap(),
        dir,
    )
}
fn target() -> Target {
    Target {
        id: "local".into(),
        name: "authorized toy target".into(),
        host: "127.0.0.1".into(),
        port: 9000,
        transport: "tcp".into(),
        enabled: true,
        revision: 0,
    }
}
fn exploit() -> Exploit {
    Exploit {
        id: "toy".into(),
        name: "toy".into(),
        enabled: true,
        revision: 0,
        version: "v1".into(),
        manifest: Manifest {
            runtime: "shell".into(),
            mode: "batch".into(),
            timeout_seconds: 1,
            memory_mib: 32,
            cpu_percent: 10,
        },
        target_ids: vec!["local".into()],
        interval_seconds: None,
    }
}
#[tokio::test]
async fn persisted_queue_audit_idempotency_and_fail_closed() {
    let (f, dir) = setup().await;
    let a = Actor::user("operator".into());
    let mut bad = target();
    bad.host = "example.com".into();
    assert!(f.target(a.clone(), bad).await.is_err());
    f.target(a.clone(), target()).await.unwrap();
    assert!(f.target(a.clone(), target()).await.is_err());
    f.upload(a.clone(), "toy".into(), "v1".into(), b"exit 0".to_vec())
        .await
        .unwrap();
    assert!(
        f.upload(a.clone(), "toy".into(), "v1".into(), vec![1])
            .await
            .is_err()
    );
    f.exploit(a.clone(), exploit()).await.unwrap();
    let id = f
        .enqueue(a.clone(), "toy".into(), 1, "one".into())
        .await
        .unwrap();
    assert_eq!(
        id,
        f.enqueue(a.clone(), "toy".into(), 1, "one".into())
            .await
            .unwrap()
    );
    assert!(
        f.enqueue(a.clone(), "toy".into(), 2, "one".into())
            .await
            .is_err()
    );
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(f.clone().background(stop.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    stop.cancel();
    task.await.unwrap();
    f.auth
        .db
        .run(move |db| {
            let (status, err): (String, String) = db.query_row(
                "SELECT status,stderr FROM farm_runs WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            assert_eq!(status, "failed");
            assert!(err.contains("sandbox prerequisites"));
            let n: i64 = db.query_row(
                "SELECT count(*) FROM audit WHERE actor_id='operator' AND action LIKE 'farm.%'",
                [],
                |r| r.get(0),
            )?;
            assert!(n >= 4);
            let system:i64=db.query_row("SELECT count(*) FROM audit WHERE actor_kind='system' AND actor_id='farm-scheduler' AND json_extract(metadata,'$.initiated_by')='operator'",[],|r|r.get(0))?;
            assert!(system>=2);
            Ok(())
        })
        .await
        .unwrap();
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn disabled_targets_rejected_and_cancel_persisted() {
    let (f, dir) = setup().await;
    let a = Actor::user("operator".into());
    f.target(a.clone(), target()).await.unwrap();
    f.upload(a.clone(), "toy".into(), "v1".into(), vec![1])
        .await
        .unwrap();
    f.exploit(a.clone(), exploit()).await.unwrap();
    let id = f
        .enqueue(a.clone(), "toy".into(), 1, "one".into())
        .await
        .unwrap();
    f.cancel(a.clone(), id.clone()).await.unwrap();
    assert!(f.cancel(a.clone(), id).await.is_err());
    let mut t = target();
    t.revision = 1;
    t.enabled = false;
    f.target(a.clone(), t).await.unwrap();
    assert!(f.enqueue(a, "toy".into(), 1, "two".into()).await.is_err());
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn schedules_keep_user_attribution_and_restart_does_not_replay_running() {
    let (f, dir) = setup().await;
    let a = Actor::user("scheduler-owner".into());
    f.target(a.clone(), target()).await.unwrap();
    f.upload(a.clone(), "toy".into(), "v1".into(), vec![1])
        .await
        .unwrap();
    let mut e = exploit();
    e.interval_seconds = Some(10);
    f.exploit(a.clone(), e).await.unwrap();
    f.auth
        .db
        .run(|db| {
            db.execute("UPDATE farm_exploits SET next_run=0", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(f.clone().background(stop.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    stop.cancel();
    task.await.unwrap();
    f.auth
        .db
        .run(|db| {
            let user: String =
                db.query_row("SELECT initiated_by FROM farm_runs", [], |r| r.get(0))?;
            assert_eq!(user, "scheduler-owner");
            let n:i64=db.query_row("SELECT count(*) FROM audit WHERE actor_kind='system' AND action='farm.run.enqueue' AND json_extract(metadata,'$.initiated_by')='scheduler-owner'",[],|r|r.get(0))?;
            assert_eq!(n,1);
            db.execute("UPDATE farm_runs SET status='running'", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let auth = f.auth.clone();
    drop(f);
    let reopened = Farm::open(auth, dir.join("uploads"), Arc::new(UnavailableRunner))
        .await
        .unwrap();
    reopened
        .auth
        .db
        .run(|db| {
            let status: String = db.query_row("SELECT status FROM farm_runs", [], |r| r.get(0))?;
            assert_eq!(status, "interrupted");
            Ok(())
        })
        .await
        .unwrap();
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn retries_preserve_snapshot_and_are_idempotent_without_running_workers() {
    let (farm, dir) = setup().await;
    let actor = Actor::user("operator".into());
    farm.target(actor.clone(), target()).await.unwrap();
    farm.upload(actor.clone(), "toy".into(), "v1".into(), vec![1])
        .await
        .unwrap();
    farm.exploit(actor.clone(), exploit()).await.unwrap();
    let original = farm
        .enqueue(actor.clone(), "toy".into(), 1, "original".into())
        .await
        .unwrap();
    assert!(
        farm.retry(actor.clone(), original.clone(), "early".into())
            .await
            .is_err()
    );
    farm.cancel(actor.clone(), original.clone()).await.unwrap();
    let id = farm
        .retry(actor.clone(), original.clone(), "retry".into())
        .await
        .unwrap();
    assert_ne!(original, id);
    assert_eq!(
        id,
        farm.retry(actor.clone(), original.clone(), "retry".into())
            .await
            .unwrap()
    );
    assert!(
        farm.retry(
            Actor::user("another".into()),
            original.clone(),
            "retry".into()
        )
        .await
        .is_err()
    );
    farm.auth
        .db
        .run(move |db| {
            let (origin, initiated, version, status): (String, String, String, String) = db
                .query_row(
                    "SELECT origin_run_id,initiated_by,version,status FROM farm_runs WHERE id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
            assert_eq!(origin, original);
            assert_eq!(initiated, "operator");
            assert_eq!(version, "v1");
            assert_eq!(status, "queued");
            Ok(())
        })
        .await
        .unwrap();
    drop(farm);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn batch_target_aliases_rejected_without_worker_execution() {
    let (farm, directory) = setup().await;
    let actor = Actor::user("operator".into());
    farm.target(actor.clone(), target()).await.unwrap();
    let mut alias = target();
    alias.id = "alias".into();
    farm.target(actor.clone(), alias).await.unwrap();
    let mut batch = exploit();
    batch.target_ids.push("alias".into());
    assert!(
        farm.exploit(actor.clone(), batch.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("duplicate batch target endpoint")
    );
    batch.manifest.mode = "per_target".into();
    farm.exploit(actor, batch).await.unwrap();
    drop(farm);
    std::fs::remove_dir_all(directory).unwrap();
}
