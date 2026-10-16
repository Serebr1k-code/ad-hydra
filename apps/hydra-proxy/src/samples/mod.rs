//! Explicitly requested, short-lived raw samples. No background traffic recording.
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
const MAX_SAMPLE: usize = 16 * 1024;
const MAX_SLOTS: usize = 256;
const MAX_ACTIVE: usize = 16;
const TTL: Duration = Duration::from_secs(30);
#[derive(Clone, Debug)]
pub struct Sample {
    pub data: Vec<u8>,
    pub truncated: bool,
    pub expires_at: i64,
}
struct Request {
    id: String,
    deadline: Instant,
    expires_at: i64,
}
#[derive(Clone)]
struct Stored {
    id: String,
    deadline: Instant,
    expires_at: i64,
    truncated: bool,
}
struct Index {
    active: HashMap<String, Request>,
    stored: VecDeque<Stored>,
}
struct Write {
    request: Request,
    data: Vec<u8>,
    truncated: bool,
}
pub struct SampleRing {
    directory: PathBuf,
    index: Arc<Mutex<Index>>,
    writer: mpsc::Sender<Write>,
    dropped: Arc<AtomicU64>,
    active_hint: AtomicBool,
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn sample_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("hydra-sample-{id}.bin"))
}
fn safe_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let m = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        m.is_file()
            && m.nlink() == 1
            && m.uid() == std::fs::metadata("/proc/self")?.uid()
            && m.permissions().mode() & 0o077 == 0,
        "sample file is not private"
    );
    Ok(())
}
impl SampleRing {
    /// The directory must exist, be owned by Proxy's uid and have mode 0700.
    pub async fn open(directory: impl AsRef<Path>) -> anyhow::Result<Arc<Self>> {
        let directory = directory.as_ref().to_owned();
        let checked = directory.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            for ancestor in checked.ancestors().filter(|p| !p.as_os_str().is_empty()) {
                anyhow::ensure!(
                    !std::fs::symlink_metadata(ancestor)?
                        .file_type()
                        .is_symlink(),
                    "sample directory contains symlink"
                );
            }
            let m = std::fs::symlink_metadata(&checked)?;
            anyhow::ensure!(
                m.is_dir()
                    && m.uid() == std::fs::metadata("/proc/self")?.uid()
                    && m.permissions().mode() & 0o077 == 0,
                "sample directory must be private"
            );
            // Samples deliberately do not survive Proxy restart. Remove only our
            // exact UUID filenames; unrelated directory contents are untouched.
            for entry in std::fs::read_dir(&checked)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(id) = name
                    .strip_prefix("hydra-sample-")
                    .and_then(|n| n.strip_suffix(".bin"))
                else {
                    continue;
                };
                if uuid::Uuid::parse_str(id).is_ok() {
                    safe_file(&entry.path())?;
                    std::fs::remove_file(entry.path())?;
                }
            }
            Ok(())
        })
        .await??;
        let index = Arc::new(Mutex::new(Index {
            active: HashMap::new(),
            stored: VecDeque::new(),
        }));
        let (writer, mut reader) = mpsc::channel::<Write>(16);
        let dropped = Arc::new(AtomicU64::new(0));
        let ring = Arc::new(Self {
            directory: directory.clone(),
            index: index.clone(),
            writer,
            dropped: dropped.clone(),
            active_hint: AtomicBool::new(false),
        });
        // One disk worker. Timeout reaps expired files even with no further traffic.
        tokio::spawn(async move {
            loop {
                let write = tokio::select! {value=reader.recv()=>match value {Some(v)=>Some(v),None=>break},_=tokio::time::sleep(Duration::from_secs(1))=>None};
                let dir = directory.clone();
                let index = index.clone();
                let result =
                    tokio::task::spawn_blocking(move || persist(&dir, &index, write)).await;
                if !matches!(result, Ok(Ok(()))) {
                    dropped.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!("sample disk write or cleanup failed");
                }
            }
            let dir = directory;
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(mut state) = index.lock() {
                    for item in state.stored.drain(..) {
                        let _ = std::fs::remove_file(sample_path(&dir, &item.id));
                    }
                }
            })
            .await;
        });
        Ok(ring)
    }
    /// Request the next chunk for a known active flow. Caller owns auth/flow validation.
    pub fn request_capture(&self, flow_id: String) -> anyhow::Result<String> {
        anyhow::ensure!(
            !flow_id.is_empty() && flow_id.len() <= 128,
            "invalid flow id"
        );
        let mut index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("sample index unavailable"))?;
        index.active.retain(|_, r| r.deadline > Instant::now());
        if let Some(request) = index.active.get(&flow_id) {
            return Ok(request.id.clone());
        }
        anyhow::ensure!(
            index.active.len() < MAX_ACTIVE,
            "sample capture quota exhausted"
        );
        let id = uuid::Uuid::new_v4().to_string();
        index.active.insert(
            flow_id,
            Request {
                id: id.clone(),
                deadline: Instant::now() + TTL,
                expires_at: now() + 30,
            },
        );
        self.active_hint.store(true, Ordering::Release);
        Ok(id)
    }
    /// Nonblocking: false means inactive/expired or queue/lock unavailable.
    /// Count dropped() changes to distinguish queue/lock loss from inactivity.
    pub fn try_capture(&self, flow_id: &str, bytes: &[u8]) -> bool {
        if bytes.is_empty() || !self.active_hint.load(Ordering::Acquire) {
            return false;
        }
        let Ok(mut index) = self.index.try_lock() else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        index.active.retain(|_, r| r.deadline > Instant::now());
        self.active_hint
            .store(!index.active.is_empty(), Ordering::Release);
        let Some(request) = index.active.remove(flow_id) else {
            return false;
        };
        if request.deadline <= Instant::now() {
            return false;
        }
        let Ok(slot) = self.writer.try_reserve() else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            index.active.insert(flow_id.to_owned(), request);
            return false;
        };
        self.active_hint
            .store(!index.active.is_empty(), Ordering::Release);
        let data = bytes[..bytes.len().min(MAX_SAMPLE)].to_vec();
        slot.send(Write {
            request,
            data,
            truncated: bytes.len() > MAX_SAMPLE,
        });
        true
    }
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    pub async fn get(&self, id: &str) -> anyhow::Result<Option<Sample>> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Ok(None);
        }
        let item = {
            let index = self
                .index
                .lock()
                .map_err(|_| anyhow::anyhow!("sample index unavailable"))?;
            index
                .stored
                .iter()
                .find(|s| s.id == id && s.deadline > Instant::now())
                .cloned()
        };
        let Some(item) = item else { return Ok(None) };
        let path = sample_path(&self.directory, id);
        let data = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<u8>>> {
            use std::io::Read;
            match safe_file(&path) {
                Ok(()) => {}
                Err(error) => {
                    if !path.exists() {
                        return Ok(None);
                    }
                    return Err(error);
                }
            }
            let mut data = Vec::new();
            match std::fs::File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            }
            .take((MAX_SAMPLE + 1) as u64)
            .read_to_end(&mut data)?;
            anyhow::ensure!(data.len() <= MAX_SAMPLE, "invalid sample size");
            Ok(Some(data))
        })
        .await??;
        let index = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("sample index unavailable"))?;
        if item.deadline <= Instant::now() || !index.stored.iter().any(|s| s.id == id) {
            return Ok(None);
        }
        Ok(data.map(|data| Sample {
            data,
            truncated: item.truncated,
            expires_at: item.expires_at,
        }))
    }
}
fn persist(directory: &Path, index: &Mutex<Index>, write: Option<Write>) -> anyhow::Result<()> {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt};
    let mut state = index
        .lock()
        .map_err(|_| anyhow::anyhow!("sample index unavailable"))?;
    let at = Instant::now();
    state.active.retain(|_, r| r.deadline > at);
    while state
        .stored
        .front()
        .is_some_and(|s| s.deadline <= at || (write.is_some() && state.stored.len() >= MAX_SLOTS))
    {
        let old = state.stored.front().expect("front existed");
        match std::fs::remove_file(sample_path(directory, &old.id)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        state.stored.pop_front();
    }
    if let Some(write) = write {
        if write.request.deadline <= at {
            return Ok(());
        }
        let path = sample_path(directory, &write.request.id);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        if let Err(error) = file.write_all(&write.data) {
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
        state.stored.push_back(Stored {
            id: write.request.id,
            deadline: write.request.deadline,
            expires_at: write.request.expires_at,
            truncated: write.truncated,
        });
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn directory() -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("hydra-samples-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    async fn wait(ring: &SampleRing, id: &str) -> Sample {
        for _ in 0..100 {
            if let Some(s) = ring.get(id).await.unwrap() {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(10)).await
        }
        panic!("sample never persisted")
    }
    #[tokio::test]
    async fn explicit_capture_is_bounded_and_generation_expires() {
        let dir = directory();
        let ring = SampleRing::open(&dir).await.unwrap();
        assert!(!ring.try_capture("flow", b"not requested"));
        let id = ring.request_capture("flow".into()).unwrap();
        assert_eq!(id, ring.request_capture("flow".into()).unwrap());
        assert!(ring.try_capture("flow", &vec![42; MAX_SAMPLE + 1]));
        let sample = wait(&ring, &id).await;
        assert_eq!(sample.data.len(), MAX_SAMPLE);
        assert!(sample.truncated);
        assert!(!ring.try_capture("flow", b"second"));
        {
            let mut state = ring.index.lock().unwrap();
            state.stored[0].deadline = Instant::now() - Duration::from_secs(1);
        }
        assert!(ring.get(&id).await.unwrap().is_none());
        persist(&dir, &ring.index, None).unwrap();
        assert!(!sample_path(&dir, &id).exists());
        assert_ne!(id, ring.request_capture("flow".into()).unwrap());
        drop(ring);
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn active_and_disk_quotas_are_real() {
        let dir = directory();
        let ring = SampleRing::open(&dir).await.unwrap();
        for i in 0..MAX_ACTIVE {
            ring.request_capture(format!("flow{i}")).unwrap();
        }
        assert!(ring.request_capture("overflow".into()).is_err());
        ring.index.lock().unwrap().active.clear();
        let first = ring.request_capture("flow".into()).unwrap();
        ring.try_capture("flow", &vec![1; MAX_SAMPLE]);
        wait(&ring, &first).await;
        for _ in 0..MAX_SLOTS {
            let id = ring.request_capture("flow".into()).unwrap();
            assert!(ring.try_capture("flow", &vec![1; MAX_SAMPLE]));
            wait(&ring, &id).await;
        }
        assert!(ring.get(&first).await.unwrap().is_none());
        let bytes: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert!(bytes <= 4 * 1024 * 1024);
        drop(ring);
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod storage_security_tests {
    use super::*;
    #[tokio::test]
    async fn rejects_symlink_directory() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("hydra-sample-symlink-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let link = root.join("link");
        symlink(&root, &link).unwrap();
        assert!(SampleRing::open(&link).await.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
