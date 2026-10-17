//! Disk-backed bounded gzip chunks. Offsets always describe original stream bytes.
use crate::{
    infrastructure::db::Database,
    modules::{
        audit::{self, Actor},
        auth::now,
    },
};
use rusqlite::params;
use serde_json::{Value, json};
pub(crate) const PREVIEW_BYTES: usize = 65536;
pub(crate) async fn append(
    db: &Database,
    id: &str,
    stream: &'static str,
    bytes: &[u8],
) -> anyhow::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    anyhow::ensure!(bytes.len() <= 32768, "log append exceeds chunk bound");
    let id = id.to_owned();
    let bytes = bytes.to_vec();
    db.run(move |db| {
        let tx = db.transaction()?;
        append_tx(&tx, &id, stream, &bytes)?;
        enforce_quota(&tx)?;
        tx.commit()?;
        Ok(())
    })
    .await
}

/// Append within the caller's transaction, compressing only one bounded chunk at a time.
pub(crate) fn append_tx(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    stream: &str,
    bytes: &[u8],
) -> anyhow::Result<()> {
    anyhow::ensure!(["stdout", "stderr"].contains(&stream), "invalid stream");
    let column = if stream == "stdout" {
        "stdout_bytes"
    } else {
        "stderr_bytes"
    };
    let mut offset: i64 = tx.query_row(
        &format!("SELECT {column} FROM farm_runs WHERE id=?1"),
        [id],
        |r| r.get(0),
    )?;
    for raw in bytes.chunks(32768) {
        let gzip = super::encode_log(raw)?;
        tx.execute("INSERT INTO farm_log_chunks(run_id,stream,offset,raw_bytes,gzip) VALUES(?1,?2,?3,?4,?5)",params![id,stream,offset,raw.len() as i64,gzip])?;
        offset += raw.len() as i64;
    }
    tx.execute(
        &format!("UPDATE farm_runs SET {column}=?2 WHERE id=?1"),
        params![id, offset],
    )?;
    Ok(())
}

pub(crate) async fn page(
    db: &Database,
    actor: Actor,
    id: String,
    stream: String,
    offset: u64,
    limit: usize,
) -> anyhow::Result<Value> {
    anyhow::ensure!(
        ["stdout", "stderr"].contains(&stream.as_str()),
        "invalid stream"
    );
    anyhow::ensure!(
        (1..=65536).contains(&limit) && offset <= i64::MAX as u64,
        "invalid log page"
    );
    db.run(move |db| {
        let tx=db.transaction()?;
        let (status,total,plain,gzip,complete):(String,i64,String,Option<Vec<u8>>,bool)=tx.query_row(if stream=="stdout" {"SELECT status,stdout_bytes,stdout,stdout_gzip,logs_complete FROM farm_runs WHERE id=?1"}else{"SELECT status,stderr_bytes,stderr,stderr_gzip,logs_complete FROM farm_runs WHERE id=?1"},[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
        let (start,end,next,bytes)=if total==0 {
            let bytes=if let Some(gzip)=gzip {super::decode_log(&gzip)?}else{plain.into_bytes()};
            let start=(offset as usize).min(bytes.len());let next=(start+limit).min(bytes.len());
            (0,bytes.len() as u64,next as u64,bytes[start..next].to_vec())
        }else{
            let first:Option<i64>=tx.query_row("SELECT MIN(offset) FROM farm_log_chunks WHERE run_id=?1 AND stream=?2",params![id,stream],|r|r.get(0))?;
            let start=first.unwrap_or(total) as u64;let position=offset.max(start).min(total as u64);
            // Iterate one compressed chunk at a time; page and decompression stay bounded.
            let mut query=tx.prepare("SELECT offset,raw_bytes,gzip FROM farm_log_chunks WHERE run_id=?1 AND stream=?2 AND offset+raw_bytes>?3 ORDER BY offset LIMIT 65536")?;
            let mut rows=query.query(params![id,stream,position as i64])?;
            let mut bytes=Vec::with_capacity(limit);
            while bytes.len()<limit {
                let Some(row)=rows.next()? else {break};
                let chunk_offset:i64=row.get(0)?;let length:i64=row.get(1)?;let gzip:Vec<u8>=row.get(2)?;
                let raw=super::decode_log(&gzip)?;anyhow::ensure!(raw.len()==length as usize,"corrupt log chunk");
                let skip=(position.saturating_sub(chunk_offset as u64) as usize).min(raw.len());
                let n=(limit-bytes.len()).min(raw.len()-skip);bytes.extend_from_slice(&raw[skip..skip+n]);
            }
            (start,total as u64,position+bytes.len() as u64,bytes)
        };
        audit::append(&tx,&actor,"farm.run.logs.read",&id,"farm",true,now())?;tx.commit()?;
        Ok(json!({"stream":stream,"text":String::from_utf8_lossy(&bytes),"start_offset":start,"next_offset":next,"end_offset":end,"done": complete && !["queued","running"].contains(&status.as_str()) && next>=end,"dropped_bytes":start,"history_lost":offset<start}))
    }).await
}

/// Evict oldest stream chunks, then completed legacy/preview history if necessary.
pub(crate) fn enforce_quota(tx: &rusqlite::Transaction<'_>) -> anyhow::Result<()> {
    rotate_to(tx, 67108864, 536870912)
}
fn rotate_to(
    tx: &rusqlite::Transaction<'_>,
    compressed_limit: i64,
    raw_limit: i64,
) -> anyhow::Result<()> {
    loop {
        let over: bool = tx.query_row(
            "SELECT compressed_bytes>?1 OR raw_bytes>?2 FROM farm_log_budget WHERE id=1",
            params![compressed_limit, raw_limit],
            |r| r.get(0),
        )?;
        if !over {
            break;
        }
        let n=tx.execute("DELETE FROM farm_log_chunks WHERE id IN (SELECT id FROM farm_log_chunks WHERE id<>(SELECT MAX(id) FROM farm_log_chunks) ORDER BY id LIMIT 128)",[])?;
        if n == 0 {
            let n=tx.execute("DELETE FROM farm_runs WHERE id IN (SELECT id FROM farm_runs WHERE status NOT IN ('queued','running') AND logs_complete=1 ORDER BY COALESCE(finished_at,created_at),rowid LIMIT 1)",[])?;
            if n == 0 {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rotation_exposes_history_loss_and_run_deletion_cascades_budget() {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!("hydra-chunks-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        let db = Database::open(path.join("db")).await.unwrap();
        db.migrate(
            16,
            include_str!("../../../../../../migrations/0016_farm_catalog_logs.sql"),
        )
        .await
        .unwrap();
        db.migrate(
            20,
            include_str!("../../../../../../migrations/0020_farm_stream_logs.sql"),
        )
        .await
        .unwrap();
        db.run(|db|{db.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at) VALUES('r','e','v','{}','[]','a','running',0)",[])?;Ok(())}).await.unwrap();
        let chunk = vec![b'x'; 32768];
        for _ in 0..130 {
            append(&db, "r", "stdout", &chunk).await.unwrap();
        }
        db.run(|db| {
            let tx = db.transaction()?;
            rotate_to(&tx, 67108864, 1048576)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        let actor = Actor {
            kind: "user".into(),
            id: "a".into(),
        };
        let result = page(&db, actor, "r".into(), "stdout".into(), 0, 65536)
            .await
            .unwrap();
        assert_eq!(result["start_offset"], 128 * 32768);
        assert_eq!(result["end_offset"], 130 * 32768);
        assert_eq!(result["history_lost"], true);
        assert_eq!(result["done"], false);
        db.run(|db| {
            let (raw, compressed): (i64, i64) = db.query_row(
                "SELECT raw_bytes,compressed_bytes FROM farm_log_budget",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            assert_eq!(raw, 65536);
            assert!(compressed > 0);
            db.execute("DELETE FROM farm_runs WHERE id='r'", [])?;
            let budget: (i64, i64) = db.query_row(
                "SELECT raw_bytes,compressed_bytes FROM farm_log_budget",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            assert_eq!(budget, (0, 0));
            Ok(())
        })
        .await
        .unwrap();
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }
}
