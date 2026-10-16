use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

/// One SQLite owner; at most 32 pending operations and one blocking writer.
#[derive(Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
    slots: Arc<Semaphore>,
}
impl Database {
    pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_owned();
        tokio::task::spawn_blocking(move || {
            protect_storage(&path)?;
            let mut connection = Connection::open(path)?;
            connection.busy_timeout(std::time::Duration::from_secs(3))?;
            connection.pragma_update(None,"journal_mode","WAL")?;
            connection.pragma_update(None,"foreign_keys",true)?;
            connection.pragma_update(None,"max_page_count",65536)?;
            connection.pragma_update(None,"wal_autocheckpoint",1000)?;
            connection.pragma_update(None,"journal_size_limit",16*1024*1024)?;
            let tx=connection.transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, checksum TEXT NOT NULL);")?;
            for (version,migration) in [
                (1,include_str!("../../../../../migrations/0001_identity.sql")),
                (2,include_str!("../../../../../migrations/0002_services_filters.sql")),
                (3,include_str!("../../../../../migrations/0003_traffic.sql")),
                (4,include_str!("../../../../../migrations/0004_protocols.sql")),
                (5,include_str!("../../../../../migrations/0005_rule_counters.sql")),
                (6,include_str!("../../../../../migrations/0006_traffic_budget.sql")),
                (7,include_str!("../../../../../migrations/0007_flags.sql")),
                (8,include_str!("../../../../../migrations/0008_farm.sql")),
                (9,include_str!("../../../../../migrations/0009_critical_receipts.sql")),
                (10,include_str!("../../../../../migrations/0010_protocol_profiles.sql")),
                (11,include_str!("../../../../../migrations/0011_flag_triage.sql")),
                (12,include_str!("../../../../../migrations/0012_traffic_search.sql")),
                (13,include_str!("../../../../../migrations/0013_farm_retry.sql")),
                (14,include_str!("../../../../../migrations/0014_account_management.sql")),

            ] {
                let checksum=format!("{:x}",Sha256::digest(migration.as_bytes()));
                let existing=tx.query_row("SELECT checksum FROM schema_migrations WHERE version=?1",[version],|r|r.get::<_,String>(0));
                match existing {
                    Ok(value)=>anyhow::ensure!(value==checksum,"migration checksum mismatch"),
                    Err(rusqlite::Error::QueryReturnedNoRows)=>{tx.execute_batch(migration)?;tx.execute("INSERT INTO schema_migrations VALUES(?1,?2)",rusqlite::params![version,checksum])?;},
                    Err(e)=>return Err(e.into()),
                }
            }
            tx.commit()?;
            Ok(Self {connection:Arc::new(Mutex::new(connection)), slots:Arc::new(Semaphore::new(32))})
        }).await?
    }
    pub async fn run<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> anyhow::Result<T> + Send + 'static,
    {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("database busy"))?;
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut guard = connection
                .lock()
                .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
            operation(&mut guard)
        })
        .await?
    }
}
impl Database {
    pub async fn migrate(&self, version: i64, sql: &'static str) -> anyhow::Result<()> {
        self.run(move |db| {
            let tx = db.transaction()?;
            let checksum = format!("{:x}", Sha256::digest(sql.as_bytes()));
            let existing = tx.query_row(
                "SELECT checksum FROM schema_migrations WHERE version=?1",
                [version],
                |r| r.get::<_, String>(0),
            );
            match existing {
                Ok(value) => anyhow::ensure!(value == checksum, "migration checksum mismatch"),
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    tx.execute_batch(sql)?;
                    tx.execute(
                        "INSERT INTO schema_migrations VALUES(?1,?2)",
                        rusqlite::params![version, checksum],
                    )?;
                }
                Err(e) => return Err(e.into()),
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}

/// SQLite also creates WAL/SHM files beside the database. Require an owned,
/// private directory, and reject unsafe existing files rather than chmod user data.
fn protect_storage(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let uid = std::fs::metadata("/proc/self")?.uid();
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for component in parent.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        anyhow::ensure!(
            !std::fs::symlink_metadata(component)?
                .file_type()
                .is_symlink(),
            "database directory must not contain symlinks"
        );
    }
    let directory = std::fs::symlink_metadata(parent)?;
    anyhow::ensure!(
        directory.is_dir() && directory.uid() == uid && directory.permissions().mode() & 0o077 == 0,
        "database parent must be an owned private directory (0700)"
    );
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let file = std::path::PathBuf::from(name);
        match std::fs::symlink_metadata(&file) {
            Ok(metadata) => anyhow::ensure!(
                metadata.is_file()
                    && metadata.uid() == uid
                    && metadata.nlink() == 1
                    && metadata.permissions().mode() & 0o077 == 0,
                "database files must be owned private regular files without hard links"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if !path.exists() {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
            .sync_all()?;
    }
    Ok(())
}
