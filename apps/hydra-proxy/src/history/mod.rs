//! Bounded, gzip-compressed tails of completed TCP flows. Capture never writes
//! to disk on the relay task; one bounded worker owns persistence and cleanup.
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use hydra_domain::Service;
use hydra_protocol_sdk::Direction;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;

pub const MAX_DIRECTION_BYTES: usize = 64 * 1024;
pub const MAX_HISTORY_BYTES: usize = MAX_DIRECTION_BYTES * 2;
const MAX_ACTIVE: usize = 256;
const WRITE_QUEUE: usize = 64;
const DISK_QUOTA: u64 = 100 * 1024 * 1024;
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

struct Inner {
    directory: PathBuf,
    enabled_services: Mutex<HashSet<String>>,
    active: Mutex<HashMap<String, Arc<Mutex<Buffer>>>>,
    pending: Arc<Mutex<HashSet<String>>>,
    writer: mpsc::Sender<FlowHistory>,
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

fn paths(directory: &Path, id: &str) -> (PathBuf, PathBuf) {
    let stem = format!("{FILE_PREFIX}{id}");
    (
        directory.join(format!("{stem}.gz")),
        directory.join(format!("{stem}.json")),
    )
}

fn safe_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == fs::metadata("/proc/self")?.uid()
            && metadata.permissions().mode() & 0o077 == 0,
        "history file is not private"
    );
    Ok(())
}

fn safe_directory(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        anyhow::ensure!(
            !fs::symlink_metadata(ancestor)?.file_type().is_symlink(),
            "history directory contains symlink"
        );
    }
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == fs::metadata("/proc/self")?.uid()
            && metadata.permissions().mode() & 0o077 == 0,
        "history directory must be private"
    );
    Ok(())
}

impl HistoryStore {
    /// Open a private history directory and resume completed, unexpired records.
    pub async fn open(directory: impl AsRef<Path>) -> anyhow::Result<Arc<Self>> {
        let directory = directory.as_ref().to_owned();
        let checked = directory.clone();
        tokio::task::spawn_blocking(move || {
            safe_directory(&checked)?;
            cleanup(&checked, now_ms())
        })
        .await??;
        let (writer, mut reader) = mpsc::channel::<FlowHistory>(WRITE_QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let worker_dropped = dropped.clone();
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let worker_pending = pending.clone();
        let worker_directory = directory.clone();
        tokio::spawn(async move {
            let mut reap = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    item = reader.recv() => match item {
                        Some(history) => {
                            let flow_id = history.flow_id.clone();
                            let path = worker_directory.clone();
                            let result = tokio::task::spawn_blocking(move || persist(&path, history)).await;
                            if !matches!(result, Ok(Ok(()))) {
                                worker_dropped.fetch_add(1, Ordering::Relaxed);
                                tracing::warn!("flow history persistence failed");
                            }
                            worker_pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&flow_id);
                        }
                        None => break,
                    },
                    _ = reap.tick() => {
                        let path = worker_directory.clone();
                        let result = tokio::task::spawn_blocking(move || cleanup(&path, now_ms())).await;
                        if !matches!(result, Ok(Ok(()))) {
                            tracing::warn!("flow history cleanup failed");
                        }
                    }
                }
            }
        });
        Ok(Arc::new(Self(Arc::new(Inner {
            directory,
            enabled_services: Mutex::new(HashSet::new()),
            active: Mutex::new(HashMap::new()),
            pending,
            writer,
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
        match store.writer.try_send(history) {
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

fn gzip_payload(history: &FlowHistory) -> anyhow::Result<Vec<u8>> {
    let mut payload =
        Vec::with_capacity(history.client_to_server.len() + history.server_to_client.len());
    payload.extend_from_slice(&history.client_to_server);
    payload.extend_from_slice(&history.server_to_client);
    anyhow::ensure!(
        payload.len() <= MAX_HISTORY_BYTES,
        "history exceeds payload bound"
    );
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&payload)?;
    Ok(encoder.finish()?)
}

fn metadata(history: &FlowHistory) -> Metadata {
    Metadata {
        flow_id: history.flow_id.clone(),
        service_id: history.service_id.clone(),
        protocol_id: history.protocol_id.clone(),
        opened_at_ms: history.opened_at_ms,
        closed_at_ms: history.closed_at_ms,
        bytes_client_to_server: history.bytes_client_to_server,
        bytes_server_to_client: history.bytes_server_to_client,
        client_len: history.client_to_server.len(),
        server_len: history.server_to_client.len(),
        truncated_client_to_server: history.truncated_client_to_server,
        truncated_server_to_client: history.truncated_server_to_client,
        history_truncated: history.history_truncated,
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn persist(directory: &Path, history: FlowHistory) -> anyhow::Result<()> {
    anyhow::ensure!(
        uuid::Uuid::parse_str(&history.flow_id).is_ok(),
        "invalid history flow id"
    );
    let payload = gzip_payload(&history)?;
    let details = serde_json::to_vec(&metadata(&history))?;
    let (payload_path, metadata_path) = paths(directory, &history.flow_id);
    cleanup_for(directory, history.closed_at_ms, payload.len() as u64)?;
    let nonce = uuid::Uuid::new_v4();
    let payload_tmp = directory.join(format!("{FILE_PREFIX}{nonce}.tmp.gz"));
    let metadata_tmp = directory.join(format!("{FILE_PREFIX}{nonce}.tmp.json"));
    let result = (|| -> anyhow::Result<()> {
        write_private(&payload_tmp, &payload)?;
        write_private(&metadata_tmp, &details)?;
        fs::rename(&payload_tmp, &payload_path)?;
        fs::rename(&metadata_tmp, &metadata_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(payload_tmp);
        let _ = fs::remove_file(metadata_tmp);
        let _ = fs::remove_file(payload_path);
    }
    result
}

fn read_record(directory: &Path, id: &str) -> anyhow::Result<Option<FlowHistory>> {
    let (payload_path, metadata_path) = paths(directory, id);
    if !metadata_path.exists() || !payload_path.exists() {
        return Ok(None);
    }
    safe_file(&metadata_path)?;
    safe_file(&payload_path)?;
    anyhow::ensure!(
        fs::metadata(&metadata_path)?.len() <= 4096,
        "history metadata exceeds bound"
    );
    let details: Metadata = serde_json::from_slice(&fs::read(&metadata_path)?)?;
    if details.flow_id != id || details.closed_at_ms < now_ms() - TTL_SECONDS * 1000 {
        return Ok(None);
    }
    anyhow::ensure!(
        details.client_len <= MAX_DIRECTION_BYTES && details.server_len <= MAX_DIRECTION_BYTES,
        "history metadata exceeds bounds"
    );
    let file = fs::File::open(payload_path)?;
    let mut decoder = GzDecoder::new(file).take((MAX_HISTORY_BYTES + 1) as u64);
    let mut payload = Vec::with_capacity(MAX_HISTORY_BYTES);
    decoder.read_to_end(&mut payload)?;
    anyhow::ensure!(
        payload.len() <= MAX_HISTORY_BYTES,
        "decompressed history exceeds bound"
    );
    anyhow::ensure!(
        payload.len() == details.client_len + details.server_len,
        "history payload length mismatch"
    );
    let server = payload.split_off(details.client_len);
    Ok(Some(FlowHistory {
        flow_id: details.flow_id,
        service_id: details.service_id,
        protocol_id: details.protocol_id,
        opened_at_ms: details.opened_at_ms,
        closed_at_ms: details.closed_at_ms,
        bytes_client_to_server: details.bytes_client_to_server,
        bytes_server_to_client: details.bytes_server_to_client,
        client_to_server: payload,
        server_to_client: server,
        truncated_client_to_server: details.truncated_client_to_server,
        truncated_server_to_client: details.truncated_server_to_client,
        history_truncated: details.history_truncated,
    }))
}

fn cleanup(directory: &Path, now: i64) -> anyhow::Result<()> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(FILE_PREFIX) && name.contains(".tmp.") {
            let _ = fs::remove_file(entry.path());
            continue;
        }
        if let Some(id) = name
            .strip_prefix(FILE_PREFIX)
            .and_then(|s| s.strip_suffix(".gz"))
        {
            if uuid::Uuid::parse_str(id).is_ok()
                && !directory.join(format!("{FILE_PREFIX}{id}.json")).exists()
                && entry.path().exists()
            {
                safe_file(&entry.path())?;
                fs::remove_file(entry.path())?;
            }
            continue;
        }
        let Some(id) = name
            .strip_prefix(FILE_PREFIX)
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_err() {
            continue;
        }
        safe_file(&entry.path())?;
        let detail: Metadata = serde_json::from_slice(&fs::read(entry.path())?)?;
        let (payload, metadata) = paths(directory, id);
        if detail.flow_id != id || detail.closed_at_ms < now - TTL_SECONDS * 1000 {
            let _ = fs::remove_file(payload);
            let _ = fs::remove_file(metadata);
            continue;
        }
        safe_file(&payload)?;
        records.push((
            detail.closed_at_ms,
            payload.metadata()?.len(),
            payload,
            metadata,
        ));
    }
    let mut total: u64 = records.iter().map(|record| record.1).sum();
    records.sort_by_key(|record| record.0);
    for (_, size, payload, metadata) in records {
        if total <= DISK_QUOTA {
            break;
        }
        fs::remove_file(payload)?;
        fs::remove_file(metadata)?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

fn cleanup_for(directory: &Path, now: i64, needed: u64) -> anyhow::Result<()> {
    cleanup_for_quota(directory, now, needed, DISK_QUOTA)
}

fn cleanup_for_quota(directory: &Path, now: i64, needed: u64, quota: u64) -> anyhow::Result<()> {
    anyhow::ensure!(needed <= quota, "history record exceeds disk quota");
    cleanup(directory, now)?;
    let mut records = Vec::new();
    let mut total = 0u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(id) = name
            .strip_prefix(FILE_PREFIX)
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_err() {
            continue;
        }
        let metadata_path = entry.path();
        let detail: Metadata = serde_json::from_slice(&fs::read(&metadata_path)?)?;
        let (payload, sidecar) = paths(directory, id);
        safe_file(&payload)?;
        let size = payload.metadata()?.len();
        total = total.saturating_add(size);
        records.push((detail.closed_at_ms, size, payload, sidecar));
    }
    records.sort_by_key(|record| record.0);
    for (_, size, payload, metadata) in records {
        if total.saturating_add(needed) <= quota {
            break;
        }
        fs::remove_file(payload)?;
        fs::remove_file(metadata)?;
        total = total.saturating_sub(size);
    }
    anyhow::ensure!(
        total.saturating_add(needed) <= quota,
        "history disk quota exhausted"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hydra_domain::Transport;
    use std::{net::SocketAddr, path::PathBuf};

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
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
