//! A separate fixed-slot archive. Slots reserve the full bounded tail; never evict pins.
use super::{FlowHistory, storage};
use std::{
    fs,
    path::{Path, PathBuf},
};
const MAX_PINS: usize = 128;
const MAX_RECORD_BYTES: u64 = 2 * 1024 * 1024;
fn directory(root: &Path) -> anyhow::Result<PathBuf> {
    let path = root.join("favorites");
    if !path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&path)?;
    }
    storage::safe_directory(&path)?;
    Ok(path)
}
fn path(root: &Path, id: &str) -> anyhow::Result<PathBuf> {
    Ok(directory(root)?.join(format!("{id}.zst")))
}
pub(super) fn get(root: &Path, id: &str) -> anyhow::Result<Option<FlowHistory>> {
    let path = path(root, id)?;
    if !path.exists() {
        return Ok(None);
    }
    anyhow::ensure!(
        fs::symlink_metadata(&path)?.is_file() && fs::metadata(&path)?.len() <= MAX_RECORD_BYTES,
        "invalid favorite record"
    );
    let decoded = hydra_compression::decode(&fs::read(path)?, MAX_RECORD_BYTES as usize)?;
    let value: Option<FlowHistory> = serde_json::from_slice(&decoded)?;
    Ok(value)
}
fn write(root: &Path, id: &str, value: Option<&FlowHistory>) -> anyhow::Result<()> {
    let target = path(root, id)?;
    let temporary = target.with_extension("tmp");
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    let bytes = serde_json::to_vec(&value)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECORD_BYTES,
        "favorite record exceeds bound"
    );
    storage::write_private(&temporary, &hydra_compression::encode(&bytes)?)?;
    fs::rename(temporary, target)?;
    fs::File::open(directory(root)?)?.sync_all()?;
    Ok(())
}
pub(super) fn set(
    root: &Path,
    id: &str,
    favorite: bool,
    snapshot: Option<FlowHistory>,
) -> anyhow::Result<String> {
    let target = path(root, id)?;
    if !favorite {
        if target.exists() {
            fs::remove_file(&target)?;
            fs::File::open(directory(root)?)?.sync_all()?;
        }
        return Ok("removed".into());
    }
    if target.exists() {
        return Ok(match get(root, id)? {
            Some(h) if h.closed_at_ms == 0 => {
                if let Some(completed) = storage::read_record(root, id)? {
                    write(root, id, Some(&completed))?;
                    "ready"
                } else if let Some(active) = snapshot {
                    write(root, id, Some(&active))?;
                    "pending"
                } else {
                    "partial"
                }
            }
            Some(_) => "ready",
            None => "unavailable",
        }
        .into());
    }
    let count = fs::read_dir(directory(root)?)?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|v| v == "zst"))
        .count();
    anyhow::ensure!(count < MAX_PINS, "favorite archive capacity exhausted");
    let history = storage::read_record(root, id)?.or(snapshot);
    let state = match &history {
        Some(h) if h.closed_at_ms == 0 => "pending",
        Some(_) => "ready",
        None => "unavailable",
    };
    write(root, id, history.as_ref())?;
    Ok(state.into())
}
pub(super) fn update(root: &Path, history: &FlowHistory) -> anyhow::Result<()> {
    if path(root, &history.flow_id)?.exists() {
        write(root, &history.flow_id, Some(history))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(id: &str, at: i64) -> FlowHistory {
        FlowHistory {
            transport: "udp".into(),
            datagrams: vec![super::super::Datagram {
                direction: 1,
                occurred_at_ms: at,
                sequence: 1,
                data: vec![],
            }],
            datagrams_client_to_server: 1,
            datagrams_server_to_client: 0,
            flow_id: id.into(),
            service_id: "svc".into(),
            protocol_id: "raw".into(),
            opened_at_ms: at,
            closed_at_ms: at,
            bytes_client_to_server: 0,
            bytes_server_to_client: 0,
            client_to_server: vec![],
            server_to_client: vec![],
            truncated_client_to_server: false,
            truncated_server_to_client: false,
            history_truncated: false,
        }
    }
    #[test]
    fn archive_survives_normal_byte_ttl_rotation_and_unpin() {
        let dir = super::super::tests::Directory::new();
        let id = uuid::Uuid::new_v4().to_string();
        let at = super::super::now_ms();
        let history = record(&id, at);
        storage::persist(&dir.0, history.clone()).unwrap();
        assert_eq!(set(&dir.0, &id, true, None).unwrap(), "ready");
        assert_eq!(
            &fs::read(path(&dir.0, &id).unwrap()).unwrap()[..4],
            &[0x28, 0xb5, 0x2f, 0xfd]
        );
        storage::cleanup_for_quota(&dir.0, at, 1, 1).unwrap();
        assert_eq!(get(&dir.0, &id).unwrap(), Some(history.clone()));
        storage::cleanup(&dir.0, at + (super::super::TTL_SECONDS + 1) * 1000).unwrap();
        assert!(storage::read_record(&dir.0, &id).unwrap().is_none());
        assert_eq!(get(&dir.0, &id).unwrap(), Some(history));
        assert_eq!(set(&dir.0, &id, true, None).unwrap(), "ready");
        set(&dir.0, &id, false, None).unwrap();
        assert!(get(&dir.0, &id).unwrap().is_none());
    }
    #[test]
    fn active_snapshot_restart_completion_and_capacity_are_explicit() {
        let dir = super::super::tests::Directory::new();
        let id = uuid::Uuid::new_v4().to_string();
        let mut pending = record(&id, 0);
        pending.history_truncated = true;
        assert_eq!(
            set(&dir.0, &id, true, Some(pending.clone())).unwrap(),
            "pending"
        );
        assert_eq!(get(&dir.0, &id).unwrap(), Some(pending));
        let completed = record(&id, super::super::now_ms());
        update(&dir.0, &completed).unwrap();
        assert_eq!(set(&dir.0, &id, true, None).unwrap(), "ready");
        for _ in 1..MAX_PINS {
            set(&dir.0, &uuid::Uuid::new_v4().to_string(), true, None).unwrap();
        }
        assert!(
            set(&dir.0, &uuid::Uuid::new_v4().to_string(), true, None)
                .unwrap_err()
                .to_string()
                .contains("capacity")
        );
        assert_eq!(get(&dir.0, &id).unwrap(), Some(completed));
    }
    #[tokio::test]
    async fn live_pin_keeps_latest_tail_completion_and_interrupted_snapshot_after_reopen() {
        use super::super::{HistoryReadError, HistoryStore};
        use hydra_protocol_sdk::Direction;
        let dir = super::super::tests::Directory::new();
        let store = HistoryStore::open(&dir.0).await.unwrap();
        let service:hydra_domain::Service=serde_json::from_value(serde_json::json!({"id":"svc","transport":"tcp","listen_address":"127.0.0.1:23001","upstream_address":"127.0.0.1:23002","enabled":true,"retain_history":true})).unwrap();
        store.apply_services(&[service]);
        let id = uuid::Uuid::new_v4().to_string();
        let mut writer = store.begin(id.clone(), "svc".into(), "raw".into()).unwrap();
        writer.record(Direction::ClientToServer, b"before");
        assert_eq!(store.set_favorite(&id, true).await.unwrap(), "pending");
        writer.record(Direction::ClientToServer, b"after");
        assert_eq!(store.set_favorite(&id, true).await.unwrap(), "pending");
        assert!(matches!(
            store.get(&id).await,
            Err(HistoryReadError::Active)
        ));
        assert!(!writer.finish(super::super::now_ms(), 11, 0));
        assert_eq!(store.set_favorite(&id, true).await.unwrap(), "ready");
        let saved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(saved.client_to_server, b"beforeafter");
        let interrupted = uuid::Uuid::new_v4().to_string();
        let mut writer = store
            .begin(interrupted.clone(), "svc".into(), "raw".into())
            .unwrap();
        writer.record(Direction::ClientToServer, b"snapshot");
        assert_eq!(
            store.set_favorite(&interrupted, true).await.unwrap(),
            "pending"
        );
        drop(writer);
        drop(store);
        let reopened = HistoryStore::open(&dir.0).await.unwrap();
        assert_eq!(
            reopened.set_favorite(&interrupted, true).await.unwrap(),
            "partial"
        );
        let saved = reopened.get(&interrupted).await.unwrap().unwrap();
        assert_eq!(saved.client_to_server, b"snapshot");
        assert!(saved.history_truncated);
        assert_eq!(saved.closed_at_ms, 0);
        assert_eq!(saved.bytes_client_to_server, 0);
        assert_eq!(saved.bytes_server_to_client, 0);
        assert_eq!(
            reopened.get(&id).await.unwrap().unwrap().client_to_server,
            b"beforeafter"
        );
    }
}
