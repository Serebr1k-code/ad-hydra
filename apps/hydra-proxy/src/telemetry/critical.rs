//! Bounded durable critical events. Producers only enqueue; one thread owns disk I/O.
use hydra_wire::v1::{self, event::Payload};
use prost::Message;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, SystemTime},
};
use tokio::sync::oneshot;

const LIVE_LIMIT: usize = 2 * 1024 * 1024;
const EVENT_LIMIT: usize = 64 * 1024;
const QUEUE_LIMIT: usize = 64;
const HEADER: usize = 8 + 36 + 8 + 8;
const MAGIC: &[u8; 8] = b"HYDRAC01";

enum Command {
    Emit(Box<v1::Event>),
    Replay(
        String,
        u64,
        usize,
        oneshot::Sender<io::Result<Vec<v1::EventBatch>>>,
    ),
    Ack(String, u64, oneshot::Sender<io::Result<u64>>),
}
#[derive(Default)]
struct Metrics {
    bytes: AtomicU64,
    dropped: AtomicU64,
    pending_loss: AtomicU64,
}
impl Metrics {
    fn loss(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        self.pending_loss.fetch_add(1, Ordering::Relaxed);
    }
}
pub struct CriticalSpool {
    epoch: String,
    tx: Option<SyncSender<Command>>,
    metrics: Arc<Metrics>,
    worker: Option<thread::JoinHandle<()>>,
}
impl CriticalSpool {
    /// Startup-only blocking filesystem access. The directory must have one owner.
    pub fn open(path: PathBuf) -> io::Result<Arc<Self>> {
        use std::os::unix::fs::MetadataExt;
        for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            if ancestor.exists() && fs::symlink_metadata(ancestor)?.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "spool symlink ancestor",
                ));
            }
        }
        if !path.exists() {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir()
            || metadata.uid() != fs::metadata("/proc/self")?.uid()
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "spool directory must be private and owned",
            ));
        }

        let mut disk = Disk::load(path)?;
        let epoch = disk.epoch.clone();
        let metrics = Arc::new(Metrics::default());
        metrics.bytes.store(disk.size() as u64, Ordering::Relaxed);
        let worker_metrics = metrics.clone();
        let (tx, rx) = mpsc::sync_channel(QUEUE_LIMIT);
        let worker = thread::Builder::new().name("critical-spool".into()).spawn(move || {
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Command::Emit(event)) => {
                        if let Err(error) = disk.append(*event) {
                            worker_metrics.loss();
                            let dropped = worker_metrics.dropped.load(Ordering::Relaxed);
                            if dropped.is_power_of_two() {
                                tracing::warn!(kind = ?error.kind(), dropped, "critical spool rejected event");
                            }
                        }
                    }
                    Ok(Command::Replay(epoch, after, limit, reply)) => {
                        if disk.poisoned {
                            let _ = reply.send(Err(io::Error::other("critical spool durability uncertain; restart required")));
                            continue;
                        }
                        let after = if epoch == disk.epoch { after } else { 0 };
                        let batches = disk.events.iter().enumerate()
                            .filter(|(index, _)| disk.ack + 1 + *index as u64 > after)
                            .take(limit.clamp(1, 256))
                            .map(|(index, event)| v1::EventBatch {
                                proxy_epoch_id: disk.epoch.clone(),
                                first_sequence: disk.ack + 1 + index as u64,
                                critical: true, events: vec![event.clone()],
                            }).collect();
                        let _ = reply.send(Ok(batches));
                    }
                    Ok(Command::Ack(epoch, through, reply)) => {
                        let _ = reply.send(disk.acknowledge(&epoch, through));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                // Keep loss pending while quota/disk is unavailable; retry after ACK.
                let count = worker_metrics.pending_loss.swap(0, Ordering::Relaxed);
                if count > 0 {
                    let loss = v1::Event {
                        event_id: uuid::Uuid::new_v4().to_string(),
                        occurred_at: Some(SystemTime::now().into()),
                        payload: Some(Payload::Loss(v1::EventLoss {
                            count, reason: "critical_queue_quota_or_disk_loss".into(),
                        })), ..Default::default()
                    };
                    if disk.append(loss).is_err() {
                        worker_metrics.pending_loss.fetch_add(count, Ordering::Relaxed);
                    }
                }
                worker_metrics.bytes.store(disk.size() as u64, Ordering::Relaxed);
            }
        })?;
        Ok(Arc::new(Self {
            epoch,
            tx: Some(tx),
            metrics,
            worker: Some(worker),
        }))
    }
    /// True means admitted to memory, not yet durably committed. Never waits for I/O.
    pub fn try_emit(&self, event: v1::Event) -> bool {
        if !matches!(event.payload, Some(Payload::Flag(_) | Payload::Loss(_)))
            || event.encoded_len() > EVENT_LIMIT
            || self
                .tx
                .as_ref()
                .is_none_or(|tx| tx.try_send(Command::Emit(Box::new(event))).is_err())
        {
            self.metrics.loss();
            return false;
        }
        true
    }
    pub fn epoch(&self) -> &str {
        &self.epoch
    }
    pub fn bytes(&self) -> u64 {
        self.metrics.bytes.load(Ordering::Relaxed)
    }
    pub fn dropped(&self) -> u64 {
        self.metrics.dropped.load(Ordering::Relaxed)
    }
    pub async fn replay(
        &self,
        epoch: &str,
        after: u64,
        limit: usize,
    ) -> io::Result<Vec<v1::EventBatch>> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Replay(epoch.into(), after, limit, tx))?;
        rx.await
            .map_err(|_| io::Error::other("critical spool stopped"))?
    }
    pub async fn ack(&self, epoch: &str, through: u64) -> io::Result<u64> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Ack(epoch.into(), through, tx))?;
        rx.await
            .map_err(|_| io::Error::other("critical spool stopped"))?
    }
    fn send(&self, command: Command) -> io::Result<()> {
        self.tx
            .as_ref()
            .ok_or_else(|| io::Error::other("critical spool stopped"))?
            .try_send(command)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "critical spool command queue full",
                )
            })
    }
}
impl Drop for CriticalSpool {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Disk {
    path: PathBuf,
    epoch: String,
    ack: u64,
    next: u64,
    events: VecDeque<v1::Event>,
    poisoned: bool,
}
impl Disk {
    fn size(&self) -> usize {
        HEADER
            + self
                .events
                .iter()
                .map(|e| 4 + e.encoded_len())
                .sum::<usize>()
    }
    fn load(path: PathBuf) -> io::Result<Self> {
        let mut disk = Self {
            path,
            epoch: uuid::Uuid::new_v4().to_string(),
            ack: 0,
            next: 1,
            events: VecDeque::new(),
            poisoned: false,
        };
        let file = disk.path.join("events.bin");
        if file.exists() {
            let mut input = File::open(file)?;
            if input.metadata()?.len() > LIVE_LIMIT as u64 {
                return Err(invalid("critical spool exceeds quota"));
            }
            let mut bytes = Vec::new();
            input.read_to_end(&mut bytes)?;
            if bytes.len() < HEADER || &bytes[..8] != MAGIC {
                return Err(invalid("invalid critical spool header"));
            }
            disk.epoch =
                String::from_utf8(bytes[8..44].to_vec()).map_err(|_| invalid("invalid epoch"))?;
            uuid::Uuid::parse_str(&disk.epoch).map_err(|_| invalid("invalid epoch"))?;
            disk.ack = u64::from_le_bytes(
                bytes[44..52]
                    .try_into()
                    .map_err(|_| invalid("invalid ack"))?,
            );
            disk.next = u64::from_le_bytes(
                bytes[52..60]
                    .try_into()
                    .map_err(|_| invalid("invalid sequence"))?,
            );
            let mut offset = HEADER;
            while offset < bytes.len() {
                let len_bytes = bytes
                    .get(offset..offset + 4)
                    .ok_or_else(|| invalid("truncated event length"))?;
                let len = u32::from_le_bytes(
                    len_bytes
                        .try_into()
                        .map_err(|_| invalid("invalid length"))?,
                ) as usize;
                offset += 4;
                if len > EVENT_LIMIT {
                    return Err(invalid("oversized spool event"));
                }
                let body = bytes
                    .get(offset..offset + len)
                    .ok_or_else(|| invalid("truncated event"))?;
                let event = v1::Event::decode(body).map_err(|_| invalid("invalid spool event"))?;
                if !matches!(event.payload, Some(Payload::Flag(_) | Payload::Loss(_))) {
                    return Err(invalid("noncritical spool event"));
                }
                disk.events.push_back(event);
                offset += len;
            }
            if disk.next
                != disk
                    .ack
                    .checked_add(disk.events.len() as u64)
                    .and_then(|v| v.checked_add(1))
                    .ok_or_else(|| invalid("sequence overflow"))?
            {
                return Err(invalid("inconsistent spool sequence"));
            }
        } else {
            disk.persist()?;
        }
        // A leftover temporary file was never committed; the canonical file wins.
        match fs::remove_file(disk.path.join("events.tmp")) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(disk)
    }
    fn persist(&mut self) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other(
                "critical spool durability uncertain; restart required",
            ));
        }
        let mut bytes = Vec::with_capacity(self.size());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(self.epoch.as_bytes());
        bytes.extend_from_slice(&self.ack.to_le_bytes());
        bytes.extend_from_slice(&self.next.to_le_bytes());
        for event in &self.events {
            bytes.extend_from_slice(&(event.encoded_len() as u32).to_le_bytes());
            event.encode(&mut bytes).map_err(io::Error::other)?;
        }
        if bytes.len() > LIVE_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "critical spool quota exhausted",
            ));
        }
        let temp = self.path.join("events.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(temp, self.path.join("events.bin"))?;
        let sync = File::open(&self.path).and_then(|directory| directory.sync_all());
        if sync.is_err() {
            self.poisoned = true;
        }
        sync
    }
    fn append(&mut self, event: v1::Event) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other(
                "critical spool durability uncertain; restart required",
            ));
        }
        if self.size() + 4 + event.encoded_len() > LIVE_LIMIT || self.next == u64::MAX {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "critical spool quota exhausted",
            ));
        }
        self.events.push_back(event);
        self.next += 1;
        if let Err(error) = self.persist() {
            if !self.poisoned {
                self.events.pop_back();
                self.next -= 1;
            }
            return Err(error);
        }
        Ok(())
    }
    fn acknowledge(&mut self, epoch: &str, through: u64) -> io::Result<u64> {
        if self.poisoned {
            return Err(io::Error::other(
                "critical spool durability uncertain; restart required",
            ));
        }
        if epoch != self.epoch || through >= self.next {
            return Err(invalid("critical ACK epoch or sequence mismatch"));
        }
        if through <= self.ack {
            return Ok(self.ack);
        }
        let old_ack = self.ack;
        let removed: VecDeque<_> = self.events.drain(..(through - old_ack) as usize).collect();
        self.ack = through;
        if let Err(error) = self.persist() {
            if !self.poisoned {
                self.ack = old_ack;
                for event in removed.into_iter().rev() {
                    self.events.push_front(event);
                }
            }
            return Err(error);
        }
        Ok(self.ack)
    }
}
fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
