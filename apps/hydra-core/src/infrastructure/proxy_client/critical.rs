//! Commit all critical side effects before advancing a bounded durable replay highwater.
use crate::modules::{flags::Flags, traffic::Traffic};
use hydra_wire::v1::{EventBatch, event::Payload};
use rusqlite::{OptionalExtension, params};

/// Success permits ACK through this batch's final sequence. Partial failures never
/// advance its receipt; replay repairs any individually committed side effects.
pub async fn commit(flags: &Flags, traffic: &Traffic, batch: &EventBatch) -> anyhow::Result<()> {
    anyhow::ensure!(
        batch.critical && !batch.events.is_empty() && batch.events.len() <= 256,
        "invalid critical batch"
    );
    anyhow::ensure!(
        !batch.proxy_epoch_id.is_empty() && batch.proxy_epoch_id.len() <= 64,
        "invalid critical epoch"
    );
    let last = batch
        .first_sequence
        .checked_add(batch.events.len() as u64 - 1)
        .filter(|last| batch.first_sequence > 0 && *last <= i64::MAX as u64)
        .ok_or_else(|| anyhow::anyhow!("invalid critical sequence"))?;
    for event in &batch.events {
        anyhow::ensure!(
            !event.event_id.is_empty() && event.event_id.len() <= 128,
            "invalid critical event identity"
        );
        anyhow::ensure!(
            matches!(event.payload, Some(Payload::Flag(_) | Payload::Loss(_))),
            "noncritical payload in critical stream"
        );
    }
    let epoch = batch.proxy_epoch_id.clone();
    let highwater = flags.auth.db.run(move |db| {
        let highwater = db.query_row("SELECT through_sequence FROM critical_receipts WHERE proxy_epoch_id=?1", [&epoch], |row| row.get::<_, i64>(0)).optional()?;
        if highwater.is_none() {
            let epochs: i64 = db.query_row("SELECT count(*) FROM critical_receipts", [], |row| row.get(0))?;
            anyhow::ensure!(epochs < 16, "critical epoch receipt capacity exhausted; preserve records and investigate spool reset history");
        }
        Ok(highwater.map(|value| value as u64))
    }).await?;
    if let Some(through) = highwater {
        if last <= through {
            return Ok(());
        }
        anyhow::ensure!(
            batch.first_sequence <= through + 1,
            "critical stream sequence gap"
        );
    }
    let skip = highwater.map_or(0, |through| {
        through
            .saturating_add(1)
            .saturating_sub(batch.first_sequence) as usize
    });
    let mut fresh = batch.clone();
    fresh.events = fresh.events.into_iter().skip(skip).collect();
    fresh.first_sequence += skip as u64;
    flags.ingest_proxy(&fresh).await?;
    // Never send flag values to traffic; losses get deterministic epoch+seq IDs.
    let mut losses = fresh.clone();
    losses.events = fresh
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            if matches!(event.payload, Some(Payload::Loss(_))) {
                let mut event = event.clone();
                event.event_id = format!(
                    "critical:{}:{}",
                    batch.proxy_epoch_id,
                    fresh.first_sequence + index as u64
                );
                Some(event)
            } else {
                None
            }
        })
        .collect();
    if !losses.events.is_empty() {
        traffic.ingest(losses).await?;
    }
    let epoch = batch.proxy_epoch_id.clone();
    let last_id = batch
        .events
        .last()
        .expect("nonempty validated batch")
        .event_id
        .clone();
    flags.auth.db.run(move |db| {
        let tx = db.transaction()?;
        tx.execute("INSERT INTO critical_receipts(proxy_epoch_id,through_sequence,last_event_id) VALUES(?1,?2,?3) ON CONFLICT(proxy_epoch_id) DO UPDATE SET through_sequence=excluded.through_sequence,last_event_id=excluded.last_event_id WHERE excluded.through_sequence>critical_receipts.through_sequence", params![epoch,last as i64,last_id])?;
        tx.commit()?;
        Ok(())
    }).await?;
    for event in &fresh.events {
        if matches!(event.payload, Some(Payload::Flag(_))) {
            traffic.flag_detected(&event.flow_id, &event.service_id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        infrastructure::{crypto::Crypto, db::Database},
        modules::{
            auth::AuthState,
            flags::{Flags, Settings},
            traffic::Traffic,
        },
    };
    use hydra_wire::v1::{self, event::Payload};
    async fn fixture() -> (Flags, Traffic, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("hydra-critical-core-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let db = Database::open(dir.join("db")).await.unwrap();
        db.migrate(
            9,
            include_str!("../../../../../migrations/0009_critical_receipts.sql"),
        )
        .await
        .unwrap();
        let auth = AuthState::new(
            db,
            Crypto::load(&dir.join("key")).unwrap(),
            "https://localhost".into(),
        );
        let flags = Flags::open(
            auth.clone(),
            Settings {
                pattern: Some("FLAG[0-9]+".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let traffic = Traffic::open(auth).await.unwrap();
        (flags, traffic, dir)
    }
    fn batch() -> v1::EventBatch {
        v1::EventBatch {
            proxy_epoch_id: "epoch".into(),
            first_sequence: 1,
            critical: true,
            events: vec![
                v1::Event {
                    event_id: "flag-id".into(),
                    flow_id: "flagged-flow".into(),
                    service_id: "svc".into(),
                    payload: Some(Payload::Flag(v1::FlagFinding {
                        flag_value: "FLAG123".into(),
                        source_kind: "own_exposed".into(),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                v1::Event {
                    event_id: "loss-id".into(),
                    payload: Some(Payload::Loss(v1::EventLoss {
                        count: 3,
                        reason: "quota".into(),
                    })),
                    ..Default::default()
                },
            ],
        }
    }
    #[tokio::test]
    async fn flags_and_losses_are_durable_before_receipt_and_replay_is_idempotent() {
        let (flags, traffic, dir) = fixture().await;
        let mut live = traffic.events.subscribe();
        commit(&flags, &traffic, &batch()).await.unwrap();
        commit(&flags, &traffic, &batch()).await.unwrap();
        let loss = live.try_recv().unwrap();
        let finding = live.try_recv().unwrap();
        assert_eq!(loss["kind"], "event_loss");
        assert_eq!(
            finding,
            serde_json::json!({"kind":"flag_detected","flow_id":"flagged-flow","service_id":"svc","has_flag":true})
        );
        assert!(!serde_json::to_string(&finding).unwrap().contains("FLAG123"));
        assert!(live.try_recv().is_err());
        flags.auth.db.run(|db|{
            assert_eq!(db.query_row("SELECT count(*) FROM flags",[],|r|r.get::<_,i64>(0))?,1);
            assert_eq!(db.query_row("SELECT count(*) FROM traffic_events WHERE kind='event_loss'",[],|r|r.get::<_,i64>(0))?,1);
            assert_eq!(db.query_row("SELECT through_sequence FROM critical_receipts WHERE proxy_epoch_id='epoch'",[],|r|r.get::<_,i64>(0))?,2);
            assert_eq!(db.query_row("SELECT last_event_id FROM critical_receipts",[],|r|r.get::<_,String>(0))?,"loss-id");Ok(())
        }).await.unwrap();
        drop(traffic);
        drop(flags);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn invalid_batch_and_sequence_gap_never_advance_receipt() {
        let (flags, traffic, dir) = fixture().await;
        let mut invalid = batch();
        invalid.events[1].payload = None;
        assert!(commit(&flags, &traffic, &invalid).await.is_err());
        commit(&flags, &traffic, &batch()).await.unwrap();
        let mut gap = batch();
        gap.first_sequence = 4;
        assert!(commit(&flags, &traffic, &gap).await.is_err());
        drop(traffic);
        drop(flags);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
