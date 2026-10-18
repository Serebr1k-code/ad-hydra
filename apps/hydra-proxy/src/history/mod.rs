//! Bounded, gzip-compressed tails of completed TCP flows. Capture never writes
//! to disk on the relay task; one bounded worker owns persistence and cleanup.
mod storage;
use hydra_domain::Service;
use hydra_protocol_sdk::Direction;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use storage::read_record;
use tokio::sync::{mpsc, oneshot};

pub const MAX_DIRECTION_BYTES: usize = 64 * 1024;
pub const MAX_HISTORY_BYTES: usize = MAX_DIRECTION_BYTES * 2;
const MAX_ACTIVE: usize = 256;
const WRITE_QUEUE: usize = 64;
const TTL_SECONDS: i64 = 24 * 60 * 60;
const FILE_PREFIX: &str = "hydra-history-";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowHistory {
    pub flow_id: String,
    pub service_id: String,
    pub protocol_id: String,
    pub opened_at_ms: i64,
    pub closed_at_ms: i64,
    pub bytes_client_to_server: u64,
    pub bytes_server_to_client: u64,
    pub client_to_server: Vec<u8>,
    pub server_to_client: Vec<u8>,
    pub truncated_client_to_server: bool,
    pub truncated_server_to_client: bool,
    pub history_truncated: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Metadata {
    flow_id: String,
    service_id: String,
    protocol_id: String,
    opened_at_ms: i64,
    closed_at_ms: i64,
    bytes_client_to_server: u64,
    bytes_server_to_client: u64,
    client_len: usize,
    server_len: usize,
    truncated_client_to_server: bool,
    truncated_server_to_client: bool,
    history_truncated: bool,
}

#[derive(Default)]
struct Buffer {
    service_id: String,
    protocol_id: String,
    opened_at_ms: i64,
    client: Vec<u8>,
    server: Vec<u8>,
    truncated_client: bool,
    truncated_server: bool,
    discarded: bool,
}

enum WriteCommand {
    Persist(FlowHistory),
    SetQuota {
        bytes: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

struct Inner {
    directory: PathBuf,
    enabled_services: Mutex<HashSet<String>>,
    active: Mutex<HashMap<String, Arc<Mutex<Buffer>>>>,
    pending: Arc<Mutex<HashSet<String>>>,
    writer: mpsc::Sender<WriteCommand>,
    quota: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
}

pub struct HistoryStore(Arc<Inner>);

pub struct HistoryWriter {
    flow_id: String,
    buffer: Arc<Mutex<Buffer>>,
    store: Weak<Inner>,
}

#[derive(Debug)]
pub enum HistoryReadError {
    Active,
    Storage(anyhow::Error),
}
impl std::fmt::Display for HistoryReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => formatter.write_str("flow history is still active"),
            Self::Storage(_) => formatter.write_str("history storage failed"),
        }
    }
}
impl std::error::Error for HistoryReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error.as_ref()),
            Self::Active => None,
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

impl HistoryStore {
    /// Open a private history directory and resume completed, unexpired records.
    pub async fn open(directory: impl AsRef<Path>) -> anyhow::Result<Arc<Self>> {
        let directory = directory.as_ref().to_owned();
        let checked = directory.clone();
        let mut disk =
            tokio::task::spawn_blocking(move || storage::Disk::open(checked, now_ms())).await??;
        let (writer, mut reader) = mpsc::channel::<WriteCommand>(WRITE_QUEUE);
        let quota = Arc::new(AtomicU64::new(disk.quota()));
        let worker_quota = quota.clone();
        let dropped = Arc::new(AtomicU64::new(0));
        let worker_dropped = dropped.clone();
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let worker_pending = pending.clone();
        tokio::spawn(async move {
            let interval = Duration::from_secs(60);
            let mut reap =
                tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
            reap.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let item = tokio::select! {
                    item = reader.recv() => match item { Some(item) => Some(item), None => break },
                    _ = reap.tick() => None,
                };
                let flow_id = match &item {
                    Some(WriteCommand::Persist(history)) => Some(history.flow_id.clone()),
                    _ => None,
                };
                let applied_quota = worker_quota.clone();
                let completed = tokio::task::spawn_blocking(move || {
                    let result = match item {
                        Some(WriteCommand::Persist(history)) => disk.persist(history),
                        Some(WriteCommand::SetQuota { bytes, reply }) => {
                            let result = disk.set_quota(bytes);
                            if result.is_ok() {
                                applied_quota.store(bytes, Ordering::Relaxed);
                            }
                            let _ = reply
                                .send(result.as_ref().map(|_| ()).map_err(ToString::to_string));
                            result
                        }
                        None => disk.reap(now_ms()),
                    };
                    (disk, result)
                })
                .await;
                let result;
                match completed {
                    Ok((updated, outcome)) => {
                        disk = updated;
                        result = outcome;
                    }
                    Err(_) => {
                        worker_dropped.fetch_add(1, Ordering::Relaxed);
                        worker_pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clear();
                        tracing::warn!("flow history worker stopped");
                        break;
                    }
                }
                if result.is_err() {
                    if let Some(id) = &flow_id {
                        let dropped = worker_dropped.fetch_add(1, Ordering::Relaxed) + 1;
                        tracing::warn!(flow_id=%id,dropped,"completed stream history was dropped");
                    } else {
                        tracing::warn!("flow history policy or cleanup failed");
                    }
                }
                if let Some(flow_id) = flow_id {
                    worker_pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&flow_id);
                }
            }
        });
        Ok(Arc::new(Self(Arc::new(Inner {
            directory,
            enabled_services: Mutex::new(HashSet::new()),
            active: Mutex::new(HashMap::new()),
            pending,
            writer,
            quota,
            dropped,
        }))))
    }

    /// Apply the current policy. Disabling retention clears active buffers for
    /// that service immediately; the current relay flow continues unchanged.
    pub fn apply_services(&self, services: &[Service]) {
        let enabled: HashSet<String> = services
            .iter()
            .filter(|service| {
                service.transport == hydra_domain::Transport::Tcp && service.retain_history
            })
            .map(|service| service.id.clone())
            .collect();
        *self
            .0
            .enabled_services
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = enabled.clone();
        let mut active = self.0.active.lock().unwrap_or_else(|e| e.into_inner());
        active.retain(|_, buffer| {
            let mut buffer = buffer.lock().unwrap_or_else(|e| e.into_inner());
            if !enabled.contains(&buffer.service_id) {
                buffer.discarded = true;
                false
            } else {
                true
            }
        });
    }

    pub fn begin(
        &self,
        flow_id: String,
        service_id: String,
        protocol_id: String,
    ) -> Option<HistoryWriter> {
        if flow_id.len() > 128
            || !self
                .0
                .enabled_services
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&service_id)
        {
            return None;
        }
        let mut active = self.0.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.len() >= MAX_ACTIVE || active.contains_key(&flow_id) {
            return None;
        }
        let buffer = Arc::new(Mutex::new(Buffer {
            service_id,
            protocol_id,
            opened_at_ms: now_ms(),
            ..Default::default()
        }));
        active.insert(flow_id.clone(), buffer.clone());
        Some(HistoryWriter {
            flow_id,
            buffer,
            store: Arc::downgrade(&self.0),
        })
    }

    pub fn quota(&self) -> u64 {
        self.0.quota.load(Ordering::Relaxed)
    }

    pub async fn set_quota(&self, bytes: u64) -> anyhow::Result<()> {
        use hydra_domain::history::{MAX_PER_SERVICE_MIB, MIB, MIN_PER_SERVICE_MIB};
        anyhow::ensure!(
            (u64::from(MIN_PER_SERVICE_MIB) * MIB..=u64::from(MAX_PER_SERVICE_MIB) * MIB)
                .contains(&bytes),
            "history quota must be 1..1024 MiB per service"
        );
        let (reply, completed) = oneshot::channel();
        self.0
            .writer
            .send(WriteCommand::SetQuota { bytes, reply })
            .await
            .map_err(|_| anyhow::anyhow!("history worker unavailable"))?;
        completed
            .await
            .map_err(|_| anyhow::anyhow!("history worker unavailable"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn dropped(&self) -> u64 {
        self.0.dropped.load(Ordering::Relaxed)
    }

    pub async fn get(&self, flow_id: &str) -> Result<Option<FlowHistory>, HistoryReadError> {
        if flow_id.len() > 128 || uuid::Uuid::parse_str(flow_id).is_err() {
            return Ok(None);
        }
        if self
            .0
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(flow_id)
        {
            return Err(HistoryReadError::Active);
        }
        for _ in 0..20 {
            if !self
                .0
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(flow_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if self
            .0
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(flow_id)
        {
            return Err(HistoryReadError::Active);
        }
        let directory = self.0.directory.clone();
        let id = flow_id.to_owned();
        tokio::task::spawn_blocking(move || read_record(&directory, &id))
            .await
            .map_err(|error| HistoryReadError::Storage(error.into()))?
            .map_err(HistoryReadError::Storage)
    }
}

impl HistoryWriter {
    pub fn record(&mut self, direction: Direction, bytes: &[u8]) {
        let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        if buffer.discarded || bytes.is_empty() {
            return;
        }
        let Buffer {
            client,
            server,
            truncated_client,
            truncated_server,
            ..
        } = &mut *buffer;
        match direction {
            Direction::ClientToServer => append_tail(client, truncated_client, bytes),
            Direction::ServerToClient => append_tail(server, truncated_server, bytes),
        }
    }

    /// Returns true if history was intentionally discarded or could not be queued.
    pub fn finish(
        self,
        closed_at_ms: i64,
        bytes_client_to_server: u64,
        bytes_server_to_client: u64,
    ) -> bool {
        let Some(store) = self.store.upgrade() else {
            return true;
        };
        store
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.flow_id);
        let buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        if buffer.discarded {
            return false;
        }
        let history = FlowHistory {
            flow_id: self.flow_id.clone(),
            service_id: buffer.service_id.clone(),
            protocol_id: buffer.protocol_id.clone(),
            opened_at_ms: buffer.opened_at_ms,
            closed_at_ms,
            bytes_client_to_server,
            bytes_server_to_client,
            client_to_server: buffer.client.clone(),
            server_to_client: buffer.server.clone(),
            truncated_client_to_server: buffer.truncated_client
                || bytes_client_to_server > buffer.client.len() as u64,
            truncated_server_to_client: buffer.truncated_server
                || bytes_server_to_client > buffer.server.len() as u64,
            history_truncated: buffer.truncated_client
                || buffer.truncated_server
                || bytes_client_to_server > buffer.client.len() as u64
                || bytes_server_to_client > buffer.server.len() as u64,
        };
        store
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.flow_id.clone());
        match store.writer.try_send(WriteCommand::Persist(history)) {
            Ok(()) => false,
            Err(_) => {
                store
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&self.flow_id);
                store.dropped.fetch_add(1, Ordering::Relaxed);
                true
            }
        }
    }
}

fn append_tail(target: &mut Vec<u8>, truncated: &mut bool, bytes: &[u8]) {
    if bytes.len() >= MAX_DIRECTION_BYTES {
        target.clear();
        target.extend_from_slice(&bytes[bytes.len() - MAX_DIRECTION_BYTES..]);
        *truncated = true;
        return;
    }
    let overflow = target
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(MAX_DIRECTION_BYTES);
    if overflow > 0 {
        target.drain(..overflow.min(target.len()));
        *truncated = true;
    }
    target.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::storage::{cleanup, cleanup_for_quota, paths, persist, write_private};
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use hydra_domain::Transport;
    use std::{fs, io::Write};
    use std::{net::SocketAddr, path::PathBuf};

    pub(super) struct Directory(pub(super) PathBuf);
    impl Directory {
        pub(super) fn new() -> Self {
            let path = std::env::temp_dir().join(format!("hydra-history-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn service(retain_history: bool) -> Service {
        Service {
            id: "svc".into(),
            transport: Transport::Tcp,
            listen_address: "127.0.0.1:23001".parse::<SocketAddr>().unwrap(),
            upstream_address: "127.0.0.1:23002".parse().unwrap(),
            protocol_id: "raw".into(),
            enabled: true,
            retain_history,
        }
    }
    async fn stored(store: &HistoryStore, id: &str) -> FlowHistory {
        for _ in 0..100 {
            if let Some(history) = store.get(id).await.unwrap() {
                return history;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("history was not persisted")
    }

    #[tokio::test]
    async fn quota_is_applied_by_worker_and_persisted_without_restarting_relays() {
        let dir = Directory::new();
        let store = HistoryStore::open(&dir.0).await.unwrap();
        assert_eq!(store.quota(), 128 * 1024 * 1024);
        assert!(store.set_quota(0).await.is_err());
        assert!(store.set_quota(1025 * 1024 * 1024).await.is_err());
        store.set_quota(2 * 1024 * 1024).await.unwrap();
        assert_eq!(store.quota(), 2 * 1024 * 1024);
        drop(store);
        let reopened = HistoryStore::open(&dir.0).await.unwrap();
        assert_eq!(reopened.quota(), 2 * 1024 * 1024);
    }

    #[tokio::test]
    async fn appending_history_does_not_reopen_unrelated_metadata() {
        let dir = Directory::new();
        let store = HistoryStore::open(&dir.0).await.unwrap();
        store.apply_services(&[service(true)]);
        let first = uuid::Uuid::new_v4().to_string();
        store
            .begin(first.clone(), "svc".into(), "raw".into())
            .unwrap()
            .finish(now_ms(), 0, 0);
        stored(&store, &first).await;
        // Corrupt an unrelated sidecar after indexing. Appending must touch only
        // the new record, not enumerate and reopen every retained history.
        let (_, metadata) = paths(&dir.0, &first);
        fs::write(metadata, b"unrelated unreadable metadata").unwrap();
        let second = uuid::Uuid::new_v4().to_string();
        store
            .begin(second.clone(), "svc".into(), "raw".into())
            .unwrap()
            .finish(now_ms(), 0, 0);
        stored(&store, &second).await;
        assert!(store.get(&first).await.is_err());
    }

    #[tokio::test]
    async fn gzip_history_preserves_direction_order_tail_and_survives_restart() {
        let dir = Directory::new();
        let flow_id = uuid::Uuid::new_v4().to_string();
        let store = HistoryStore::open(&dir.0).await.unwrap();
        store.apply_services(&[service(true)]);
        let mut writer = store
            .begin(flow_id.clone(), "svc".into(), "raw".into())
            .unwrap();
        writer.record(Direction::ClientToServer, &vec![1; 70_000]);
        writer.record(Direction::ClientToServer, &[2, 3]);
        writer.record(Direction::ServerToClient, &[9, 8]);
        assert!(!writer.finish(now_ms(), 70_002, 2));
        let first = store.get(&flow_id).await.unwrap().unwrap();
        assert_eq!(first.client_to_server.len(), 65_536);
        assert_eq!(
            &first.client_to_server[first.client_to_server.len() - 2..],
            &[2, 3]
        );
        assert_eq!(first.server_to_client, [9, 8]);
        assert!(first.truncated_client_to_server);
        assert!(!first.truncated_server_to_client);
        drop(store);
        let reopened = HistoryStore::open(&dir.0).await.unwrap();
        assert_eq!(reopened.get(&flow_id).await.unwrap().unwrap(), first);
        let gz = std::fs::read_dir(&dir.0)
            .unwrap()
            .map(Result::unwrap)
            .find(|e| e.path().extension().is_some_and(|ext| ext == "gz"))
            .unwrap();
        assert_eq!(&std::fs::read(gz.path()).unwrap()[..2], &[0x1f, 0x8b]);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(gz.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&dir.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[tokio::test]
    async fn disabling_service_discards_active_history_and_prevents_new_records() {
        let dir = Directory::new();
        let store = HistoryStore::open(&dir.0).await.unwrap();
        store.apply_services(&[service(true)]);
        let id = uuid::Uuid::new_v4().to_string();
        let mut writer = store.begin(id.clone(), "svc".into(), "raw".into()).unwrap();
        writer.record(Direction::ClientToServer, b"secret");
        store.apply_services(&[service(false)]);
        writer.record(Direction::ClientToServer, b"discarded");
        assert!(!writer.finish(now_ms(), 15, 0));
        assert!(store.get(&id).await.unwrap().is_none());
        assert!(
            store
                .begin(uuid::Uuid::new_v4().to_string(), "svc".into(), "raw".into())
                .is_none()
        );
    }

    #[tokio::test]
    async fn active_history_limit_is_bounded() {
        let dir = Directory::new();
        let store = HistoryStore::open(&dir.0).await.unwrap();
        store.apply_services(&[service(true)]);
        let mut writers = Vec::new();
        for _ in 0..MAX_ACTIVE {
            writers.push(
                store
                    .begin(uuid::Uuid::new_v4().to_string(), "svc".into(), "raw".into())
                    .unwrap(),
            );
        }
        assert!(
            store
                .begin(uuid::Uuid::new_v4().to_string(), "svc".into(), "raw".into())
                .is_none()
        );
        drop(writers);
    }

    #[tokio::test]
    async fn full_writer_queue_reports_capture_loss() {
        let dir = Directory::new();
        let (writer, _reader) = mpsc::channel(1);
        let store = Arc::new(HistoryStore(Arc::new(Inner {
            directory: dir.0.clone(),
            enabled_services: Mutex::new(HashSet::from(["svc".into()])),
            active: Mutex::new(HashMap::new()),
            pending: Arc::new(Mutex::new(HashSet::new())),
            writer,
            quota: Arc::new(AtomicU64::new(128 * 1024 * 1024)),
            dropped: Arc::new(AtomicU64::new(0)),
        })));
        let first_id = uuid::Uuid::new_v4().to_string();
        let second_id = uuid::Uuid::new_v4().to_string();
        let mut first = store.begin(first_id, "svc".into(), "raw".into()).unwrap();
        let mut second = store.begin(second_id, "svc".into(), "raw".into()).unwrap();
        first.record(Direction::ClientToServer, b"first");
        second.record(Direction::ClientToServer, b"second");
        assert!(!first.finish(now_ms(), 5, 0));
        assert!(second.finish(now_ms(), 6, 0));
        assert_eq!(store.dropped(), 1);
    }

    #[tokio::test]
    async fn corrupt_and_oversized_gzip_are_rejected() {
        let dir = Directory::new();
        let store = HistoryStore::open(&dir.0).await.unwrap();
        store.apply_services(&[service(true)]);
        let corrupt_id = uuid::Uuid::new_v4().to_string();
        let mut writer = store
            .begin(corrupt_id.clone(), "svc".into(), "raw".into())
            .unwrap();
        writer.record(Direction::ClientToServer, b"valid");
        writer.finish(now_ms(), 5, 0);
        let _ = stored(&store, &corrupt_id).await;
        let (payload_path, _) = paths(&dir.0, &corrupt_id);
        fs::write(&payload_path, b"not gzip").unwrap();
        assert!(store.get(&corrupt_id).await.is_err());

        let oversized_id = uuid::Uuid::new_v4().to_string();
        let details = Metadata {
            flow_id: oversized_id.clone(),
            service_id: "svc".into(),
            protocol_id: "raw".into(),
            opened_at_ms: now_ms(),
            closed_at_ms: now_ms(),
            bytes_client_to_server: MAX_HISTORY_BYTES as u64,
            bytes_server_to_client: 0,
            client_len: MAX_DIRECTION_BYTES,
            server_len: MAX_DIRECTION_BYTES,
            truncated_client_to_server: false,
            truncated_server_to_client: false,
            history_truncated: false,
        };
        let (payload_path, metadata_path) = paths(&dir.0, &oversized_id);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&vec![0; MAX_HISTORY_BYTES + 1]).unwrap();
        write_private(&payload_path, &encoder.finish().unwrap()).unwrap();
        write_private(&metadata_path, &serde_json::to_vec(&details).unwrap()).unwrap();
        assert!(store.get(&oversized_id).await.is_err());
    }

    #[test]
    fn compressed_quota_evicts_oldest_record() {
        let dir = Directory::new();
        let now = now_ms();
        for _ in 0..2 {
            let id = uuid::Uuid::new_v4().to_string();
            persist(
                &dir.0,
                FlowHistory {
                    flow_id: id,
                    service_id: "svc".into(),
                    protocol_id: "raw".into(),
                    opened_at_ms: now,
                    closed_at_ms: now,
                    bytes_client_to_server: 1,
                    bytes_server_to_client: 0,
                    client_to_server: vec![42],
                    server_to_client: vec![],
                    truncated_client_to_server: false,
                    truncated_server_to_client: false,
                    history_truncated: false,
                },
            )
            .unwrap();
        }
        cleanup_for_quota(&dir.0, now, 1, 1).unwrap();
        let remaining = fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .count();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn expired_records_are_removed_during_cleanup() {
        let dir = Directory::new();
        let id = uuid::Uuid::new_v4().to_string();
        let closed_at = now_ms() - (TTL_SECONDS + 1) * 1000;
        persist(
            &dir.0,
            FlowHistory {
                flow_id: id.clone(),
                service_id: "svc".into(),
                protocol_id: "raw".into(),
                opened_at_ms: closed_at - 1000,
                closed_at_ms: closed_at,
                bytes_client_to_server: 1,
                bytes_server_to_client: 0,
                client_to_server: vec![42],
                server_to_client: vec![],
                truncated_client_to_server: false,
                truncated_server_to_client: false,
                history_truncated: false,
            },
        )
        .unwrap();
        cleanup(&dir.0, now_ms()).unwrap();
        let (payload, metadata) = paths(&dir.0, &id);
        assert!(!payload.exists());
        assert!(!metadata.exists());
    }
}
