use super::*;
use crate::modules::{audit::Actor, auth::now};
use rusqlite::params;
async fn catalog(f: &Farm) -> Exploit {
    let a = Actor::user("owner".into());
    f.save_service(
        a.clone(),
        FarmService {
            id: "web".into(),
            name: "Web".into(),
            port: 12345,
            transport: "tcp".into(),
            enabled: true,
            revision: 0,
        },
    )
    .await
    .unwrap();
    for (id, host) in [("one", "127.0.0.1"), ("two", "127.0.0.2")] {
        f.save_team(
            a.clone(),
            FarmTeam {
                id: id.into(),
                name: id.into(),
                host: host.into(),
                enabled: true,
                revision: 0,
            },
        )
        .await
        .unwrap();
    }
    let mut e = super::tests::exploit();
    e.service_id = Some("web".into());
    e.target_ids.clear();
    f.upload(
        a.clone(),
        e.id.clone(),
        e.version.clone(),
        b"printf harmless".to_vec(),
    )
    .await
    .unwrap();
    f.exploit(a, e.clone()).await.unwrap();
    e.revision = 1;
    e
}
#[tokio::test]
async fn service_team_selection_snapshots_exact_endpoints_and_idempotency() {
    let (f, dir) = super::tests::setup().await;
    let e = catalog(&f).await;
    let a = Actor::user("owner".into());
    let run = f
        .enqueue_for_team(
            a.clone(),
            e.id.clone(),
            1,
            "direct".into(),
            Some("two".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        run,
        f.enqueue_for_team(
            a.clone(),
            e.id.clone(),
            1,
            "direct".into(),
            Some("two".into())
        )
        .await
        .unwrap()
    );
    assert!(
        f.enqueue_for_team(
            a.clone(),
            e.id.clone(),
            1,
            "direct".into(),
            Some("one".into())
        )
        .await
        .is_err()
    );
    let all = f
        .enqueue(a.clone(), e.id.clone(), 1, "all".into())
        .await
        .unwrap();
    f.auth
        .db
        .run(move |db| {
            for (id, n) in [(run, 1), (all, 2)] {
                let snapshot: String = db.query_row(
                    "SELECT target_snapshot FROM farm_runs WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )?;
                let values: Vec<Target> = serde_json::from_str(&snapshot)?;
                assert_eq!(values.len(), n);
                assert!(values.iter().all(|t| t.port == 12345));
                if n == 1 {
                    assert_eq!(values[0].host, "127.0.0.2");
                }
            }
            Ok(())
        })
        .await
        .unwrap();
    f.save_team(
        a.clone(),
        FarmTeam {
            id: "two".into(),
            name: "two".into(),
            host: "127.0.0.2".into(),
            enabled: false,
            revision: 1,
        },
    )
    .await
    .unwrap();
    assert!(
        f.enqueue_for_team(a, e.id, 1, "disabled".into(), Some("two".into()))
            .await
            .is_err()
    );
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn rotation_keeps_active_runs_and_bounds_completed_gzip_history() {
    let (f, dir) = super::tests::setup().await;
    let e = catalog(&f).await;
    let a = Actor::user("owner".into());
    let queued = f
        .enqueue(a.clone(), e.id, 1, "queued".into())
        .await
        .unwrap();
    f.auth.db.run(|db| {
        let tx=db.transaction()?;
        for n in 0..1100 {tx.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at,finished_at,stdout_gzip,stderr_gzip) VALUES(?1,'toy','v1','{}','[]','owner','succeeded',?2,?2,?3,?3)",params![format!("completed-{n}"),now(),super::runtimes::encode_log(b"line")?])?;}
        tx.commit()?;Ok(())
    }).await.unwrap();
    assert_eq!(f.prune_runs(a.clone(), 7).await.unwrap(), 100);
    f.auth
        .db
        .run(move |db| {
            assert!(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM farm_runs WHERE id=?1 AND status='queued')",
                [queued],
                |r| r.get::<_, bool>(0)
            )?);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(f.prune_runs(a, 0).await.unwrap(), 1000);
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn authenticated_log_api_decompresses_gzip_and_audits_reads() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let (f, dir) = super::tests::setup().await;
    let e = catalog(&f).await;
    crate::modules::users::create(
        &f.auth.db,
        &f.auth.crypto,
        "reader".into(),
        "long-test-password".into(),
    )
    .await
    .unwrap();
    let token = f
        .auth
        .login("reader".into(), "long-test-password".into(), "test".into())
        .await
        .unwrap()
        .response
        .access_token;
    let run = f
        .enqueue(Actor::user("owner".into()), e.id, 1, "logs".into())
        .await
        .unwrap();
    let id = run.clone();
    f.auth
        .db
        .run(move |db| {
            db.execute(
                "UPDATE farm_runs SET status='succeeded',stdout_gzip=?2,stderr_gzip=?3 WHERE id=?1",
                params![
                    id,
                    super::runtimes::encode_log("hello €\n".as_bytes())?,
                    super::runtimes::encode_log(b"warning\n")?
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = router(f.clone());
    let path = format!("/api/v1/farm/runs/{run}");
    assert_eq!(
        app.clone()
            .oneshot(Request::get(&path).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status(),
        401
    );
    let response = app
        .oneshot(
            Request::get(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let value: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["stdout"], "hello €\n");
    assert_eq!(value["stderr"], "warning\n");
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn interrupted_or_failed_runner_keeps_live_gzip_output() {
    let (f, dir) = super::tests::setup().await;
    let e = catalog(&f).await;
    let a = Actor::user("owner".into());
    for (key, error) in [("interrupted", false), ("failed", true)] {
        let run = f
            .enqueue(a.clone(), e.id.clone(), 1, key.into())
            .await
            .unwrap();
        let id = run.clone();
        f.auth.db.run(move |db|{db.execute("UPDATE farm_runs SET status='running',stdout_gzip=?2,stderr_gzip=?3 WHERE id=?1",params![id,super::runtimes::encode_log(b"saved live stdout")?,super::runtimes::encode_log(b"saved live stderr")?])?;Ok(())}).await.unwrap();
        let output = if error {
            Err(anyhow::anyhow!("late error"))
        } else {
            Ok(RunOutput {
                status: "interrupted".into(),
                stdout: Vec::new(),
                stderr: b"shutdown".to_vec(),
                exit_code: None,
            })
        };
        super::completion::finish(&f, run.clone(), "owner".into(), output, !error)
            .await
            .unwrap();
        f.auth
            .db
            .run(move |db| {
                let (out, err): (Vec<u8>, Vec<u8>) = db.query_row(
                    "SELECT stdout_gzip,stderr_gzip FROM farm_runs WHERE id=?1",
                    [run],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                assert_eq!(super::runtimes::decode_log(&out)?, b"saved live stdout");
                assert!(
                    String::from_utf8(super::runtimes::decode_log(&err)?)
                        .unwrap()
                        .contains("saved live stderr")
                );
                Ok(())
            })
            .await
            .unwrap();
    }
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn team_named_all_does_not_collide_with_all_teams_and_legacy_keys_replay() {
    let (f, dir) = super::tests::setup().await;
    let e = catalog(&f).await;
    let a = Actor::user("owner".into());
    f.save_team(
        a.clone(),
        FarmTeam {
            id: "all".into(),
            name: "all".into(),
            host: "127.0.0.3".into(),
            enabled: true,
            revision: 0,
        },
    )
    .await
    .unwrap();
    let original = f
        .enqueue(a.clone(), e.id.clone(), 1, "selection".into())
        .await
        .unwrap();
    assert!(
        f.enqueue_for_team(
            a.clone(),
            e.id.clone(),
            1,
            "selection".into(),
            Some("all".into())
        )
        .await
        .is_err()
    );
    let id = original.clone();
    f.auth
        .db
        .run(move |db| {
            db.execute(
                "UPDATE farm_runs SET request_hash='toy:1:owner:false' WHERE id=?1",
                [id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        original,
        f.enqueue(a, e.id, 1, "selection".into()).await.unwrap()
    );
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn rotation_enforces_compressed_byte_budget_before_count_limit() {
    use std::io::Write;
    let (f, dir) = super::tests::setup().await;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
    encoder.write_all(&vec![0; 1024 * 1024]).unwrap();
    let large = encoder.finish().unwrap();
    f.auth.db.run(move|db| {
        let tx=db.transaction()?;
        for n in 0..65 {tx.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at,finished_at,stdout_gzip) VALUES(?1,'toy','v1','{}','[]','owner','succeeded',?2,?2,?3)",params![format!("large-{n}"),now(),large])?;}
        tx.commit()?;Ok(())
    }).await.unwrap();
    assert!(f.prune_runs(Actor::user("owner".into()), 7).await.unwrap() > 0);
    f.auth
        .db
        .run(|db| {
            let (count, size): (i64, i64) = db.query_row(
                "SELECT count(*),sum(length(stdout_gzip)) FROM farm_runs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            assert!(count > 0);
            assert!(size <= 64 * 1024 * 1024);
            Ok(())
        })
        .await
        .unwrap();
    drop(f);
    std::fs::remove_dir_all(dir).unwrap();
}
