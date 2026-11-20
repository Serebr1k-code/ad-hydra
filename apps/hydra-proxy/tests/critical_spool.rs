#[path = "../src/telemetry/critical.rs"]
mod critical;
use critical::CriticalSpool;
use hydra_wire::v1::{self, event::Payload};
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
    time::Duration,
};
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
async fn wait_control_queue(spool: &Arc<CriticalSpool>, count: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while spool.control_queue_len() != count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
struct WorkerRelease {
    gate: Arc<Barrier>,
    released: bool,
}
impl WorkerRelease {
    fn release(&mut self) {
        if !self.released {
            self.released = true;
            self.gate.wait();
        }
    }
}
impl Drop for WorkerRelease {
    fn drop(&mut self) {
        self.release();
    }
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

#[tokio::test]
async fn saturated_emit_queue_does_not_block_replay_or_ack() {
    let path = path();
    let gate = Arc::new(Barrier::new(2));
    let spool = CriticalSpool::open_paused(path.clone(), gate.clone()).unwrap();
    let mut release = WorkerRelease {
        gate,
        released: false,
    };
    let epoch = spool.epoch().to_owned();

    // The worker cannot consume while both bounded queues are filled. Admission is
    // deterministic, with no timing window between producing and saturation.
    for index in 0..critical::QUEUE_LIMIT {
        assert!(spool.try_emit(flag(&format!("FLAG{{queued-{index}}}"))));
    }
    assert!(!spool.try_emit(flag("FLAG{overflow}")));

    let replay_spool = spool.clone();
    let replay = tokio::spawn(async move { replay_spool.replay(&epoch, 0, 8).await });
    wait_control_queue(&spool, 1).await;
    let ack_epoch = spool.epoch().to_owned();
    let ack_spool = spool.clone();
    let ack = tokio::spawn(async move { ack_spool.ack(&ack_epoch, 0).await });
    wait_control_queue(&spool, 2).await;

    release.release();
    assert!(replay.await.unwrap().unwrap().is_empty());
    assert_eq!(ack.await.unwrap().unwrap(), 0);
    assert_eq!(
        wait_events(&spool, critical::QUEUE_LIMIT + 1).await.len(),
        critical::QUEUE_LIMIT + 1
    );

    drop(spool);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn control_backpressure_stays_bounded_when_waiting_calls_are_cancelled() {
    let path = path();
    let gate = Arc::new(Barrier::new(2));
    let spool = CriticalSpool::open_paused(path.clone(), gate.clone()).unwrap();
    let mut release = WorkerRelease {
        gate,
        released: false,
    };
    let epoch = spool.epoch().to_owned();

    let pending = (0..critical::CONTROL_QUEUE_LIMIT)
        .map(|_| {
            let spool = spool.clone();
            let epoch = epoch.clone();
            tokio::spawn(async move { spool.replay(&epoch, 0, 1).await })
        })
        .collect::<Vec<_>>();
    wait_control_queue(&spool, critical::CONTROL_QUEUE_LIMIT).await;

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let waiting_spool = spool.clone();
    let waiting_epoch = epoch.clone();
    let waiting = tokio::spawn(async move {
        let _ = started_tx.send(());
        waiting_spool.replay(&waiting_epoch, 0, 1).await
    });
    started_rx.await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(spool.control_queue_len(), critical::CONTROL_QUEUE_LIMIT);
    assert!(!waiting.is_finished());
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    assert_eq!(spool.control_queue_len(), critical::CONTROL_QUEUE_LIMIT);

    release.release();
    for call in pending {
        assert!(call.await.unwrap().unwrap().is_empty());
    }

    drop(spool);
    std::fs::remove_dir_all(path).unwrap();
}
