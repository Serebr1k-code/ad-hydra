//! Script submitter integration using an executable fake uv; no network effects.
use super::{script_config::SubmitterConfig, *};
use crate::infrastructure::{crypto::Crypto, db::Database, operations::HostRuntimeSettings};
use rusqlite::params;
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
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
        mode: "custom".into(),
        batch_size: 240,
        revision: 0,
        enabled,
        flag_pattern: "FLAG[0-9]+".into(),
        python_packages: vec![],
        source: "print('submit')\n".into(),
        forcad_ip: String::new(),
        forcad_port: 8080,
        team_token: String::new(),
        team_token_configured: false,
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
    let mut invalid = config(false);
    invalid.mode = "forcad".into();
    invalid.batch_size = 100;
    invalid.forcad_ip = "https://192.0.2.1".into();
    assert!(
        f.configure_submitter(actor(), invalid.clone())
            .await
            .is_err()
    );
    invalid.forcad_ip = "192.0.2.1".into();
    invalid.team_token = "bad\r\ntoken".into();
    assert!(f.configure_submitter(actor(), invalid).await.is_err());
    configure(f, true).await;
    assert!(f.configure_submitter(actor(), config(true)).await.is_err());
    let view = f.submitter_view(actor()).await.unwrap();
    assert_eq!(view.config.revision, 1);
    assert!(view.config.enabled && view.runtime_available);
    assert_eq!(view.interval_seconds, 1);
    assert!(view.next_batch_at <= crate::modules::auth::now() + 1);
    let count = f.auth.db.run(|db| Ok(db.query_row("SELECT count(*) FROM audit WHERE action='submitter.configure' AND actor_id='initiator'", [], |r| r.get::<_,i64>(0))?)).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn first_install_uses_paused_forcad_preset_and_never_returns_token() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    let fresh = f.submitter_view(actor()).await.unwrap().config;
    assert_eq!(fresh.mode, "forcad");
    assert_eq!(fresh.forcad_port, 8080);
    assert_eq!(fresh.flag_pattern, "FLAG[0-9]+");
    assert!(!fresh.enabled && !fresh.team_token_configured);
    assert!(
        serde_json::to_value(&fresh)
            .unwrap()
            .get("team_token")
            .is_none()
    );
    let mut forcad = config(false);
    forcad.mode = "forcad".into();
    forcad.batch_size = 100;
    forcad.forcad_ip = "127.0.0.1".into();
    forcad.team_token = "forcad-secret-test-token".into();
    f.configure_submitter(actor(), forcad).await.unwrap();
    let saved = f.submitter_view(actor()).await.unwrap().config;
    assert!(saved.team_token_configured);
    let serialized = serde_json::to_string(&saved).unwrap();
    assert!(!serialized.contains("forcad-secret-test-token"));
    assert!(!serialized.contains("team_token\""));
    let audit = f
        .auth
        .db
        .run(|db| {
            Ok(db.query_row(
                "SELECT group_concat(metadata) FROM audit WHERE target='python-submitter'",
                [],
                |r| r.get::<_, Option<String>>(0),
            )?)
        })
        .await
        .unwrap()
        .unwrap_or_default();
    assert!(!audit.contains("forcad-secret-test-token"));

    // A blank write-only field preserves the configured secret, so an operator
    // can edit other settings without reading or resending the token.
    let mut update = saved;
    update.revision = 1;
    update.enabled = true;
    update.team_token.clear();
    update.team_token_configured = false;
    f.configure_submitter(actor(), update).await.unwrap();
    let enabled = f.submitter_view(actor()).await.unwrap().config;
    assert!(enabled.enabled && enabled.team_token_configured);
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
async fn captured_farm_flags_preserve_exact_batch_input_and_unredacted_zstd_output() {
    let fixture =
        fixture(b"printf 'count=%s\\n' \"$#\"\nprintf '%s\\n' \"$@\"\nprintf '%s\\n' \"$@\" >&2\n")
            .await;
    let f = &fixture.flags;
    configure(f, true).await;
    for n in 1000..1260 {
        capture(f, n).await;
    }
    assert_eq!(counts(f).await, (0, 260, 0));
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    assert_eq!(counts(f).await, (240, 20, 1));
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
    let input = batch.input_flags.as_ref().unwrap();
    assert_eq!(input.len(), batch.flag_count as usize);
    assert!(
        input
            .iter()
            .all(|flag| batch.stdout.contains(flag) && batch.stderr.contains(flag))
    );
    assert!(!batch.stdout.contains("[redacted flag]"));
    let (out, err) = f.auth.db.run(|db| Ok(db.query_row("SELECT stdout_gzip,stderr_gzip FROM flag_submitter_batches ORDER BY rowid DESC LIMIT 1", [], |r| Ok((r.get::<_,Vec<u8>>(0)?, r.get::<_,Vec<u8>>(1)?)))?)).await.unwrap();
    assert!(
        out.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) && err.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
    );
    assert!(
        String::from_utf8_lossy(&crate::modules::farm::runtimes::decode_log(&out).unwrap())
            .contains("FLAG")
    );
}

#[tokio::test]
async fn forcad_http_uploads_are_capped_and_apply_mixed_verdicts_per_flag() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let fixture = fixture(b"# replaced below\n").await;
    let uv_path = fixture._directory.0.join("uv");
    std::fs::write(&uv_path, b"#!/bin/sh\nshift 2\n[ \"$1\" = python ] || exit 90\nshift\nexec /usr/bin/python3 \"$@\"\n").unwrap();
    std::fs::set_permissions(&uv_path, std::fs::Permissions::from_mode(0o700)).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sizes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<usize>::new()));
    let server_received = received.clone();
    let server_sizes = sizes.clone();
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            let body_start = loop {
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break pos + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..body_start]);
            assert!(headers.starts_with("PUT /flags/ HTTP/1.1\r\n"));
            assert!(headers.lines().any(|line| line.eq_ignore_ascii_case("X-Team-Token: secret-forcad-team-token")));
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                })
                .unwrap();
            while request.len() < body_start + content_length {
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
            }
            let flags: Vec<String> =
                serde_json::from_slice(&request[body_start..body_start + content_length]).unwrap();
            assert!(flags.len() <= 100);
            server_sizes.lock().unwrap().push(flags.len());
            server_received
                .lock()
                .unwrap()
                .extend(flags.iter().cloned());
            let results: Vec<serde_json::Value> = flags
                .iter()
                .map(|flag| {
                    let message = match flag.as_str() {
                        "FLAG1000" => "Flag accepted! Earned points!",
                        "FLAG1001" => "Flag already stolen",
                        "FLAG1002" => "Flag is your own",
                        "FLAG1003" => "Flag is invalid or too old.",
                        "FLAG1004" => "Cannot submit flags while service is down",
                        _ => "Flag accepted! Earned points!",
                    };
                    serde_json::json!({"flag":flag,"msg":format!("[{flag}] {message}")})
                })
                .collect();
            let body = serde_json::to_vec(&results).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
    });

    let f = &fixture.flags;
    let mut selected = config(false);
    selected.mode = "forcad".into();
    selected.batch_size = 100;
    selected.forcad_ip = address.ip().to_string();
    selected.forcad_port = address.port();
    selected.team_token = "secret-forcad-team-token".into();
    f.configure_submitter(actor(), selected).await.unwrap();
    for n in 1000..1250 {
        capture(f, n).await;
    }
    f.request_manual_run(actor(), 250).await.unwrap();
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    server.await.unwrap();

    let batch_sizes = sizes.lock().unwrap().clone();
    assert_eq!(batch_sizes, [100, 100, 50]);
    assert!(batch_sizes.iter().all(|size| *size <= 100));
    let uploaded = received.lock().unwrap().clone();
    let expected: Vec<String> = (1000..1250).map(|n| format!("FLAG{n}")).collect();
    assert_eq!(uploaded, expected);
    let states = f.auth.db.run(|db| Ok(db.prepare("SELECT f.value,s.state FROM submissions s JOIN flags f ON f.id=s.flag_id ORDER BY f.value")?.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?)).await.unwrap();
    let state_for = |flag: &str| {
        states
            .iter()
            .find(|(value, _)| value == flag)
            .map(|(_, state)| state.as_str())
            .unwrap()
    };
    assert_eq!(state_for("FLAG1000"), "accepted");
    assert_eq!(state_for("FLAG1001"), "accepted");
    assert_eq!(state_for("FLAG1002"), "failed");
    assert_eq!(state_for("FLAG1003"), "failed");
    assert_eq!(state_for("FLAG1004"), "retry");
    let (count, maximum, logs) = f
        .auth
        .db
        .run(|db| {
            let blobs = db
                .prepare("SELECT stdout_gzip,stderr_gzip FROM flag_submitter_batches")?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, Option<Vec<u8>>>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut text = String::new();
            for (stdout, stderr) in blobs {
                for blob in [stdout, stderr].into_iter().flatten() {
                    text.push_str(&String::from_utf8_lossy(&hydra_compression::decode(
                        &blob, 65536,
                    )?));
                }
            }
            Ok((
                db.query_row("SELECT count(*) FROM flag_submitter_batches", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                db.query_row(
                    "SELECT max(flag_count) FROM flag_submitter_batches",
                    [],
                    |row| row.get::<_, i64>(0),
                )?,
                text,
            ))
        })
        .await
        .unwrap();
    assert_eq!((count, maximum), (3, 100));
    assert!(!logs.contains("secret-forcad-team-token"));
}

#[tokio::test]
async fn forcad_invalid_team_token_keeps_flag_queued_for_retry() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fixture = fixture(b"# replaced below\n").await;
    let uv_path = fixture._directory.0.join("uv");
    std::fs::write(&uv_path, b"#!/bin/sh\nshift 2\n[ \"$1\" = python ] || exit 90\nshift\nexec /usr/bin/python3 \"$@\"\n").unwrap();
    std::fs::set_permissions(&uv_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        let body_start = loop {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&chunk[..count]);
            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..body_start]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            })
            .unwrap();
        while request.len() < body_start + content_length {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&chunk[..count]);
        }
        let body = br#"{"error":"Invalid team token."}"#;
        let header = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
    });
    let f = &fixture.flags;
    let mut selected = config(false);
    selected.mode = "forcad".into();
    selected.batch_size = 100;
    selected.forcad_ip = address.ip().to_string();
    selected.forcad_port = address.port();
    selected.team_token = "bad-token".into();
    f.configure_submitter(actor(), selected).await.unwrap();
    let flag_id = capture(f, 888).await;
    f.request_manual_run(actor(), 1).await.unwrap();
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    server.await.unwrap();
    let state = f
        .auth
        .db
        .run(move |db| {
            Ok(db.query_row(
                "SELECT state FROM submissions WHERE flag_id=?1",
                [flag_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(state, "retry");
    let view = f.submitter_view(actor()).await.unwrap();
    let batch = view.last_batch.unwrap();
    assert_eq!(batch.status, "retry");
    assert!(!batch.stderr.contains("bad-token"));
    assert!(!batch.stderr.contains("Traceback"));
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
    assert!(batch.stderr.contains("FLAG123"));
    assert_eq!(batch.input_flags.unwrap(), ["FLAG123"]);
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
async fn defensive_observation_does_not_store_values_or_transform_captures() {
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
    assert_eq!(state, "accepted");
    assert_eq!(counts(f).await, (1, 0, 1));
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

#[tokio::test]
async fn manual_run_works_paused_and_limits_total_and_batch_sizes() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    let mut chosen = config(false);
    chosen.batch_size = 1;
    f.configure_submitter(actor(), chosen).await.unwrap();
    for i in 1..=5 {
        capture(f, i).await;
    }
    assert!(!f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    let request = f.request_manual_run(actor(), 2).await.unwrap();
    assert!(f.request_manual_run(actor(), 2).await.is_err());
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    assert_eq!(counts(f).await, (2, 3, 2));
    let done = f.latest_manual().await.unwrap().unwrap();
    assert_eq!(done.id, request.id);
    assert_eq!(done.flag_count, 2);
    assert_eq!(done.status, "finished");
    assert!(!f.submitter_view(actor()).await.unwrap().config.enabled);
}

#[tokio::test]
async fn attempt_history_is_paginated_audited_and_preserves_retries() {
    let fixture = fixture(b"printf '%s\\n' \"$@\"\nexit 7\n").await;
    let f = &fixture.flags;
    configure(f, true).await;
    capture(f, 101).await;
    for _ in 0..2 {
        due(f).await;
        assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    }
    let newest = f.submitter_batches(actor(), 1, 0).await.unwrap();
    let older = f.submitter_batches(actor(), 1, 1).await.unwrap();
    assert_eq!(newest.total, 2);
    assert_eq!(older.items.len(), 1);
    assert_ne!(newest.items[0].id, older.items[0].id);
    for page in [newest, older] {
        let batch = f
            .submitter_batch(actor(), page.items[0].id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch.input_flags.unwrap(), ["FLAG101"]);
        assert_eq!(batch.exit_code, Some(7));
        assert!(batch.stdout.contains("FLAG101"));
    }
    assert!(f.submitter_batches(actor(), 101, 0).await.is_err());
    assert!(
        f.submitter_batch(actor(), uuid::Uuid::new_v4().to_string())
            .await
            .unwrap()
            .is_none()
    );
    let reads = f.auth.db.run(|db| {
        let count = db.query_row("SELECT count(*) FROM audit WHERE action IN ('submitter.batches.read','submitter.batch.read') AND actor_id='initiator'", [], |r| r.get::<_,i64>(0))?;
        let payloads = db.prepare("SELECT metadata FROM audit")?.query_map([], |r| r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        assert!(payloads.iter().all(|value| !value.contains("FLAG101")));
        Ok(count)
    }).await.unwrap();
    assert_eq!(reads, 5);
}

#[tokio::test]
async fn legacy_input_stays_absent_and_new_inputs_survive_reopen() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    configure(f, true).await;
    capture(f, 42).await;
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    let batch = f.submitter_view(actor()).await.unwrap().last_batch.unwrap();
    let reopened = Flags::open(
        f.auth.clone(),
        Settings {
            script_runtime: Some(fixture.runtime.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        reopened
            .submitter_batch(actor(), batch.id.clone())
            .await
            .unwrap()
            .unwrap()
            .input_flags
            .unwrap(),
        ["FLAG42"]
    );
    f.auth
        .db
        .run(move |db| {
            db.execute(
                "UPDATE flag_submitter_batches SET input_gzip=NULL WHERE id=?1",
                [batch.id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        f.submitter_view(actor())
            .await
            .unwrap()
            .last_batch
            .unwrap()
            .input_flags
            .is_none()
    );
}

#[tokio::test]
async fn retention_includes_input_bytes_and_retains_one_thousand_batches() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    f.auth.db.run(|db| {
        let tx = db.transaction()?;
        for index in 0..1001 {
            tx.execute("INSERT INTO flag_submitter_batches(id,revision,flag_count,status,started_at,input_gzip) VALUES(?1,1,1,'failed',?2,zeroblob(1))", params![format!("batch-{index}"), crate::modules::auth::now()])?;
        }
        tx.commit()?; Ok(())
    }).await.unwrap();
    f.rotate_submitter_logs().await.unwrap();
    assert_eq!(counts(f).await.2, 1000);
    f.auth.db.run(|db| {
        db.execute("DELETE FROM flag_submitter_batches", [])?;
        for index in 0..3 {
            db.execute("INSERT INTO flag_submitter_batches(id,revision,flag_count,status,started_at,input_gzip) VALUES(?1,1,1,'failed',?2,zeroblob(24000000))", params![format!("large-{index}"), crate::modules::auth::now()])?;
        }
        Ok(())
    }).await.unwrap();
    f.rotate_submitter_logs().await.unwrap();
    let ids = f
        .auth
        .db
        .run(|db| {
            Ok(db
                .prepare("SELECT id FROM flag_submitter_batches ORDER BY rowid")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .unwrap();
    assert_eq!(ids, ["large-1", "large-2"]);
}

#[tokio::test]
async fn input_decode_rejects_oversized_gzip_and_preserves_json_escaping() {
    let fixture = fixture(b"exit 0\n").await;
    let f = &fixture.flags;
    let id = uuid::Uuid::new_v4().to_string();
    let key = id.clone();
    f.auth.db.run(move |db| {
        let input = super::script_config::encode_compressed(&serde_json::to_vec(&vec!["quote\"\n\\flag"]).unwrap())?;
        db.execute("INSERT INTO flag_submitter_batches(id,revision,flag_count,status,started_at,input_gzip) VALUES(?1,1,1,'failed',?2,?3)", params![key,crate::modules::auth::now(),input])?;
        Ok(())
    }).await.unwrap();
    assert_eq!(
        f.submitter_batch(actor(), id.clone())
            .await
            .unwrap()
            .unwrap()
            .input_flags
            .unwrap(),
        ["quote\"\n\\flag"]
    );
    f.auth
        .db
        .run(move |db| {
            let input = super::script_config::encode_compressed(&vec![b'x'; 400000])?;
            db.execute(
                "UPDATE flag_submitter_batches SET input_gzip=?2 WHERE id=?1",
                params![id, input],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(f.submitter_view(actor()).await.is_err());
}

#[tokio::test]
async fn batch_history_endpoints_require_auth_and_return_original_diagnostics() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let fixture = fixture(b"printf '%s\\n' \"$@\"\nprintf '%s\\n' \"$@\" >&2\n").await;
    let f = &fixture.flags;
    configure(f, true).await;
    capture(f, 987).await;
    assert!(f.dispatch_cycle(CancellationToken::new()).await.unwrap());
    let batch = f.submitter_view(actor()).await.unwrap().last_batch.unwrap();
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
    let app = super::script_http::router(f.clone());
    for path in [
        "/api/v1/submitter/batches".to_string(),
        format!("/api/v1/submitter/batches/{}", batch.id),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(Request::get(&path).body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            401
        );
        let response = app
            .clone()
            .oneshot(
                Request::get(&path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        if path.ends_with("/batches") {
            assert_eq!(value["items"][0]["id"], batch.id);
            assert!(value["items"][0].get("stdout").is_none());
            assert!(value["items"][0].get("input_flags").is_none());
        } else {
            assert_eq!(value["input_flags"], serde_json::json!(["FLAG987"]));
            assert_eq!(value["stdout"], "FLAG987\n");
            assert_eq!(value["stderr"], "FLAG987\n");
        }
    }
    let response = app
        .oneshot(
            Request::get(format!(
                "/api/v1/submitter/batches/{}",
                uuid::Uuid::new_v4()
            ))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}
