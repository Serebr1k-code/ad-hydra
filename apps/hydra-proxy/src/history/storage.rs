//! Single-owner bounded index of retained gzip files. Scan once at startup;
//! completed flows and expiry touch only new or evicted records.
use super::{
    DISK_QUOTA, FILE_PREFIX, FlowHistory, MAX_DIRECTION_BYTES, MAX_HISTORY_BYTES, Metadata,
    TTL_SECONDS, now_ms,
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

// Bounds index memory and small-file/inode use even for highly compressed data.
const MAX_RECORDS: usize = 65_536;

pub(super) fn paths(directory: &Path, id: &str) -> (PathBuf, PathBuf) {
    let stem = format!("{FILE_PREFIX}{id}");
    (
        directory.join(format!("{stem}.gz")),
        directory.join(format!("{stem}.json")),
    )
}

pub(super) struct Disk {
    directory: PathBuf,
    records: BTreeMap<(i64, String), u64>,
    bytes: u64,
}

impl Disk {
    pub(super) fn open(directory: PathBuf, now: i64) -> anyhow::Result<Self> {
        safe_directory(&directory)?;
        let mut disk = Self {
            directory,
            records: BTreeMap::new(),
            bytes: 0,
        };
        // Only this worker modifies the private directory. Interrupted writes
        // are cleaned at startup; no periodic enumeration is necessary.
        for entry in fs::read_dir(&disk.directory)? {
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
                    && !paths(&disk.directory, id).1.exists()
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
            let metadata_path = entry.path();
            safe_file(&metadata_path)?;
            anyhow::ensure!(
                fs::metadata(&metadata_path)?.len() <= 4096,
                "history metadata exceeds bound"
            );
            let detail: Metadata = serde_json::from_slice(&fs::read(&metadata_path)?)?;
            let (payload, _) = paths(&disk.directory, id);
            if detail.flow_id != id
                || detail.closed_at_ms < now - TTL_SECONDS * 1000
                || !payload.exists()
            {
                remove_if_present(&payload)?;
                remove_if_present(&metadata_path)?;
                continue;
            }
            safe_file(&payload)?;
            let size = payload.metadata()?.len();
            anyhow::ensure!(
                size <= MAX_HISTORY_BYTES as u64 + 1024,
                "compressed history exceeds bound"
            );
            disk.records.insert((detail.closed_at_ms, id.into()), size);
            disk.bytes += size;
            // Keep startup memory bounded even when importing a pre-limit directory.
            disk.make_room(now, 0, 0, DISK_QUOTA)?;
        }
        Ok(disk)
    }

    pub(super) fn reap(&mut self, now: i64) -> anyhow::Result<()> {
        self.make_room(now, 0, 0, DISK_QUOTA)
    }

    fn make_room(
        &mut self,
        now: i64,
        needed: u64,
        reserve: usize,
        quota: u64,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(needed <= quota, "history record exceeds disk quota");
        while let Some(((closed, id), size)) = self.records.first_key_value() {
            if *closed >= now - TTL_SECONDS * 1000
                && self.bytes.saturating_add(needed) <= quota
                && self.records.len() + reserve <= MAX_RECORDS
            {
                break;
            }
            let size = *size;
            let (payload, metadata) = paths(&self.directory, id);
            // Update accounting only after deletion; a partial I/O failure can
            // be retried without forgetting a file or exceeding the quota.
            remove_if_present(&payload)?;
            remove_if_present(&metadata)?;
            self.records.pop_first();
            self.bytes = self.bytes.saturating_sub(size);
        }
        anyhow::ensure!(
            self.bytes.saturating_add(needed) <= quota,
            "history disk quota exhausted"
        );
        Ok(())
    }

    pub(super) fn persist(&mut self, history: FlowHistory) -> anyhow::Result<()> {
        anyhow::ensure!(
            uuid::Uuid::parse_str(&history.flow_id).is_ok(),
            "invalid history flow id"
        );
        let payload = gzip_payload(&history)?;
        let details = serde_json::to_vec(&metadata(&history))?;
        let (payload_path, metadata_path) = paths(&self.directory, &history.flow_id);
        anyhow::ensure!(
            !payload_path.exists() && !metadata_path.exists(),
            "history already exists"
        );
        self.make_room(now_ms(), payload.len() as u64, 1, DISK_QUOTA)?;
        let nonce = uuid::Uuid::new_v4();
        let payload_tmp = self.directory.join(format!("{FILE_PREFIX}{nonce}.tmp.gz"));
        let metadata_tmp = self
            .directory
            .join(format!("{FILE_PREFIX}{nonce}.tmp.json"));
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
        } else {
            self.bytes += payload.len() as u64;
            self.records.insert(
                (history.closed_at_ms, history.flow_id),
                payload.len() as u64,
            );
        }
        result
    }
}

fn remove_if_present(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
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

pub(super) fn safe_directory(path: &Path) -> anyhow::Result<()> {
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

pub(super) fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
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

pub(super) fn read_record(directory: &Path, id: &str) -> anyhow::Result<Option<FlowHistory>> {
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

// Synchronous setup helpers for existing disk-format/expiry tests only.
#[cfg(test)]
pub(super) fn persist(directory: &Path, history: FlowHistory) -> anyhow::Result<()> {
    Disk::open(directory.to_owned(), now_ms())?.persist(history)
}
#[cfg(test)]
pub(super) fn cleanup(directory: &Path, now: i64) -> anyhow::Result<()> {
    Disk::open(directory.to_owned(), now)?.reap(now)
}
#[cfg(test)]
pub(super) fn cleanup_for_quota(
    directory: &Path,
    now: i64,
    needed: u64,
    quota: u64,
) -> anyhow::Result<()> {
    Disk::open(directory.to_owned(), now)?.make_room(now, needed, 0, quota)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(closed_at_ms: i64) -> FlowHistory {
        FlowHistory {
            flow_id: uuid::Uuid::new_v4().to_string(),
            service_id: "svc".into(),
            protocol_id: "raw".into(),
            opened_at_ms: closed_at_ms,
            closed_at_ms,
            bytes_client_to_server: 3,
            bytes_server_to_client: 0,
            client_to_server: vec![0, 255, 1],
            server_to_client: vec![],
            truncated_client_to_server: false,
            truncated_server_to_client: false,
            history_truncated: false,
        }
    }

    #[test]
    fn index_rebuilds_exact_bytes_and_evicts_oldest_without_rescan() {
        let dir = super::super::tests::Directory::new();
        let now = now_ms();
        let mut disk = Disk::open(dir.0.clone(), now).unwrap();
        let older = history(now - 1000);
        let newer = history(now);
        disk.persist(newer.clone()).unwrap();
        disk.persist(older.clone()).unwrap();
        let size = paths(&dir.0, &older.flow_id).0.metadata().unwrap().len();
        let mut disk = Disk::open(dir.0.clone(), now).unwrap();
        assert_eq!(disk.bytes, 2 * size);
        disk.make_room(now, 1, 0, 2 * size).unwrap();
        assert_eq!(disk.bytes, size);
        assert!(!paths(&dir.0, &older.flow_id).0.exists());
        assert!(read_record(&dir.0, &newer.flow_id).unwrap().is_some());
        disk.reap(now + (TTL_SECONDS + 1) * 1000).unwrap();
        assert!(disk.records.is_empty());
        assert_eq!(disk.bytes, 0);
        assert!(!paths(&dir.0, &newer.flow_id).1.exists());
    }

    #[test]
    fn failed_eviction_retains_accounting_and_is_retryable() {
        let dir = super::super::tests::Directory::new();
        let now = now_ms();
        let mut disk = Disk::open(dir.0.clone(), now).unwrap();
        let record = history(now);
        disk.persist(record.clone()).unwrap();
        let size = disk.bytes;
        let (_, metadata) = paths(&dir.0, &record.flow_id);
        fs::remove_file(&metadata).unwrap();
        fs::create_dir(&metadata).unwrap();
        assert!(disk.make_room(now, 1, 0, 1).is_err());
        assert_eq!(disk.bytes, size);
        assert_eq!(disk.records.len(), 1);
        fs::remove_dir(&metadata).unwrap();
        disk.make_room(now, 1, 0, 1).unwrap();
        assert_eq!(disk.bytes, 0);
        assert!(disk.records.is_empty());
    }

    #[test]
    fn record_count_is_bounded_even_when_payloads_compress_to_almost_nothing() {
        let dir = super::super::tests::Directory::new();
        let now = now_ms();
        let mut disk = Disk::open(dir.0.clone(), now).unwrap();
        // Populate accounting directly: this exercises the actual cap without
        // creating 130,000 files in a routine unit test.
        for i in 0..MAX_RECORDS {
            disk.records
                .insert((now, uuid::Uuid::from_u128(i as u128).to_string()), 0);
        }
        disk.make_room(now, 0, 1, DISK_QUOTA).unwrap();
        assert_eq!(disk.records.len(), MAX_RECORDS - 1);
        assert_eq!(disk.bytes, 0);
    }

    #[test]
    fn duplicate_write_cannot_replace_indexed_history() {
        let dir = super::super::tests::Directory::new();
        let now = now_ms();
        let mut disk = Disk::open(dir.0.clone(), now).unwrap();
        let record = history(now);
        disk.persist(record.clone()).unwrap();
        let size = disk.bytes;
        assert!(disk.persist(record.clone()).is_err());
        assert_eq!(disk.bytes, size);
        assert_eq!(disk.records.len(), 1);
        assert_eq!(read_record(&dir.0, &record.flow_id).unwrap(), Some(record));
    }
}
