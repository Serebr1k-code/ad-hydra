//! Private, bounded durable substitution journal. No flag values leave this module.
use anyhow::{Context, ensure};
use hydra_rewrite::{Format, Frame};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};
const MAX_ENTRIES: usize = 8192;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    namespace: String,
    original: String,
    substitute: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    record: Record,
    checksum: String,
}
struct State {
    file: File,
    forward: BTreeMap<(String, String), String>,
    reverse: BTreeMap<(String, String), String>,
    bytes: u64,
    failed: bool,
}
pub struct Store {
    state: Mutex<State>,
    pub created: AtomicU64,
    pub reused: AtomicU64,
    pub restored: AtomicU64,
    pub errors: AtomicU64,
    formats: std::sync::RwLock<BTreeMap<String, Arc<Format>>>,
}
fn checksum(r: &Record) -> anyhow::Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(r)?)))
}
impl Store {
    pub async fn open(path: PathBuf) -> anyhow::Result<Arc<Self>> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let parent = path
            .parent()
            .context("mapping journal requires a directory")?;
        let meta = std::fs::symlink_metadata(parent)?;
        let uid = nix::unistd::geteuid().as_raw();
        ensure!(
            meta.is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0,
            "mapping directory must be private and owned"
        );
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(&path)?;
        let meta = handle.metadata()?;
        ensure!(
            meta.is_file()
                && meta.uid() == uid
                && meta.nlink() == 1
                && meta.mode() & 0o077 == 0
                && meta.len() <= MAX_BYTES,
            "unsafe or oversized mapping journal"
        );
        handle
            .try_lock()
            .context("mapping journal is already in use")?;
        handle.sync_all()?;
        std::fs::File::open(parent)?.sync_all()?;
        let mut file = File::from_std(handle);
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "mapping journal exceeds quota"
        );
        let mut forward = BTreeMap::new();
        let mut reverse = BTreeMap::new();
        let mut valid = 0usize;
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if line.last() != Some(&b'\n') {
                break;
            }
            ensure!(line.len() <= 4096, "mapping record limit");
            let entry: Entry = serde_json::from_slice(line)?;
            let r = entry.record;
            ensure!(
                r.namespace.len() <= 128
                    && r.original.len() <= 256
                    && !r.original.is_empty()
                    && r.original.len() == r.substitute.len()
                    && r.original != r.substitute
                    && checksum(&r)? == entry.checksum,
                "invalid mapping journal record"
            );
            ensure!(
                !forward.contains_key(&(r.namespace.clone(), r.original.clone()))
                    && !reverse.contains_key(&(r.namespace.clone(), r.substitute.clone()))
                    && !reverse.contains_key(&(r.namespace.clone(), r.original.clone()))
                    && !forward.contains_key(&(r.namespace.clone(), r.substitute.clone())),
                "ambiguous mapping journal"
            );
            forward.insert(
                (r.namespace.clone(), r.original.clone()),
                r.substitute.clone(),
            );
            reverse.insert((r.namespace, r.substitute), r.original);
            valid += line.len();
            ensure!(forward.len() <= MAX_ENTRIES, "mapping entry limit");
        }
        file.set_len(valid as u64).await?;
        file.sync_data().await?;
        file.seek(std::io::SeekFrom::End(0)).await?;
        Ok(Arc::new(Self {
            created: AtomicU64::new(forward.len() as u64),
            reused: AtomicU64::new(0),
            restored: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            formats: Default::default(),
            state: Mutex::new(State {
                file,
                forward,
                reverse,
                bytes: valid as u64,
                failed: false,
            }),
        }))
    }
    pub fn configure(&self, services: &[hydra_domain::Service]) -> anyhow::Result<()> {
        let mut formats = BTreeMap::new();
        for s in services {
            if let Some(f) = &s.forwarding {
                formats.insert(namespace(s), Arc::new(Format::compile(&f.flag_pattern)?));
            }
        }
        *self.formats.write().unwrap_or_else(|e| e.into_inner()) = formats;
        Ok(())
    }
    pub fn format(&self, service: &hydra_domain::Service) -> anyhow::Result<Arc<Format>> {
        self.formats
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&namespace(service))
            .cloned()
            .context("forwarding configuration was not prepared")
    }
    pub async fn rewrite(
        &self,
        namespace: &str,
        format: &Format,
        frame: &mut Frame,
        reverse: bool,
    ) -> anyhow::Result<()> {
        self.rewrite_inner(namespace, format, frame, reverse).await
    }
    async fn rewrite_inner(
        &self,
        namespace: &str,
        format: &Format,
        frame: &mut Frame,
        reverse: bool,
    ) -> anyhow::Result<()> {
        if !frame.regions.iter().any(|spans| {
            let bytes: Vec<u8> = spans
                .iter()
                .flat_map(|r| frame.wire[r.clone()].iter().copied())
                .collect();
            !format.ranges(&bytes).is_empty()
        }) {
            return Ok(());
        }
        let mut state = self.state.lock().await;
        let mut plans = Vec::new();
        for spans in &frame.regions {
            let logical: Vec<u8> = spans
                .iter()
                .flat_map(|r| frame.wire[r.clone()].iter().copied())
                .collect();
            let mut replacements = Vec::new();
            for range in format.ranges(&logical) {
                let original = std::str::from_utf8(&logical[range.clone()])?.to_owned();
                let key = (namespace.to_owned(), original.clone());
                let value = if reverse {
                    let Some(value) = state.reverse.get(&key).cloned() else {
                        continue;
                    };
                    self.restored.fetch_add(1, Ordering::Relaxed);
                    value
                } else if let Some(value) = state.forward.get(&key) {
                    self.reused.fetch_add(1, Ordering::Relaxed);
                    value.clone()
                } else {
                    ensure!(!state.failed, "mapping storage unavailable");
                    ensure!(
                        !state.reverse.contains_key(&key),
                        "original flag conflicts with an existing substitute"
                    );
                    ensure!(
                        state.forward.len() < MAX_ENTRIES,
                        "mapping capacity exhausted"
                    );
                    let mut substitute = None;
                    for _ in 0..128 {
                        let value = String::from_utf8(format.generate()?)?;
                        let k = (namespace.to_owned(), value.clone());
                        if value != original
                            && !state.forward.contains_key(&k)
                            && !state.reverse.contains_key(&k)
                        {
                            substitute = Some(value);
                            break;
                        }
                    }
                    let substitute =
                        substitute.context("flag format has no available unique substitutes")?;
                    let record = Record {
                        namespace: namespace.into(),
                        original: original.clone(),
                        substitute: substitute.clone(),
                    };
                    let hash = checksum(&record)?;
                    let mut line = serde_json::to_vec(&Entry {
                        record,
                        checksum: hash,
                    })?;
                    line.push(b'\n');
                    ensure!(
                        state.bytes + line.len() as u64 <= MAX_BYTES,
                        "mapping journal capacity exhausted"
                    );
                    // A partial failed append poisons allocation until restart/recovery. Never append behind it.
                    state.failed = true;
                    state.file.write_all(&line).await?;
                    state.file.flush().await?;
                    state.file.sync_data().await?;
                    state.failed = false;
                    state.bytes += line.len() as u64;
                    state.forward.insert(key, substitute.clone());
                    state
                        .reverse
                        .insert((namespace.into(), substitute.clone()), original);
                    self.created.fetch_add(1, Ordering::Relaxed);
                    substitute
                };
                replacements.push((range, value.into_bytes()));
            }
            plans.push(replacements);
        }
        let mut plans = plans.into_iter();
        frame.rewrite(|_| Ok(plans.next().unwrap_or_default()))?;
        Ok(())
    }
}
pub fn namespace(service: &hydra_domain::Service) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                &service.id,
                service
                    .forwarding
                    .as_ref()
                    .map(|f| (&f.target_address, &f.flag_pattern))
            ))
            .expect("serializable forwarding config")
        )
    )
}

#[cfg(test)]
mod journal_tests {
    use super::*;
    #[tokio::test]
    async fn incomplete_tail_recovers_but_corruption_and_exhaustion_fail_closed() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("hydra-mapping-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("journal");
        let mut data = Vec::new();
        for i in 0..MAX_ENTRIES {
            let record = Record {
                namespace: "ns".into(),
                original: format!("FLAG{i:08}"),
                substitute: format!("FLAG{:08}", i + MAX_ENTRIES),
            };
            let hash = checksum(&record).unwrap();
            data.extend(
                serde_json::to_vec(&Entry {
                    record,
                    checksum: hash,
                })
                .unwrap(),
            );
            data.push(b'\n');
        }
        let valid = data.len();
        data.extend(b"{incomplete");
        std::fs::write(&path, &data).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let store = Store::open(path.clone()).await.unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), valid as u64);
        let format = Format::compile("FLAG[0-9]{8}").unwrap();
        let mut frame = Frame::payload(b"FLAG99999999".to_vec(), 0);
        assert!(
            store
                .rewrite("ns", &format, &mut frame, false)
                .await
                .is_err()
        );
        assert_eq!(frame.wire, b"FLAG99999999");
        let mut existing = Frame::payload(b"FLAG00000000".to_vec(), 0);
        store
            .rewrite("ns", &format, &mut existing, false)
            .await
            .unwrap();
        assert_ne!(existing.wire, b"FLAG00000000");
        drop(store);
        std::fs::write(&path, b"{broken}\n").unwrap();
        assert!(Store::open(path).await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
