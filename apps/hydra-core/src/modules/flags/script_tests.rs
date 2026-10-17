//! Script submitter integration using an executable fake uv; no network effects.
use super::{script_config::SubmitterConfig, *};
use crate::infrastructure::{crypto::Crypto, db::Database, operations::HostRuntimeSettings};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
};
use tokio_util::sync::CancellationToken;

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    flags: Flags,
    runtime: HostRuntimeSettings,
    _directory: Directory,
}
async fn fixture(body: &[u8]) -> Fixture {
    let directory =
        std::env::temp_dir().join(format!("hydra-script-integration-{}", uuid::Uuid::new_v4()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    let uv = directory.join("uv");
    let mut executable = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&uv)
        .unwrap();
    executable.write_all(b"#!/bin/bash\nwhile (($#)); do if [[ $1 == python ]]; then shift; break; fi; shift; done\n[[ $1 == -u ]] && shift\nshift\n").unwrap();
    executable.write_all(body).unwrap();
    drop(executable);
    let db = Database::open(directory.join("db")).await.unwrap();
    db.run(|db| { db.execute_batch("INSERT INTO farm_targets VALUES('allowed','allowed','127.0.0.1',80,'tcp',1,1,0,0,'initiator'); INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at) VALUES('farm-run','exploit','v1','{}','[\"allowed\"]','initiator','running',0);")?; Ok(()) }).await.unwrap();
    let runtime = HostRuntimeSettings {
        uv,
        work_dir: directory.join("runtime"),
    };
    let auth = AuthState::new(
        db,
        Crypto::load(&directory.join("key")).unwrap(),
        "https://localhost".into(),
    );
    let flags = Flags::open(
        auth,
        Settings {
            pattern: Some("FLAG[0-9]+".into()),
            script_runtime: Some(runtime.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    Fixture {
        flags,
        runtime,
        _directory: Directory(directory),
    }
}
fn config(enabled: bool) -> SubmitterConfig {
    SubmitterConfig {
        revision: 0,
        enabled,
        flag_pattern: "FLAG[0-9]+".into(),
        python_packages: vec![],
        source: "print('submit')\n".into(),
        timeout_seconds: 3,
    }
}
fn actor() -> Actor {
    Actor::user("initiator".into())
}
async fn configure(f: &Flags, enabled: bool) {
    f.configure_submitter(actor(), config(enabled))
        .await
        .unwrap();
}
async fn capture(f: &Flags, n: usize) -> String {
    f.record_farm_captured(
        format!("FLAG{n}"),
        "allowed".into(),
        "farm-run".into(),
        "initiator".into(),
    )
    .await
    .unwrap()
}
async fn due(f: &Flags) {
    f.auth
        .db
        .run(|db| {
            db.execute(
                "UPDATE flag_submitter_clock SET next_batch_at=0 WHERE id=1",
                [],
            )?;
            db.execute(
                "UPDATE submissions SET next_attempt=0 WHERE state='retry'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}
async fn counts(f: &Flags) -> (i64, i64, i64) {
    f.auth
        .db
        .run(|db| {
            Ok((
                db.query_row(
                    "SELECT count(*) FROM submissions WHERE state='accepted'",
                    [],
                    |r| r.get(0),
                )?,
                db.query_row(
                    "SELECT count(*) FROM submissions WHERE state IN ('queued','retry')",
                    [],
                    |r| r.get(0),
                )?,
                db.query_row("SELECT count(*) FROM flag_submitter_batches", [], |r| {
                    r.get(0)
                })?,
            ))
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn configure_checks_revisions_validation_and_attribution() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    let mut invalid = config(true);
    invalid.flag_pattern = ".*".into();
    assert!(f.configure_submitter(actor(), invalid).await.is_err());
    let mut invalid = config(true);
    invalid.python_packages = vec!["https://example.invalid/package.whl".into()];
    assert!(f.configure_submitter(actor(), invalid).await.is_err());
    let mut invalid = config(true);
    invalid.source.clear();
    assert!(f.configure_submitter(actor(), invalid).await.is_err());
    configure(f, true).await;
    assert!(f.configure_submitter(actor(), config(true)).await.is_err());
    let view = f.submitter_view(actor()).await.unwrap();
    assert_eq!(view.config.revision, 1);
    assert!(view.config.enabled && view.runtime_available);
    assert_eq!(view.interval_seconds, 30);
    assert!(view.next_batch_at >= crate::modules::auth::now() + 29);
    let count = f.auth.db.run(|db| Ok(db.query_row("SELECT count(*) FROM audit WHERE action='submitter.configure' AND actor_id='initiator'", [], |r| r.get::<_,i64>(0))?)).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn configuration_and_regex_survive_reopen() {
    let fixture = fixture(b"exit 0\n").await;
    let mut selected = config(false);
    selected.flag_pattern = "ALT[0-9]+".into();
    selected.python_packages = vec!["requests".into(), "pwntools==4.14.1".into()];
    fixture
        .flags
        .configure_submitter(actor(), selected)
        .await
        .unwrap();
    let reopened = Flags::open(
        fixture.flags.auth.clone(),
        Settings {
            pattern: Some("FLAG[0-9]+".into()),
            script_runtime: Some(fixture.runtime.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let view = reopened.submitter_view(actor()).await.unwrap();
    assert_eq!(view.config.flag_pattern, "ALT[0-9]+");
    assert_eq!(
        view.config.python_packages,
        ["requests", "pwntools==4.14.1"]
    );
    assert_eq!(view.config.revision, 1);
    assert!(!view.config.enabled);
    assert_eq!(reopened.extract(b"ALT123 FLAG456").unwrap(), ["ALT123"]);
}

#[tokio::test]
async fn captured_farm_flags_use_bounded_batches_per_window_and_gzip_redacts_output() {
    let fixture =
        fixture(b"printf 'count=%s\\n' \"$#\"\nprintf '%s\\n' \"$@\"\nprintf '%s\\n' \"$@\" >&2\n")
            .await;
    let f = &fixture.flags;
    configure(f, true).await;
    for n in 1000..1260 {
        capture(f, n).await;
    }
    assert_eq!(counts(f).await, (0, 260, 0));
    assert!(!f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    due(f).await;
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    assert_eq!(counts(f).await, (260, 0, 2));
    let (total, maximum) = f
        .auth
        .db
        .run(|db| {
            Ok(db.query_row(
                "SELECT sum(flag_count),max(flag_count) FROM flag_submitter_batches",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(total, 260);
    assert!((1..=256).contains(&maximum));
    assert!(!f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    let view = f.submitter_view(actor()).await.unwrap();
    let batch = view.last_batch.unwrap();
    assert!(batch.flag_count <= 256);
    assert!(
        batch
            .stdout
            .contains(&format!("count={}", batch.flag_count))
    );
    assert!(!batch.stdout.contains("FLAG") && !batch.stderr.contains("FLAG"));
    let (out, err) = f.auth.db.run(|db| Ok(db.query_row("SELECT stdout_gzip,stderr_gzip FROM flag_submitter_batches ORDER BY rowid DESC LIMIT 1", [], |r| Ok((r.get::<_,Vec<u8>>(0)?, r.get::<_,Vec<u8>>(1)?)))?)).await.unwrap();
    assert!(out.starts_with(&[0x1f, 0x8b]) && err.starts_with(&[0x1f, 0x8b]));
    assert!(
        !String::from_utf8_lossy(&crate::modules::farm::runtimes::decode_log(&out).unwrap())
            .contains("FLAG")
    );
}

#[tokio::test]
async fn failed_attempt_retries_only_when_next_collection_window_is_due() {
    let fixture = fixture(b"printf '%s\\n' \"$@\" >&2\nexit 7\n").await;
    let f = &fixture.flags;
    configure(f, true).await;
    capture(f, 123).await;
    due(f).await;
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    let attempts = f
        .auth
        .db
        .run(|db| {
            Ok(db.query_row("SELECT attempts FROM submissions", [], |r| {
                r.get::<_, i64>(0)
            })?)
        })
        .await
        .unwrap();
    assert_eq!(attempts, 1);
    assert_eq!(counts(f).await, (0, 1, 1));
    assert!(!f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    due(f).await;
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    let attempts = f
        .auth
        .db
        .run(|db| {
            Ok(db.query_row("SELECT attempts FROM submissions", [], |r| {
                r.get::<_, i64>(0)
            })?)
        })
        .await
        .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(counts(f).await, (0, 1, 2));
    let batch = f.submitter_view(actor()).await.unwrap().last_batch.unwrap();
    assert_eq!(batch.exit_code, Some(7));
    assert!(!batch.stderr.contains("FLAG123"));
}

#[tokio::test]
async fn paused_submitter_retains_queue_without_starting_a_child() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    configure(f, false).await;
    capture(f, 123).await;
    due(f).await;
    assert!(!f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    assert_eq!(counts(f).await, (0, 1, 0));
    assert!(!fixture.runtime.work_dir.exists());
}

#[tokio::test]
async fn own_exposed_quarantine_is_preserved_by_script_dispatch() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    configure(f, true).await;
    capture(f, 123).await;
    f.ingest_proxy(&hydra_wire::v1::EventBatch {
        proxy_epoch_id: "epoch".into(),
        critical: true,
        events: vec![hydra_wire::v1::Event {
            event_id: "defensive-event".into(),
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
    })
    .await
    .unwrap();
    due(f).await;
    f.dispatch_cycle(CancellationToken::new()).await.unwrap();
    let state = f
        .auth
        .db
        .run(|db| {
            Ok(db.query_row("SELECT state FROM submissions", [], |r| {
                r.get::<_, String>(0)
            })?)
        })
        .await
        .unwrap();
    assert_eq!(state, "quarantined");
    assert_eq!(counts(f).await, (0, 0, 0));
    assert!(!fixture.runtime.work_dir.exists());
}

#[tokio::test]
async fn pausing_during_first_child_leaves_remaining_chunk_queued() {
    let fixture = fixture(b"sleep 0.3\nprintf 'done\\n'\n").await;
    let f = &fixture.flags;
    configure(f, true).await;
    for n in 0..260 {
        capture(f, n).await;
    }
    due(f).await;
    let worker = f.clone();
    let task = tokio::spawn(async move {
        worker
            .dispatch_cycle(CancellationToken::new())
            .await
            .unwrap()
    });
    loop {
        let running = f
            .auth
            .db
            .run(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM flag_submitter_batches WHERE status='running'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        if running > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let mut paused = config(false);
    paused.revision = 1;
    f.configure_submitter(actor(), paused).await.unwrap();
    assert!(task.await.unwrap());
    let (accepted, queued, batches) = counts(f).await;
    assert_eq!((accepted, queued, batches), (240, 20, 1));
}

#[tokio::test]
async fn interrupted_claims_recover_and_expired_batch_logs_rotate() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    configure(f, true).await;
    capture(f, 17).await;
    f.auth.db.run(|db| {
        db.execute("UPDATE submissions SET state='inflight',attempts=1",[])?;
        db.execute_batch("INSERT INTO flag_submitter_batches(id,revision,flag_count,status,started_at) VALUES('running',1,1,'running',0),('old',1,1,'failed',0);")?;
        Ok(())
    }).await.unwrap();
    f.recover_submissions().await.unwrap();
    let (state, attempts, next) = f
        .auth
        .db
        .run(|db| {
            Ok(db.query_row(
                "SELECT state,attempts,next_attempt FROM submissions",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u32>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )?)
        })
        .await
        .unwrap();
    assert_eq!((state.as_str(), attempts), ("retry", 1));
    assert!(next >= crate::modules::auth::now() + 29);
    let status = f
        .auth
        .db
        .run(|db| {
            Ok(db.query_row(
                "SELECT status FROM flag_submitter_batches WHERE id='running'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(status, "interrupted");
    f.rotate_submitter_logs().await.unwrap();
    assert_eq!(counts(f).await, (0, 1, 0));
}
