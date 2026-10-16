use super::*;
use crate::config::Limits;
use tokio_util::sync::CancellationToken;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("hydra-state-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn start(path: PathBuf) -> Arc<State> {
    let telemetry = Telemetry::new(4);
    let manager = Manager::new(
        Limits::default(),
        telemetry.clone(),
        CancellationToken::new(),
    );
    State::start(path, ServiceSnapshot::default(), manager, telemetry)
        .await
        .unwrap()
}
fn snapshot(revision: u64) -> ServiceSnapshot {
    ServiceSnapshot {
        revision,
        services: Vec::new(),
    }
}

#[tokio::test]
async fn admitted_commit_survives_caller_cancellation_and_bounds_admission() {
    let directory = Directory::new();
    let state = start(directory.0.join("saved.json")).await;
    let guard = state.current.lock().await;
    let callers: Vec<_> = (0..16)
        .map(|_| {
            let state = state.clone();
            tokio::spawn(async move { state.apply(snapshot(1), Provenance::default()).await })
        })
        .collect();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.commits.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        state
            .apply(snapshot(2), Provenance::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("capacity")
    );
    for caller in callers {
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
    }
    drop(guard);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.commits.available_permits() != 16 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(state.revision().await, 1);
    let saved: Saved =
        serde_json::from_slice(&tokio::fs::read(&state.path).await.unwrap()).unwrap();
    assert_eq!(saved.snapshot.revision, 1);
    state.stop().await;
}

#[tokio::test]
async fn failed_persistence_preserves_revision_and_replay_checks_content() {
    let directory = Directory::new();
    let state = start(directory.0.join("saved.json")).await;
    assert_eq!(
        state
            .apply(snapshot(1), Provenance::default())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        state
            .apply(snapshot(1), Provenance::default())
            .await
            .unwrap(),
        1
    );
    let mut conflicting = snapshot(1);
    conflicting.services.push(hydra_domain::Service {
        id: "disabled".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:23001".parse().unwrap(),
        upstream_address: "127.0.0.1:23002".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: false,
    });
    assert!(
        state
            .apply(conflicting, Provenance::default())
            .await
            .is_err()
    );
    assert!(
        state
            .apply(snapshot(0), Provenance::default())
            .await
            .is_err()
    );
    // A directory at the destination makes rename fail even when run as root.
    tokio::fs::remove_file(&state.path).await.unwrap();
    tokio::fs::create_dir(&state.path).await.unwrap();
    assert!(
        state
            .apply(snapshot(2), Provenance::default())
            .await
            .is_err()
    );
    assert_eq!(state.revision().await, 1);
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    state.stop().await;
}

#[tokio::test]
async fn stop_drains_admitted_commit_and_rejects_later_work() {
    let directory = Directory::new();
    let state = start(directory.0.join("saved.json")).await;
    let guard = state.current.lock().await;
    let caller_state = state.clone();
    let caller =
        tokio::spawn(async move { caller_state.apply(snapshot(1), Provenance::default()).await });
    while state.commits.available_permits() == 16 {
        tokio::task::yield_now().await;
    }
    let stop_state = state.clone();
    let stop = tokio::spawn(async move { stop_state.stop().await });
    // acquire_many reserves the remaining permits while it waits for the
    // admitted commit, closing admission without cancelling that commit.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.commits.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        state
            .apply(snapshot(2), Provenance::default())
            .await
            .is_err()
    );
    assert!(!stop.is_finished());
    drop(guard);
    assert_eq!(caller.await.unwrap().unwrap(), 1);
    tokio::time::timeout(std::time::Duration::from_secs(2), stop)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.revision().await, 1);
    assert!(
        state
            .apply(snapshot(2), Provenance::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn combined_snapshot_moves_dependent_rules_and_service_together() {
    let directory = Directory::new();
    let state = start(directory.0.join("saved.json")).await;
    let service = hydra_domain::Service {
        id: "service".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:24001".parse().unwrap(),
        upstream_address: "127.0.0.1:24002".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: false,
    };
    let rule = hydra_domain::Rule {
        id: "rule".into(),
        service_id: service.id.clone(),
        name: "log".into(),
        description: String::new(),
        enabled: true,
        mode: hydra_domain::Mode::Enforce,
        priority: 1,
        expression: hydra_domain::Expression::Predicate {
            field: "network.src_ip".into(),
            op: hydra_domain::Comparison::Eq,
            value: hydra_domain::FieldValue::String("127.0.0.1".into()),
        },
        action: hydra_domain::Action::Log,
        backend: "userspace".into(),
        expires_at_ms: None,
    };
    state
        .apply_config(
            ServiceSnapshot {
                revision: 1,
                services: vec![service.clone()],
            },
            hydra_domain::RuleSnapshot {
                revision: 1,
                rules: vec![rule],
            },
            Provenance::default(),
        )
        .await
        .unwrap();
    let mut udp = service;
    udp.transport = hydra_domain::Transport::Udp;
    state
        .apply(
            ServiceSnapshot {
                revision: 2,
                services: vec![udp.clone()],
            },
            Provenance::default(),
        )
        .await
        .unwrap();
    let fields = crate::inspection::network_fields(
        &udp,
        "127.0.0.1:1000".parse().unwrap(),
        "client_to_server",
    );
    let rules = state.telemetry.rules.read().unwrap().clone();
    assert_eq!(
        rules
            .evaluate(&hydra_domain::EvaluationContext {
                service_id: "service",
                transport: hydra_domain::Transport::Udp,
                fields: &fields,
                now_ms: 0,
                safe_http: false
            })
            .hits
            .len(),
        1
    );
    assert!(
        state
            .apply_config(
                ServiceSnapshot {
                    revision: 3,
                    services: vec![]
                },
                hydra_domain::RuleSnapshot {
                    revision: 3,
                    rules: vec![]
                },
                Provenance::default()
            )
            .await
            .is_ok()
    );
    assert_eq!(state.rules_revision().await, 3);
    state.stop().await;
}
