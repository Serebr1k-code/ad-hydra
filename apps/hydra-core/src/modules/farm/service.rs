use crate::modules::{
    audit::{self, Actor},
    auth::{AuthState, now},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    pin::Pin,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub transport: String,
    pub enabled: bool,
    pub revision: i64,
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub runtime: String,
    pub mode: String,
    pub timeout_seconds: u64,
    pub memory_mib: u64,
    pub cpu_percent: u64,
    /// Exact PyPI name==version pins prepared with uv before isolated execution.
    #[serde(default)]
    pub python_packages: Vec<String>,
}
impl Manifest {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            ["python", "elf", "shell"].contains(&self.runtime.as_str())
                && ["per_target", "batch"].contains(&self.mode.as_str())
                && (1..=120).contains(&self.timeout_seconds)
                && (32..=256).contains(&self.memory_mib)
                && (10..=100).contains(&self.cpu_percent),
            "invalid manifest"
        );
        crate::infrastructure::sandbox::validate_python_packages(&self.python_packages)?;
        anyhow::ensure!(
            self.runtime == "python" || self.python_packages.is_empty(),
            "packages require Python runtime"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Exploit {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub revision: i64,
    pub version: String,
    pub manifest: Manifest,
    pub target_ids: Vec<String>,
    pub interval_seconds: Option<u64>,
}
#[derive(Clone)]
pub struct Job {
    pub run_id: String,
    pub exploit_id: String,
    pub version: String,
    pub artifact: PathBuf,
    pub manifest: Manifest,
    pub targets: Vec<Target>,
    pub initiated_by: String,
}
pub struct RunOutput {
    pub status: String,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
}
pub trait Runner: Send + Sync {
    fn run(
        &self,
        job: Job,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<RunOutput>> + Send>>;
}
pub struct UnavailableRunner;
impl Runner for UnavailableRunner {
    fn run(
        &self,
        _: Job,
        _: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<RunOutput>> + Send>> {
        Box::pin(async { anyhow::bail!("sandbox prerequisites unavailable") })
    }
}
#[derive(Clone)]
pub struct Farm {
    pub auth: AuthState,
    pub(super) directory: PathBuf,
    pub(super) runner: Arc<dyn Runner>,
    pub(super) active: Arc<tokio::sync::Mutex<Option<(String, CancellationToken)>>>,
    _lock: Arc<std::fs::File>,
}
pub(super) fn identifier(s: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !s.is_empty()
            && s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "invalid identifier"
    );
    Ok(())
}
impl Farm {
    pub async fn open(
        auth: AuthState,
        directory: PathBuf,
        runner: Arc<dyn Runner>,
    ) -> anyhow::Result<Self> {
        if !directory.exists() {
            std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        }
        let m = std::fs::symlink_metadata(&directory)?;
        anyhow::ensure!(
            m.is_dir()
                && !m.file_type().is_symlink()
                && m.permissions().mode() & 0o077 == 0
                && m.uid() == std::fs::metadata("/proc/self")?.uid(),
            "farm upload directory must be private"
        );
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(directory.join(".lock"))?;
        lock.try_lock()?;
        auth.db
            .migrate(8, include_str!("../../../../../migrations/0008_farm.sql"))
            .await?;
        auth.db
            .migrate(
                13,
                include_str!("../../../../../migrations/0013_farm_retry.sql"),
            )
            .await?;
        auth.db
            .migrate(
                15,
                include_str!("../../../../../migrations/0015_python_packages.sql"),
            )
            .await?;
        auth.db
            .run(|db| {
                let tx = db.transaction()?;
                let rows = tx
                    .prepare("SELECT id,initiated_by FROM farm_runs WHERE status='running'")?
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                for (id, user) in rows {
                    tx.execute(
                        "UPDATE farm_runs SET status='interrupted',finished_at=?2 WHERE id=?1",
                        params![id, now()],
                    )?;
                    audit::append_change(
                        &tx,
                        &Actor {
                            kind: "system".into(),
                            id: "farm-scheduler".into(),
                        },
                        "farm.run.interrupted",
                        &id,
                        "restart",
                        true,
                        now(),
                        &serde_json::json!({"initiated_by":user}),
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(Self {
            auth,
            directory,
            runner,
            active: Default::default(),
            _lock: Arc::new(lock),
        })
    }
    pub async fn target(&self, a: Actor, t: Target) -> anyhow::Result<()> {
        identifier(&t.id)?;
        anyhow::ensure!(
            t.name.len() <= 128
                && !t.name.is_empty()
                && t.host.parse::<std::net::IpAddr>().is_ok()
                && t.port > 0
                && ["tcp", "udp"].contains(&t.transport.as_str()),
            "invalid target"
        );
        self.auth.db.run(move|db|{let tx=db.transaction()?;let old:Option<i64>=tx.query_row("SELECT revision FROM farm_targets WHERE id=?1",[&t.id],|r|r.get(0)).optional()?;anyhow::ensure!(old.unwrap_or(0)==t.revision,"revision conflict");let count:i64=tx.query_row("SELECT count(*) FROM farm_targets",[],|r|r.get(0))?;anyhow::ensure!(old.is_some() || count<1024,"target quota exhausted");tx.execute("INSERT INTO farm_targets VALUES(?1,?2,?3,?4,?5,?6,1,?7,?7,?8) ON CONFLICT(id) DO UPDATE SET name=excluded.name,host=excluded.host,port=excluded.port,transport=excluded.transport,enabled=excluded.enabled,revision=farm_targets.revision+1,updated_at=excluded.updated_at,author_id=excluded.author_id",params![t.id,t.name,t.host,t.port,t.transport,t.enabled,now(),a.id])?;audit::append(&tx,&a,"farm.target.write",&t.id,"farm",true,now())?;tx.commit()?;Ok(())}).await
    }
    pub async fn exploit(&self, a: Actor, e: Exploit) -> anyhow::Result<()> {
        identifier(&e.id)?;
        identifier(&e.version)?;
        e.manifest.validate()?;
        anyhow::ensure!(
            !e.name.is_empty()
                && e.name.len() <= 128
                && !e.target_ids.is_empty()
                && e.target_ids.len() <= 256
                && e.target_ids
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == e.target_ids.len()
                && e.interval_seconds.is_none_or(|n| (10..=86400).contains(&n)),
            "invalid exploit schedule"
        );
        self.auth.db.run(move|db|{let tx=db.transaction()?;let old:Option<i64>=tx.query_row("SELECT revision FROM farm_exploits WHERE id=?1",[&e.id],|r|r.get(0)).optional()?;anyhow::ensure!(old.unwrap_or(0)==e.revision,"revision conflict");let binding = serde_json::to_string(&(e.manifest.runtime.as_str(), &e.manifest.python_packages))?; let previous:Option<Option<String>>=tx.query_row("SELECT runtime_binding FROM farm_versions WHERE exploit_id=?1 AND version=?2",params![e.id,e.version],|r|r.get(0)).optional()?; if let Some(previous)=previous {anyhow::ensure!(previous.as_ref().is_none_or(|old|old==&binding),"immutable version runtime/packages differ; upload a new version");tx.execute("UPDATE farm_versions SET runtime_binding=?3 WHERE exploit_id=?1 AND version=?2 AND runtime_binding IS NULL",params![e.id,e.version,binding])?;} let count:i64=tx.query_row("SELECT count(*) FROM farm_exploits",[],|r|r.get(0))?;anyhow::ensure!(old.is_some() || count<64,"exploit quota exhausted");let mut endpoints = std::collections::HashSet::new();for id in &e.target_ids {if e.manifest.mode == "batch" {let endpoint:(String,u16,String)=tx.query_row("SELECT host,port,transport FROM farm_targets WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;anyhow::ensure!(endpoints.insert(endpoint),"duplicate batch target endpoint: selected target aliases share an address and transport");}anyhow::ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM farm_targets WHERE id=?1 AND enabled=1)",[id],|r|r.get::<_,bool>(0))?,"target unavailable");}tx.execute("INSERT INTO farm_exploits VALUES(?1,?2,?3,1,?4,?5,?6,?7,?8,?9,?10,?10) ON CONFLICT(id) DO UPDATE SET name=excluded.name,enabled=excluded.enabled,revision=farm_exploits.revision+1,version=excluded.version,manifest=excluded.manifest,target_ids=excluded.target_ids,interval_seconds=excluded.interval_seconds,next_run=excluded.next_run,initiated_by=excluded.initiated_by,updated_at=excluded.updated_at",params![e.id,e.name,e.enabled,e.version,serde_json::to_string(&e.manifest)?,serde_json::to_string(&e.target_ids)?,e.interval_seconds.map(|n|n as i64),e.interval_seconds.map(|n|now()+n as i64),a.id,now()])?;audit::append(&tx,&a,"farm.exploit.write",&e.id,"farm",true,now())?;tx.commit()?;Ok(())}).await
    }
    pub async fn upload(
        &self,
        a: Actor,
        id: String,
        version: String,
        bytes: Vec<u8>,
    ) -> anyhow::Result<String> {
        identifier(&id)?;
        identifier(&version)?;
        anyhow::ensure!(
            !bytes.is_empty() && bytes.len() <= 2 * 1024 * 1024,
            "upload size invalid"
        );
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let path = self.directory.join(format!("{id}-{version}-{hash}"));
        let path2 = path.clone();
        let hash2 = hash.clone();
        self.auth.db.run(move|db|{let tx=db.transaction()?;let n:i64=tx.query_row("SELECT count(*) FROM farm_versions",[],|r|r.get(0))?;anyhow::ensure!(n<256,"version quota exhausted");anyhow::ensure!(!tx.query_row("SELECT EXISTS(SELECT 1 FROM farm_versions WHERE exploit_id=?1 AND version=?2)",params![id,version],|r|r.get::<_,bool>(0))?,"immutable version exists");anyhow::ensure!(std::fs::read_dir(path2.parent().ok_or_else(||anyhow::anyhow!("missing upload directory"))?)?.take(258).count()<257,"upload file quota exhausted (including orphans)");let mut f=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path2)?;f.write_all(&bytes)?;f.sync_all()?;tx.execute("INSERT INTO farm_versions(exploit_id,version,sha256,path,created_at,author_id) VALUES(?1,?2,?3,?4,?5,?6)",params![id,version,hash2,path2.to_string_lossy(),now(),a.id])?;if let Some(manifest)=tx.query_row("SELECT manifest FROM farm_exploits WHERE id=?1 AND version=?2",params![id,version],|r|r.get::<_,String>(0)).optional()? {let manifest:Manifest=serde_json::from_str(&manifest)?;let binding=serde_json::to_string(&(manifest.runtime,&manifest.python_packages))?;tx.execute("UPDATE farm_versions SET runtime_binding=?3 WHERE exploit_id=?1 AND version=?2",params![id,version,binding])?;} audit::append(&tx,&a,"farm.version.upload",&id,"farm",true,now())?;tx.commit()?;Ok(())}).await?;
        Ok(hash)
    }
}
impl Farm {
    pub async fn delete(
        &self,
        actor: Actor,
        kind: String,
        id: String,
        revision: i64,
    ) -> anyhow::Result<()> {
        self.auth
            .db
            .run(move |db| {
                let tx = db.transaction()?;
                let sql = match kind.as_str() {
                    "targets" => "DELETE FROM farm_targets WHERE id=?1 AND revision=?2",
                    "exploits" => "DELETE FROM farm_exploits WHERE id=?1 AND revision=?2",
                    _ => anyhow::bail!("invalid resource"),
                };
                anyhow::ensure!(
                    tx.execute(sql, params![id, revision])? == 1,
                    "revision conflict or missing resource"
                );
                audit::append(&tx, &actor, "farm.delete", &id, "farm", true, now())?;
                tx.commit()?;
                Ok(())
            })
            .await
    }
}
