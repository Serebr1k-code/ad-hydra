//! Single-owner bounded index of retained compressed files. Scan once at startup;
//! completed flows and expiry touch only new or evicted records.
use super::{
    Datagram, FILE_PREFIX, FlowHistory, MAX_DATAGRAMS, MAX_DIRECTION_BYTES, MAX_ENCODED_BYTES,
    MAX_HISTORY_BYTES, Metadata, TTL_SECONDS, now_ms,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

// Bounds index memory and small-file/inode use even for highly compressed data.
const MAX_RECORDS: usize = 65_536;

pub(super) fn paths(directory: &Path, id: &str) -> (PathBuf, PathBuf) {
    let stem = format!("{FILE_PREFIX}{id}");
    (
        directory.join(format!("{stem}.zst")),
        directory.join(format!("{stem}.json")),
    )
}

fn legacy_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{FILE_PREFIX}{id}.gz"))
}

// A single sidecar identifies one payload. Ambiguous pairs are rejected rather
// than silently ignoring bytes in quota accounting or selecting stale content.
fn payload_path(directory: &Path, id: &str) -> anyhow::Result<Option<PathBuf>> {
    let current = paths(directory, id).0;
    let legacy = legacy_path(directory, id);
    anyhow::ensure!(
        !(current.exists() && legacy.exists()),
        "ambiguous history payloads"
    );
    Ok(if current.exists() {
        Some(current)
    } else if legacy.exists() {
        Some(legacy)
    } else {
        None
    })
}

type Key = (i64, String);
struct Record {
    service: String,
    bytes: u64,
}
#[derive(Default)]
struct Usage {
    records: BTreeSet<Key>,
    bytes: u64,
}
pub(super) struct Disk {
    directory: PathBuf,
    records: BTreeMap<Key, Record>,
    services: HashMap<String, Usage>,
    bytes: u64,
    quota: u64,
    compression_level: i32,
}

impl Disk {
    pub(super) fn directory_path(&self) -> PathBuf {
        self.directory.clone()
    }
    pub(super) fn open(directory: PathBuf, now: i64) -> anyhow::Result<Self> {
        safe_directory(&directory)?;
        let policy = directory.join("history-policy.json");
        let (quota, compression_level) = if policy.exists() {
            safe_file(&policy)?;
            anyhow::ensure!(
                policy.metadata()?.len() <= 4096,
                "history policy exceeds bound"
            );
            let value: serde_json::Value = serde_json::from_slice(&fs::read(policy)?)?;
            let (quota, compression_level) = if let Some(quota) = value.as_u64() {
                (quota, hydra_compression::DEFAULT_LEVEL)
            } else {
                (
                    value
                        .get("per_service_bytes")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| anyhow::anyhow!("invalid history quota"))?,
                    value
                        .get("compression_level")
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .ok_or_else(|| anyhow::anyhow!("invalid compression level"))?,
                )
            };
            anyhow::ensure!(
                quota > 0
                    && quota
                        <= u64::from(hydra_domain::history::MAX_PER_SERVICE_MIB)
                            * hydra_domain::history::MIB,
                "invalid history quota"
            );
            anyhow::ensure!(
                (hydra_compression::MIN_LEVEL..=hydra_compression::MAX_LEVEL)
                    .contains(&compression_level),
                "invalid compression level"
            );
            (quota, compression_level)
        } else {
            (
                u64::from(hydra_domain::history::default_per_service_mib())
                    * hydra_domain::history::MIB,
                hydra_compression::DEFAULT_LEVEL,
            )
        };
        let mut disk = Self {
            directory,
            records: BTreeMap::new(),
            services: HashMap::new(),
            bytes: 0,
            quota,
            compression_level,
        };
        hydra_compression::set_level(compression_level)?;
        // Import existing flat files once. The service ID is already in each
        // sidecar, so no disk-format migration or per-flow directory scan is needed.
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
                .and_then(|s| s.strip_suffix(".gz").or_else(|| s.strip_suffix(".zst")))
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
            // Startup quota rotation may have already removed this directory entry.
            if !metadata_path.exists() {
                continue;
            }
            safe_file(&metadata_path)?;
            let metadata_size = fs::metadata(&metadata_path)?.len();
            anyhow::ensure!(metadata_size <= 4096, "history metadata exceeds bound");
            let detail: Metadata = serde_json::from_slice(&fs::read(&metadata_path)?)?;
            let Some(payload) = payload_path(&disk.directory, id)? else {
                remove_if_present(&metadata_path)?;
                continue;
            };
            if detail.flow_id != id
                || detail.closed_at_ms < now - TTL_SECONDS * 1000
                || !payload.exists()
            {
                remove_if_present(&payload)?;
                remove_if_present(&metadata_path)?;
                continue;
            }
            anyhow::ensure!(
                !detail.service_id.is_empty() && detail.service_id.len() <= 128,
                "invalid history service ID"
            );
            safe_file(&payload)?;
            let size = payload.metadata()?.len();
            anyhow::ensure!(
                size <= MAX_ENCODED_BYTES as u64 + 1024,
                "compressed history exceeds bound"
            );
            disk.index_record(
                (detail.closed_at_ms, id.into()),
                &detail.service_id,
                size + metadata_size,
            );
            disk.make_room(now, &detail.service_id, 0, 0, quota)?;
        }
        Ok(disk)
    }

    pub(super) fn quota(&self) -> u64 {
        self.quota
    }

    pub(super) fn compression_level(&self) -> i32 {
        self.compression_level
    }

    pub(super) fn set_quota(&mut self, quota: u64) -> anyhow::Result<()> {
        self.set_policy(quota, self.compression_level)
    }

    pub(super) fn set_policy(&mut self, quota: u64, compression_level: i32) -> anyhow::Result<()> {
        anyhow::ensure!(
            quota > 0
                && quota
                    <= u64::from(hydra_domain::history::MAX_PER_SERVICE_MIB)
                        * hydra_domain::history::MIB,
            "invalid history quota"
        );
        anyhow::ensure!(
            (hydra_compression::MIN_LEVEL..=hydra_compression::MAX_LEVEL)
                .contains(&compression_level),
            "invalid compression level"
        );
        if self.quota != quota || self.compression_level != compression_level {
            let temporary = self
                .directory
                .join(format!("{FILE_PREFIX}{}.tmp.policy", uuid::Uuid::new_v4()));
            let result = (|| -> anyhow::Result<()> {
                write_private(
                    &temporary,
                    &serde_json::to_vec(
                        &serde_json::json!({"per_service_bytes":quota,"compression_level":compression_level}),
                    )?,
                )?;
                fs::rename(&temporary, self.directory.join("history-policy.json"))?;
                fs::File::open(&self.directory)?.sync_all()?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result?;
            self.quota = quota;
            self.compression_level = compression_level;
            hydra_compression::set_level(compression_level)?;
        }
        // Enforce decreases immediately and retry any partially failed eviction.
        for service in self.services.keys().cloned().collect::<Vec<_>>() {
            self.make_room(now_ms(), &service, 0, 0, quota)?;
        }
        Ok(())
    }

    fn index_record(&mut self, key: Key, service: &str, bytes: u64) {
        let usage = self.services.entry(service.to_owned()).or_default();
        usage.records.insert(key.clone());
        usage.bytes += bytes;
        self.bytes += bytes;
        self.records.insert(
            key,
            Record {
                service: service.to_owned(),
                bytes,
            },
        );
    }

    fn remove(&mut self, key: &Key) -> anyhow::Result<()> {
        let Some(record) = self.records.get(key) else {
            return Ok(());
        };
        let (payload, metadata) = paths(&self.directory, &key.1);
        // Retain both indexes until both unlinks succeed, allowing an I/O retry.
        remove_if_present(&payload)?;
        remove_if_present(&legacy_path(&self.directory, &key.1))?;
        remove_if_present(&metadata)?;
        let usage = self
            .services
            .get_mut(&record.service)
            .expect("indexed service");
        usage.records.remove(key);
        usage.bytes = usage.bytes.saturating_sub(record.bytes);
        if usage.records.is_empty() {
            self.services.remove(&record.service);
        }
        self.bytes = self.bytes.saturating_sub(record.bytes);
        self.records.remove(key);
        Ok(())
    }

    pub(super) fn reap(&mut self, now: i64) -> anyhow::Result<()> {
        while let Some(key) = self
            .records
            .first_key_value()
            .and_then(|(key, _)| (key.0 < now - TTL_SECONDS * 1000).then(|| key.clone()))
        {
            self.remove(&key)?;
        }
        Ok(())
    }

    fn make_room(
        &mut self,
        now: i64,
        service: &str,
        needed: u64,
        reserve: usize,
        quota: u64,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(needed <= quota, "history record exceeds service disk quota");
        self.reap(now)?;
        loop {
            let usage = self.services.get(service);
            if usage.map_or(0, |u| u.bytes).saturating_add(needed) <= quota
                && self.records.len() + reserve <= MAX_RECORDS
            {
                return Ok(());
            }
            let key = usage
                .and_then(|u| u.records.first())
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("history index capacity exhausted"))?;
            // Capacity pressure rotates only the writing service, never a neighbour.
            self.remove(&key)?;
        }
    }

    pub(super) fn persist(&mut self, history: FlowHistory) -> anyhow::Result<()> {
        anyhow::ensure!(
            uuid::Uuid::parse_str(&history.flow_id).is_ok(),
            "invalid history flow id"
        );
        anyhow::ensure!(
            !history.service_id.is_empty() && history.service_id.len() <= 128,
            "invalid history service ID"
        );
        let payload = compressed_payload(&history)?;
        let details = serde_json::to_vec(&metadata(&history))?;
        anyhow::ensure!(details.len() <= 4096, "history metadata exceeds bound");
        let (payload_path, metadata_path) = paths(&self.directory, &history.flow_id);
        anyhow::ensure!(
            !payload_path.exists()
                && !legacy_path(&self.directory, &history.flow_id).exists()
                && !metadata_path.exists(),
            "history already exists"
        );
        let size = (payload.len() + details.len()) as u64;
        self.make_room(now_ms(), &history.service_id, size, 1, self.quota)?;
        let nonce = uuid::Uuid::new_v4();
        let payload_tmp = self.directory.join(format!("{FILE_PREFIX}{nonce}.tmp.zst"));
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
            self.index_record(
                (history.closed_at_ms, history.flow_id),
                &history.service_id,
                size,
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

fn compressed_payload(history: &FlowHistory) -> anyhow::Result<Vec<u8>> {
    let mut payload = Vec::new();
    if history.transport == "udp" || !history.datagrams.is_empty() {
        anyhow::ensure!(
            history.datagrams.len() <= MAX_DATAGRAMS,
            "history datagram bound"
        );
        let mut totals = [0usize; 2];
        let mut previous = 0;
        for datagram in &history.datagrams {
            anyhow::ensure!(
                (1..=2).contains(&datagram.direction) && datagram.sequence > previous,
                "invalid datagram metadata"
            );
            previous = datagram.sequence;
            totals[usize::from(datagram.direction == 2)] += datagram.data.len();
            payload.push(datagram.direction);
            payload.extend_from_slice(&datagram.occurred_at_ms.to_le_bytes());
            payload.extend_from_slice(&datagram.sequence.to_le_bytes());
            payload.extend_from_slice(&(datagram.data.len() as u32).to_le_bytes());
            payload.extend_from_slice(&datagram.data);
        }
        anyhow::ensure!(
            totals.iter().all(|n| *n <= MAX_DIRECTION_BYTES),
            "history direction bound"
        );
    } else {
        payload.extend_from_slice(&history.client_to_server);
        payload.extend_from_slice(&history.server_to_client);
        anyhow::ensure!(
            payload.len() <= MAX_HISTORY_BYTES,
            "history exceeds payload bound"
        );
    }
    anyhow::ensure!(
        payload.len() <= MAX_ENCODED_BYTES,
        "history exceeds encoded bound"
    );
    hydra_compression::encode(&payload)
}

fn decode_datagrams(payload: &[u8], count: usize) -> anyhow::Result<Vec<Datagram>> {
    anyhow::ensure!(count <= MAX_DATAGRAMS, "history datagram bound");
    let mut datagrams = Vec::with_capacity(count);
    let mut offset = 0usize;
    let mut totals = [0usize; 2];
    let mut previous = 0;
    for _ in 0..count {
        anyhow::ensure!(
            payload.len().saturating_sub(offset) >= 21,
            "incomplete datagram header"
        );
        let direction = payload[offset];
        let occurred_at_ms = i64::from_le_bytes(payload[offset + 1..offset + 9].try_into()?);
        let sequence = u64::from_le_bytes(payload[offset + 9..offset + 17].try_into()?);
        let len = u32::from_le_bytes(payload[offset + 17..offset + 21].try_into()?) as usize;
        offset += 21;
        anyhow::ensure!(
            (1..=2).contains(&direction) && sequence > previous && len <= payload.len() - offset,
            "invalid datagram framing"
        );
        previous = sequence;
        totals[usize::from(direction == 2)] += len;
        anyhow::ensure!(
            totals.iter().all(|n| *n <= MAX_DIRECTION_BYTES),
            "history direction bound"
        );
        datagrams.push(Datagram {
            direction,
            occurred_at_ms,
            sequence,
            data: payload[offset..offset + len].to_vec(),
        });
        offset += len;
    }
    anyhow::ensure!(offset == payload.len(), "history payload length mismatch");
    Ok(datagrams)
}

fn metadata(history: &FlowHistory) -> Metadata {
    Metadata {
        ordered_tcp: history.transport == "tcp" && !history.datagrams.is_empty(),
        transport: history.transport.clone(),
        datagram_count: history.datagrams.len(),
        datagrams_client_to_server: history.datagrams_client_to_server,
        datagrams_server_to_client: history.datagrams_server_to_client,
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
    let (_, metadata_path) = paths(directory, id);
    let Some(payload_path) = payload_path(directory, id)? else {
        return Ok(None);
    };
    if !metadata_path.exists() {
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
    anyhow::ensure!(
        details.transport == "tcp" || details.transport == "udp",
        "invalid history transport"
    );
    anyhow::ensure!(
        !details.ordered_tcp || details.transport == "tcp",
        "invalid ordered TCP history metadata"
    );
    let bound = if details.transport == "udp" || details.ordered_tcp {
        MAX_ENCODED_BYTES
    } else {
        MAX_HISTORY_BYTES
    };
    anyhow::ensure!(
        payload_path.metadata()?.len() <= MAX_ENCODED_BYTES as u64 + 1024,
        "compressed history exceeds bound"
    );
    let mut payload = hydra_compression::decode(&fs::read(payload_path)?, bound)?;
    let (client, server, datagrams) = if details.transport == "udp" || details.ordered_tcp {
        let datagrams = decode_datagrams(&payload, details.datagram_count)?;
        let mut counts = [0u64; 2];
        for datagram in &datagrams {
            counts[usize::from(datagram.direction == 2)] += 1;
        }
        anyhow::ensure!(
            details.ordered_tcp
                || (counts[0] <= details.datagrams_client_to_server
                    && counts[1] <= details.datagrams_server_to_client),
            "history datagram count mismatch"
        );
        if details.ordered_tcp {
            let mut client = Vec::new();
            let mut server = Vec::new();
            for chunk in &datagrams {
                if chunk.direction == 1 {
                    client.extend_from_slice(&chunk.data)
                } else {
                    server.extend_from_slice(&chunk.data)
                }
            }
            anyhow::ensure!(
                client.len() == details.client_len && server.len() == details.server_len,
                "ordered TCP length mismatch"
            );
            (client, server, datagrams)
        } else {
            (vec![], vec![], datagrams)
        }
    } else {
        anyhow::ensure!(
            payload.len() == details.client_len + details.server_len,
            "history payload length mismatch"
        );
        let server = payload.split_off(details.client_len);
        (payload, server, vec![])
    };
    Ok(Some(FlowHistory {
        transport: details.transport,
        datagrams,
        datagrams_client_to_server: details.datagrams_client_to_server,
        datagrams_server_to_client: details.datagrams_server_to_client,
        flow_id: details.flow_id,
        service_id: details.service_id,
        protocol_id: details.protocol_id,
        opened_at_ms: details.opened_at_ms,
        closed_at_ms: details.closed_at_ms,
        bytes_client_to_server: details.bytes_client_to_server,
        bytes_server_to_client: details.bytes_server_to_client,
        client_to_server: client,
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
    Disk::open(directory.to_owned(), now)?.make_room(now, "svc", needed, 0, quota)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(closed_at_ms: i64) -> FlowHistory {
        FlowHistory {
            transport: "tcp".into(),
            datagrams: vec![],
            datagrams_client_to_server: 0,
            datagrams_server_to_client: 0,
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
    fn imports_legacy_tcp_metadata_without_transport_or_datagram_fields() {
        let dir = super::super::tests::Directory::new();
        let record = history(now_ms());
        let mut detail = serde_json::to_value(metadata(&record)).unwrap();
        for key in [
            "transport",
            "datagram_count",
            "datagrams_client_to_server",
            "datagrams_server_to_client",
        ] {
            detail.as_object_mut().unwrap().remove(key);
        }
        let (payload, sidecar) = paths(&dir.0, &record.flow_id);
        write_private(&payload, &compressed_payload(&record).unwrap()).unwrap();
        write_private(&sidecar, &serde_json::to_vec(&detail).unwrap()).unwrap();
        assert_eq!(
            Disk::open(dir.0.clone(), now_ms()).unwrap().records.len(),
            1
        );
        assert_eq!(
            read_record(&dir.0, &record.flow_id).unwrap().unwrap(),
            record
        );
    }

    #[test]
    fn reads_legacy_gzip_payload_and_keeps_zstd_as_the_write_format() {
        use std::io::Write;
        let dir = super::super::tests::Directory::new();
        let record = history(now_ms());
        let plain = [
            record.client_to_server.as_slice(),
            record.server_to_client.as_slice(),
        ]
        .concat();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&plain).unwrap();
        let legacy = dir.0.join(format!("{FILE_PREFIX}{}.gz", record.flow_id));
        write_private(&legacy, &encoder.finish().unwrap()).unwrap();
        let (_, sidecar) = paths(&dir.0, &record.flow_id);
        write_private(&sidecar, &serde_json::to_vec(&metadata(&record)).unwrap()).unwrap();
        assert_eq!(
            read_record(&dir.0, &record.flow_id).unwrap().unwrap(),
            record
        );

        let fresh = history(now_ms());
        Disk::open(dir.0.clone(), now_ms())
            .unwrap()
            .persist(fresh.clone())
            .unwrap();
        let (zstd, _) = paths(&dir.0, &fresh.flow_id);
        assert!(
            fs::read(zstd)
                .unwrap()
                .starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
        );
    }

    #[test]
    fn rejects_malformed_udp_lengths_direction_order_and_excess_empty_records() {
        assert!(decode_datagrams(&[0; 20], 1).is_err());
        assert!(decode_datagrams(&[0; 21], 1).is_err());
        assert!(decode_datagrams(&[], MAX_DATAGRAMS + 1).is_err());
        let mut header = vec![1u8];
        header.extend_from_slice(&0i64.to_le_bytes());
        header.extend_from_slice(&1u64.to_le_bytes());
        header.extend_from_slice(&1u32.to_le_bytes());
        assert!(decode_datagrams(&header, 1).is_err());
        header[17..21].copy_from_slice(&0u32.to_le_bytes());
        assert!(decode_datagrams(&header, 1).unwrap()[0].data.is_empty());
        let duplicate = [header.clone(), header].concat();
        assert!(decode_datagrams(&duplicate, 2).is_err());
    }

    #[test]
    fn service_quotas_rotate_independently_and_survive_restart() {
        let dir = super::super::tests::Directory::new();
        let now = now_ms();
        let mut disk = Disk::open(dir.0.clone(), now).unwrap();
        let mut a = history(now - 2000);
        a.service_id = "aaa".into();
        let mut b = history(now - 1000);
        b.service_id = "bbb".into();
        disk.persist(a.clone()).unwrap();
        let size = paths(&dir.0, &a.flow_id).0.metadata().unwrap().len()
            + paths(&dir.0, &a.flow_id).1.metadata().unwrap().len();
        disk.set_quota(size * 2).unwrap();
        disk.persist(b.clone()).unwrap();
        let mut a2 = a.clone();
        a2.flow_id = uuid::Uuid::new_v4().to_string();
        a2.closed_at_ms = now;
        disk.persist(a2.clone()).unwrap();
        let mut a3 = a2.clone();
        a3.flow_id = uuid::Uuid::new_v4().to_string();
        disk.persist(a3.clone()).unwrap();
        assert!(read_record(&dir.0, &a.flow_id).unwrap().is_none());
        for item in [&b, &a2, &a3] {
            assert!(read_record(&dir.0, &item.flow_id).unwrap().is_some());
        }
        let mut disk = Disk::open(dir.0.clone(), now).unwrap();
        assert_eq!(disk.quota, size * 2);
        assert!(disk.bytes > disk.quota);
        disk.set_quota(size).unwrap();
        assert!(read_record(&dir.0, &b.flow_id).unwrap().is_some());
        assert_eq!(disk.records.len(), 2);
        assert!(disk.services.values().all(|usage| usage.bytes <= size));
    }

    #[test]
    fn startup_imports_existing_flat_history_into_service_budgets() {
        let dir = super::super::tests::Directory::new();
        let now = now_ms();
        let mut record = history(now);
        for name in ["aaa", "bbb"] {
            record.flow_id = uuid::Uuid::new_v4().to_string();
            record.service_id = name.into();
            write_private(
                &paths(&dir.0, &record.flow_id).0,
                &compressed_payload(&record).unwrap(),
            )
            .unwrap();
            write_private(
                &paths(&dir.0, &record.flow_id).1,
                &serde_json::to_vec(&metadata(&record)).unwrap(),
            )
            .unwrap();
        }
        let disk = Disk::open(dir.0.clone(), now).unwrap();
        assert_eq!(disk.services.len(), 2);
        assert_eq!(disk.records.len(), 2);
        assert_eq!(disk.quota, 128 * 1024 * 1024);
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
        let pair = paths(&dir.0, &older.flow_id);
        let size = pair.0.metadata().unwrap().len() + pair.1.metadata().unwrap().len();
        let mut disk = Disk::open(dir.0.clone(), now).unwrap();
        assert_eq!(disk.bytes, 2 * size);
        disk.make_room(now, "svc", 1, 0, 2 * size).unwrap();
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
        assert!(disk.make_room(now, "svc", 1, 0, 1).is_err());
        assert_eq!(disk.bytes, size);
        assert_eq!(disk.records.len(), 1);
        fs::remove_dir(&metadata).unwrap();
        disk.make_room(now, "svc", 1, 0, 1).unwrap();
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
            disk.index_record(
                (now, uuid::Uuid::from_u128(i as u128).to_string()),
                "svc",
                0,
            );
        }
        assert!(
            disk.make_room(now, "another-service", 0, 1, disk.quota)
                .is_err()
        );
        assert_eq!(disk.records.len(), MAX_RECORDS);
        disk.make_room(now, "svc", 0, 1, disk.quota).unwrap();
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
