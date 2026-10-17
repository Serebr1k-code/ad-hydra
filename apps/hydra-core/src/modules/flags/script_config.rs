use super::Flags;
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use utoipa::ToSchema;

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitterConfig {
    #[serde(default = "batch_size")]
    pub batch_size: u32,
    pub revision: i64,
    pub enabled: bool,
    pub flag_pattern: String,
    #[serde(default)]
    pub python_packages: Vec<String>,
    pub source: String,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn batch_size() -> u32 {
    240
}
fn timeout() -> u64 {
    20
}
impl SubmitterConfig {
    pub(super) fn validate(&self) -> anyhow::Result<regex::Regex> {
        anyhow::ensure!(
            self.source.len() <= 2 * 1024 * 1024 && !self.source.contains('\0'),
            "Python source exceeds 2 MiB or contains NUL"
        );
        anyhow::ensure!(
            !self.enabled || !self.source.trim().is_empty(),
            "Python source is required before enabling submission"
        );
        anyhow::ensure!(
            (1..=120).contains(&self.timeout_seconds),
            "submitter timeout must be 1..120 seconds"
        );
        anyhow::ensure!(
            (1..=240).contains(&self.batch_size),
            "batch size must be 1..240 flags"
        );
        crate::modules::farm::validate_python_packages(&self.python_packages)?;
        compile_pattern(&self.flag_pattern)
    }
}
pub(super) fn compile_pattern(pattern: &str) -> anyhow::Result<regex::Regex> {
    anyhow::ensure!(
        !pattern.is_empty() && pattern.len() <= 4096,
        "flag regex must contain 1..4096 bytes"
    );
    let regex = regex::RegexBuilder::new(pattern)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()?;
    anyhow::ensure!(!regex.is_match(""), "flag regex must not match empty input");
    Ok(regex)
}
#[derive(Serialize, ToSchema)]
pub struct SubmitterBatch {
    pub id: String,
    pub revision: i64,
    pub flag_count: u32,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub input_flags: Option<Vec<String>>,
    pub stdout: String,
    pub stderr: String,
}
#[derive(Serialize, ToSchema)]
pub struct SubmitterBatchSummary {
    pub id: String,
    pub revision: i64,
    pub flag_count: u32,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
}
#[derive(Serialize, ToSchema)]
pub struct SubmitterBatchPage {
    pub items: Vec<SubmitterBatchSummary>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}
#[derive(Serialize, ToSchema)]
pub struct SubmitterView {
    pub manual_request: Option<super::manual::RunRequest>,
    pub config: SubmitterConfig,
    pub queued_flags: u64,
    pub next_batch_at: i64,
    pub interval_seconds: u32,
    pub last_batch: Option<SubmitterBatch>,
    pub runtime_available: bool,
}

pub(super) fn load(db: &rusqlite::Connection) -> anyhow::Result<Option<SubmitterConfig>> {
    let row=db.query_row("SELECT revision,enabled,flag_pattern,python_packages,source,timeout_seconds,batch_size FROM flag_submitter_config WHERE id=1",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,bool>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,u32>(5)? as u64,r.get::<_,u32>(6)?))).optional()?;
    row.map(
        |(revision, enabled, flag_pattern, packages, source, timeout_seconds, batch_size)| {
            Ok(SubmitterConfig {
                batch_size,
                revision,
                enabled,
                flag_pattern,
                python_packages: serde_json::from_str(&packages)?,
                source,
                timeout_seconds,
            })
        },
    )
    .transpose()
}
impl Flags {
    pub async fn configure_submitter(
        &self,
        actor: Actor,
        config: SubmitterConfig,
    ) -> anyhow::Result<()> {
        let _guard = self.configuration.lock().await;
        anyhow::ensure!((0..i64::MAX).contains(&config.revision), "invalid revision");
        let regex = config.validate()?;
        anyhow::ensure!(
            !config.enabled || self.settings.script_runtime.is_some(),
            "host Python runtime/uv is not configured by the operator"
        );
        let mut cached = config.clone();
        cached.revision += 1;
        self.auth.db.run(move|db|{
            let tx=db.transaction()?;
            let old=tx.query_row("SELECT revision FROM flag_submitter_config WHERE id=1",[],|r|r.get::<_,i64>(0)).optional()?;
            anyhow::ensure!(old.unwrap_or(0)==config.revision,"submitter revision conflict; reload configuration");
            tx.execute("INSERT INTO flag_submitter_config(id,revision,enabled,flag_pattern,python_packages,source,timeout_seconds,updated_at,author_id,batch_size) VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,enabled=excluded.enabled,flag_pattern=excluded.flag_pattern,python_packages=excluded.python_packages,source=excluded.source,timeout_seconds=excluded.timeout_seconds,updated_at=excluded.updated_at,author_id=excluded.author_id,batch_size=excluded.batch_size",params![config.revision+1,config.enabled,config.flag_pattern,serde_json::to_string(&config.python_packages)?,config.source,config.timeout_seconds as u32,now(),actor.id,config.batch_size])?;
            // New captures become eligible immediately; the worker serializes bounded batches.
            tx.execute("UPDATE flag_submitter_clock SET next_batch_at=?1 WHERE id=1",[now()])?;
            audit::append_change(&tx,&actor,"submitter.configure","python-submitter",&uuid::Uuid::new_v4().to_string(),true,now(),&serde_json::json!({"revision":config.revision+1,"enabled":config.enabled,"source_sha256":format!("{:x}",Sha256::digest(config.source.as_bytes())),"interval_seconds":1}))?;
            tx.commit()?;Ok(())
        }).await?;
        *self.farm_regex.write().unwrap_or_else(|e| e.into_inner()) = Some(regex);
        *self.script.write().unwrap_or_else(|e| e.into_inner()) = Some(cached);
        Ok(())
    }
    pub async fn submitter_view(&self, actor: Actor) -> anyhow::Result<SubmitterView> {
        let config = self
            .script
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| SubmitterConfig {
                batch_size: 240,
                revision: 0,
                enabled: false,
                flag_pattern: self.settings.pattern.clone().unwrap_or_default(),
                python_packages: Vec::new(),
                source: String::new(),
                timeout_seconds: 20,
            });
        let (queued_flags, next_batch_at, last_batch) = self
            .auth
            .db
            .run(move |db| {
                let tx = db.transaction()?;
                let queued = tx.query_row(
                    "SELECT count(*) FROM submissions WHERE state IN ('queued','retry','inflight')",
                    [],
                    |r| r.get::<_, u32>(0),
                )?;
                let next = tx.query_row(
                    "SELECT next_batch_at FROM flag_submitter_clock WHERE id=1",
                    [],
                    |r| r.get::<_, i64>(0),
                )?;
                let batch = tx
                    .prepare(&format!("{BATCH_SELECT} ORDER BY rowid DESC LIMIT 1"))?
                    .query_row([], batch_row)
                    .optional()?
                    .map(decode_batch)
                    .transpose()?;
                audit::append(
                    &tx,
                    &actor,
                    "submitter.read",
                    "python-submitter",
                    &uuid::Uuid::new_v4().to_string(),
                    true,
                    now(),
                )?;
                tx.commit()?;
                Ok((queued, next, batch))
            })
            .await?;
        Ok(SubmitterView {
            manual_request: self.latest_manual().await?,
            config,
            queued_flags: queued_flags as u64,
            next_batch_at,
            interval_seconds: 1,
            last_batch,
            runtime_available: self.settings.script_runtime.is_some(),
        })
    }
}
// Keep submitter codecs independent of Farm's output retention policy.
const MAX_INPUT_BYTES: usize = 240 * (256 * 6 + 3) + 2;
const MAX_OUTPUT_BYTES: usize = 65536;
const BATCH_SELECT: &str = "SELECT id,revision,flag_count,status,started_at,finished_at,exit_code,stdout_gzip,stderr_gzip,input_gzip FROM flag_submitter_batches";
type BatchRow = (
    String,
    i64,
    u32,
    String,
    i64,
    Option<i64>,
    Option<i32>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);
fn batch_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BatchRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
    ))
}
fn decode_batch(row: BatchRow) -> anyhow::Result<SubmitterBatch> {
    let (id, revision, flag_count, status, started_at, finished_at, exit_code, out, err, input) =
        row;
    let input_flags = input
        .map(|blob| -> anyhow::Result<Vec<String>> {
            let values: Vec<String> =
                serde_json::from_slice(&decode_gzip(&blob, MAX_INPUT_BYTES)?)?;
            anyhow::ensure!(
                values.len() <= 240 && values.iter().all(|v| v.len() <= 256),
                "invalid submitter input bounds"
            );
            Ok(values)
        })
        .transpose()?;
    Ok(SubmitterBatch {
        id,
        revision,
        flag_count,
        status,
        started_at,
        finished_at,
        exit_code,
        input_flags,
        stdout: decode_output(out)?,
        stderr: decode_output(err)?,
    })
}
fn decode_gzip(blob: &[u8], limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(blob)
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= limit,
        "submitter diagnostic exceeds decoded size limit"
    );
    Ok(bytes)
}
fn decode_output(blob: Option<Vec<u8>>) -> anyhow::Result<String> {
    Ok(match blob {
        Some(blob) => String::from_utf8_lossy(&decode_gzip(&blob, MAX_OUTPUT_BYTES)?).into_owned(),
        None => String::new(),
    })
}
pub(super) fn encode_gzip(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}
impl Flags {
    pub async fn submitter_batches(
        &self,
        actor: Actor,
        limit: u32,
        offset: u32,
    ) -> anyhow::Result<SubmitterBatchPage> {
        anyhow::ensure!((1..=100).contains(&limit), "limit must be 1..100");
        self.auth.db.run(move |db| {
            let tx = db.transaction()?;
            let total = tx.query_row("SELECT count(*) FROM flag_submitter_batches", [], |r| r.get::<_,u32>(0))? as u64;
            let items = tx.prepare("SELECT id,revision,flag_count,status,started_at,finished_at,exit_code FROM flag_submitter_batches ORDER BY rowid DESC LIMIT ?1 OFFSET ?2")?
                .query_map(params![limit,offset], |r| Ok(SubmitterBatchSummary {
                    id:r.get(0)?,revision:r.get(1)?,flag_count:r.get(2)?,status:r.get(3)?,started_at:r.get(4)?,finished_at:r.get(5)?,exit_code:r.get(6)?
                }))?.collect::<Result<Vec<_>,_>>()?;
            audit::append_change(&tx,&actor,"submitter.batches.read","submitter-batches",&uuid::Uuid::new_v4().to_string(),true,now(),&serde_json::json!({"limit":limit,"offset":offset}))?;
            tx.commit()?;
            Ok(SubmitterBatchPage {items,total,limit,offset})
        }).await
    }
    pub async fn submitter_batch(
        &self,
        actor: Actor,
        id: String,
    ) -> anyhow::Result<Option<SubmitterBatch>> {
        anyhow::ensure!(uuid::Uuid::parse_str(&id).is_ok(), "invalid batch id");
        self.auth
            .db
            .run(move |db| {
                let tx = db.transaction()?;
                let batch = tx
                    .prepare(&format!("{BATCH_SELECT} WHERE id=?1"))?
                    .query_row([&id], batch_row)
                    .optional()?
                    .map(decode_batch)
                    .transpose()?;
                audit::append(
                    &tx,
                    &actor,
                    "submitter.batch.read",
                    &id,
                    &uuid::Uuid::new_v4().to_string(),
                    batch.is_some(),
                    now(),
                )?;
                tx.commit()?;
                Ok(batch)
            })
            .await
    }
}
