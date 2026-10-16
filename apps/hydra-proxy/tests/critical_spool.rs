#[path = "../src/telemetry/critical.rs"]
mod critical;
use critical::CriticalSpool;
use hydra_wire::v1::{self, event::Payload};
use std::{path::PathBuf, sync::Arc, time::Duration};
fn path() -> PathBuf {
    std::env::temp_dir().join(format!("hydra-critical-{}", uuid::Uuid::new_v4()))
}
fn flag(value: &str) -> v1::Event {
    v1::Event {
        event_id: uuid::Uuid::new_v4().to_string(),
        payload: Some(Payload::Flag(v1::FlagFinding {
            flag_value: value.into(),
            ..Default::default()
        })),
        ..Default::default()
    }
}
async fn wait_events(spool: &Arc<CriticalSpool>, count: usize) -> Vec<v1::EventBatch> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let batches = spool.replay("", 0, 256).await.unwrap();
            if batches.len() >= count {
                break batches;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn persisted_flag_replays_after_restart_and_ack_survives_restart() {
    let path = path();
    let spool = CriticalSpool::open(path.clone()).unwrap();
    let epoch = spool.epoch().to_owned();
    assert!(spool.try_emit(flag("FLAG{durable}")));
    let batches = wait_events(&spool, 1).await;
    assert!(batches[0].critical);
    assert_eq!(batches[0].first_sequence, 1);
    drop(spool);
    let spool = CriticalSpool::open(path.clone()).unwrap();
    assert_eq!(spool.epoch(), epoch);
    assert_eq!(spool.replay(&epoch, 0, 32).await.unwrap(), batches);
    assert_eq!(spool.ack(&epoch, 1).await.unwrap(), 1);
    drop(spool);
    let spool = CriticalSpool::open(path.clone()).unwrap();
    assert!(spool.replay(&epoch, 0, 32).await.unwrap().is_empty());
    assert_eq!(spool.ack(&epoch, 1).await.unwrap(), 1);
    assert!(spool.try_emit(flag("FLAG{second}")));
    assert_eq!(wait_events(&spool, 1).await[0].first_sequence, 2);
    drop(spool);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn rejects_wrong_epoch_and_ack_beyond_committed_sequence() {
    let path = path();
    let spool = CriticalSpool::open(path.clone()).unwrap();
    assert!(spool.try_emit(flag("FLAG{x}")));
    wait_events(&spool, 1).await;
    assert!(spool.ack("wrong-epoch", 1).await.is_err());
    assert!(spool.ack(spool.epoch(), 2).await.is_err());
    assert_eq!(spool.replay("old-epoch", 999, 32).await.unwrap().len(), 1);
    drop(spool);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn oversized_event_and_noncritical_payload_are_rejected_and_loss_is_replayed() {
    let path = path();
    let spool = CriticalSpool::open(path.clone()).unwrap();
    assert!(!spool.try_emit(flag(&"x".repeat(70_000))));
    assert!(!spool.try_emit(v1::Event::default()));
    assert!(spool.dropped() >= 2);
    let batches = wait_events(&spool, 1).await;
    assert!(matches!(
        batches[0].events[0].payload,
        Some(Payload::Loss(_))
    ));
    drop(spool);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn quota_is_bounded_overflow_is_reported_and_ack_frees_space() {
    let path = path();
    let spool = CriticalSpool::open(path.clone()).unwrap();
    for _ in 0..70 {
        spool.try_emit(flag(&"x".repeat(60_000)));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(spool.dropped() > 0);
    assert!(spool.bytes() <= 2 * 1024 * 1024);
    let disk_bytes: u64 = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(disk_bytes <= 4 * 1024 * 1024);
    let batches = spool.replay("", 0, 256).await.unwrap();
    assert!(!batches.is_empty());
    assert!(
        batches
            .iter()
            .any(|b| matches!(b.events[0].payload, Some(Payload::Loss(_))))
    );
    let last = batches.last().unwrap().first_sequence;
    spool.ack(spool.epoch(), last).await.unwrap();
    assert!(spool.try_emit(flag("FLAG{space-reclaimed}")));
    let batches = wait_events(&spool, 1).await;
    assert!(batches.iter().any(|b| matches!(&b.events[0].payload, Some(Payload::Flag(f)) if f.flag_value == "FLAG{space-reclaimed}")));
    drop(spool);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn failed_disk_write_is_not_replayed_as_durable_and_recovery_reports_loss() {
    let path = path();
    let spool = CriticalSpool::open(path.clone()).unwrap();
    std::fs::create_dir(path.join("events.tmp")).unwrap();
    assert!(spool.try_emit(flag("FLAG{not-committed}")));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(spool.replay("", 0, 32).await.unwrap().is_empty());
    assert!(spool.dropped() > 0);
    std::fs::remove_dir(path.join("events.tmp")).unwrap();
    let batches = wait_events(&spool, 1).await;
    assert!(matches!(
        batches[0].events[0].payload,
        Some(Payload::Loss(_))
    ));
    assert_eq!(batches[0].first_sequence, 1);
    drop(spool);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn corrupt_spool_fails_closed_without_resetting_epoch_or_events() {
    let path = path();
    let spool = CriticalSpool::open(path.clone()).unwrap();
    drop(spool);
    std::fs::write(path.join("events.bin"), b"corrupt").unwrap();
    assert!(CriticalSpool::open(path.clone()).is_err());
    assert_eq!(std::fs::read(path.join("events.bin")).unwrap(), b"corrupt");
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn failed_ack_write_preserves_unacknowledged_events_for_restart() {
    let path = path();
    let spool = CriticalSpool::open(path.clone()).unwrap();
    assert!(spool.try_emit(flag("FLAG{keep-until-durable-ack}")));
    let batches = wait_events(&spool, 1).await;
    let epoch = spool.epoch().to_owned();
    std::fs::create_dir(path.join("events.tmp")).unwrap();
    assert!(spool.ack(&epoch, 1).await.is_err());
    assert_eq!(spool.replay(&epoch, 0, 32).await.unwrap(), batches);
    std::fs::remove_dir(path.join("events.tmp")).unwrap();
    drop(spool);
    let spool = CriticalSpool::open(path.clone()).unwrap();
    assert_eq!(spool.replay(&epoch, 0, 32).await.unwrap(), batches);
    drop(spool);
    std::fs::remove_dir_all(path).unwrap();
}
