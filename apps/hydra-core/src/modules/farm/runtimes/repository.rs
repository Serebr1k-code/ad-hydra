use crate::infrastructure::db::Database;

/// Persist logs independently of status: cancellation commits its terminal status before
/// the child finishes draining its pipes.
pub(crate) async fn persist_logs(
    db: &Database,
    run_id: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> anyhow::Result<()> {
    let id = run_id.to_owned();
    let out = encode_log(stdout)?;
    let err = encode_log(stderr)?;
    db.run(move |connection| {
        let tx=connection.transaction()?;
        tx.execute(
            "UPDATE farm_runs SET stdout='',stderr='',stdout_gzip=?2,stderr_gzip=?3 WHERE id=?1 AND status IN ('running','cancelled')",
            rusqlite::params![id, out, err],
        )?;
        super::chunks::enforce_quota(&tx)?;
        tx.commit()?;
        Ok(())
    }).await
}

/// Zstandard encoding for bounded previews and stream chunks; the shared decoder
/// also accepts legacy gzip blobs retained in older database rows.
pub(crate) fn encode_log(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    hydra_compression::encode(bytes)
}

pub(crate) fn decode_log(blob: &[u8]) -> anyhow::Result<Vec<u8>> {
    hydra_compression::decode(blob, 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zstd_roundtrip_and_legacy_gzip_reads_remain_bounded() {
        use std::io::Write;
        let bytes = vec![b'x'; 32768];
        let compressed = encode_log(&bytes).unwrap();
        assert_eq!(&compressed[..4], b"\x28\xb5\x2f\xfd");
        assert!(compressed.len() < 8192);
        assert_eq!(decode_log(&compressed).unwrap(), bytes);
        let mut legacy = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        legacy.write_all(b"old gzip row").unwrap();
        assert_eq!(
            decode_log(&legacy.finish().unwrap()).unwrap(),
            b"old gzip row"
        );
        let bytes = vec![b'x'; 2 * 1024 * 1024];
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&bytes).unwrap();
        assert!(decode_log(&encoder.finish().unwrap()).is_err());
    }
}

/// Incremental conversion of completed pre-gzip runs; each call touches at most64
/// rows and holds only one row's bounded streams while compressing.
pub(crate) async fn compress_legacy_logs(db: &Database) -> anyhow::Result<usize> {
    db.run(|connection| {
        let tx = connection.transaction()?;
        let ids = tx.prepare(
            "SELECT id FROM farm_runs WHERE status NOT IN ('queued','running') AND ((stdout_gzip IS NULL AND stdout<>'') OR (stderr_gzip IS NULL AND stderr<>'')) ORDER BY created_at,id LIMIT 64",
        )?.query_map([], |row| row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        for id in &ids {
            let (out, err, out_missing, err_missing): (String, String, bool, bool) = tx.query_row(
                "SELECT stdout,stderr,stdout_gzip IS NULL,stderr_gzip IS NULL FROM farm_runs WHERE id=?1",
                [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            if out_missing && !out.is_empty() {
                tx.execute("UPDATE farm_runs SET stdout_gzip=?2,stdout='' WHERE id=?1 AND stdout_gzip IS NULL", rusqlite::params![id, encode_log(out.as_bytes())?])?;
            }
            if err_missing && !err.is_empty() {
                tx.execute("UPDATE farm_runs SET stderr_gzip=?2,stderr='' WHERE id=?1 AND stderr_gzip IS NULL", rusqlite::params![id, encode_log(err.as_bytes())?])?;
            }
        }
        tx.commit()?;
        Ok(ids.len())
    }).await
}

#[cfg(test)]
mod legacy_tests {
    use super::*;
    #[tokio::test]
    async fn legacy_compression_is_incremental_and_preserves_active_rows_and_existing_blobs() {
        use std::os::unix::fs::DirBuilderExt;
        let directory =
            std::env::temp_dir().join(format!("hydra-legacy-log-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let db = Database::open(directory.join("db")).await.unwrap();
        db.migrate(
            16,
            include_str!("../../../../../../migrations/0016_farm_catalog_logs.sql"),
        )
        .await
        .unwrap();
        let existing = encode_log(b"existing").unwrap();
        let expected = existing.clone();
        db.run(move |connection| {
            for n in 0..67 {
                let status = if n == 65 { "running" } else if n == 66 { "queued" } else { "succeeded" };
                connection.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at,stdout,stderr_gzip) VALUES(?1,'e','v','{}','[]','actor',?2,0,'legacy',?3)", rusqlite::params![format!("row-{n:03}"), status, existing])?;
            }
            Ok(())
        }).await.unwrap();
        assert_eq!(compress_legacy_logs(&db).await.unwrap(), 64);
        assert_eq!(compress_legacy_logs(&db).await.unwrap(), 1);
        assert_eq!(compress_legacy_logs(&db).await.unwrap(), 0);
        let (stdout, gzip, existing_blob, active_text) = db
            .run(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT stdout FROM farm_runs WHERE id='row-000'",
                        [],
                        |row| row.get::<_, String>(0),
                    )?,
                    connection.query_row(
                        "SELECT stdout_gzip FROM farm_runs WHERE id='row-000'",
                        [],
                        |row| row.get::<_, Vec<u8>>(0),
                    )?,
                    connection.query_row(
                        "SELECT stderr_gzip FROM farm_runs WHERE id='row-000'",
                        [],
                        |row| row.get::<_, Vec<u8>>(0),
                    )?,
                    connection.query_row(
                        "SELECT stdout FROM farm_runs WHERE id='row-065'",
                        [],
                        |row| row.get::<_, String>(0),
                    )?,
                ))
            })
            .await
            .unwrap();
        assert!(stdout.is_empty());
        assert_eq!(decode_log(&gzip).unwrap(), b"legacy");
        assert_eq!(existing_blob, expected);
        assert_eq!(active_text, "legacy");
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
