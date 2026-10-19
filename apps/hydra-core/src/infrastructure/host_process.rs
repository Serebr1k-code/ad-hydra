//! Explicit operator-selected host children. These run with Core's OS privileges.
use super::{
    db::Database,
    operations::{HostRuntimeSettings, append_bounded, batch_captures},
};
use crate::modules::{farm, flags};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct ProcessGroup(i32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = killpg(Pid::from_raw(self.0), Signal::SIGKILL);
    }
}

fn private_directory(path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(path.is_absolute(), "host work directory must be absolute");
    if !path.exists() {
        std::fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.mode() & 0o077 == 0
            && metadata.uid() == nix::unistd::geteuid().as_raw(),
        "host work directory must be private and owned by Core user"
    );
    Ok(())
}

pub(crate) async fn run(
    settings: HostRuntimeSettings,
    job: farm::Job,
    cancel: CancellationToken,
    flags: flags::Flags,
    db: Database,
) -> anyhow::Result<farm::RunOutput> {
    anyhow::ensure!(!job.targets.is_empty(), "job has no targets");
    anyhow::ensure!(settings.uv.is_absolute(), "uv path must be absolute");
    private_directory(&settings.work_dir)?;
    let cache = settings.work_dir.join("uv-cache");
    private_directory(&cache)?;
    let home = settings.work_dir.join("home");
    private_directory(&home)?;
    let directory = settings
        .work_dir
        .join(format!("run-{}", uuid::Uuid::new_v4()));
    private_directory(&directory)?;
    let _directory = Directory(directory.clone());
    let bytes = tokio::fs::read(&job.artifact).await?;
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= 2 * 1024 * 1024,
        "invalid immutable artifact"
    );
    let entrypoint = directory.join(match job.manifest.runtime.as_str() {
        "python" => "exploit.py",
        "shell" => "exploit.sh",
        "elf" => "exploit",
        _ => anyhow::bail!("unsupported host runtime"),
    });
    std::fs::write(&entrypoint, bytes)?;
    std::fs::set_permissions(&entrypoint, std::fs::Permissions::from_mode(0o700))?;
    let targets_file = directory.join("targets.json");
    if job.manifest.mode == "batch" {
        std::fs::write(&targets_file, serde_json::to_vec(&job.targets)?)?;
        std::fs::set_permissions(&targets_file, std::fs::Permissions::from_mode(0o600))?;
    }
    let legacy_targets = job.targets.iter().map(|target| {
        let address = std::net::SocketAddr::new(target.host.parse()?, target.port);
        anyhow::ensure!(target.port != 0, "target port required");
        Ok(serde_json::json!({"id":target.id,"address":address.to_string(),"transport":target.transport}))
    }).collect::<anyhow::Result<Vec<_>>>()?;
    let legacy_targets = serde_json::to_string(&legacy_targets)?;
    let mut output = farm::RunOutput {
        status: "succeeded".into(),
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: Some(0),
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(job.manifest.timeout_seconds);
    let targets = if job.manifest.mode == "batch" {
        &job.targets[..1]
    } else {
        &job.targets[..]
    };
    for target in targets {
        // Scheduler has resolved the allowlist snapshot; forbid hostname/DNS interpretation.
        let _: std::net::IpAddr = target.host.parse()?;
        anyhow::ensure!(target.port != 0, "target port required");
        if cancel.is_cancelled() {
            output.status = "cancelled".into();
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            output.status = "timeout".into();
            break;
        }
        let label = format!(
            "\n[target id={} name={:?} host={} port={}]\n",
            target.id, target.name, target.host, target.port
        );
        farm::runtimes::chunks::append(&db, &job.run_id, "stdout", label.as_bytes()).await?;
        farm::runtimes::chunks::append(&db, &job.run_id, "stderr", label.as_bytes()).await?;
        append_bounded(&mut output.stdout, label.as_bytes());
        append_bounded(&mut output.stderr, label.as_bytes());
        let mut command = match job.manifest.runtime.as_str() {
            "python" => {
                let mut command = Command::new(&settings.uv);
                command.args(["run", "--no-project"]);
                for package in &job.manifest.python_packages {
                    command.arg("--with").arg(package);
                }
                command.args(["python", "-u"]).arg(&entrypoint);
                command
            }
            "shell" => {
                let mut command = Command::new("/bin/bash");
                command.arg(&entrypoint);
                command
            }
            _ => Command::new(&entrypoint),
        };
        command
            .arg(&target.host)
            .arg(target.port.to_string())
            .current_dir(&directory)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("UV_CACHE_DIR", &cache)
            .env("UV_PYTHON_INSTALL_DIR", home.join("python"))
            .env("HYDRA_TARGET_HOST", &target.host)
            .env("HYDRA_TARGET_PORT", target.port.to_string())
            .env("HYDRA_TARGET_ID", &target.id)
            .env("TARGET_HOST", &target.host)
            .env("TARGET_PORT", target.port.to_string())
            .env("TARGETS_JSON", &legacy_targets)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if job.manifest.mode == "batch" {
            command.env("HYDRA_TARGETS_FILE", &targets_file);
        }
        let (result_target, scope) = if job.manifest.mode == "batch" {
            ("", "batch")
        } else {
            (target.id.as_str(), "per_target")
        };
        farm::process_results::started(&db, &job.run_id, result_target, scope).await?;
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                farm::process_results::finished(&db, &job.run_id, result_target, "failed", None)
                    .await?;
                return Err(error.into());
            }
        };
        let group = ProcessGroup(
            child
                .id()
                .ok_or_else(|| anyhow::anyhow!("child pid missing"))? as i32,
        );
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut stdout_log = DiskLog::default();
        let mut stderr_log = DiskLog::default();
        let mut out = Scanner::default();
        let mut err = Scanner::default();
        let mut out_open = true;
        let mut err_open = true;
        let mut out_chunk = [0u8; 8192];
        let mut err_chunk = [0u8; 8192];
        let mut interval = tokio::time::interval(Duration::from_millis(300));
        let mut interruption = None;
        let status = loop {
            tokio::select! {
                status = child.wait() => break status?,
                _ = cancel.cancelled() => {
                    interruption = Some("cancelled");
                    output.status = "cancelled".into();
                    let _ = killpg(Pid::from_raw(group.0), Signal::SIGKILL);
                    break child.wait().await?;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    interruption = Some("timeout");
                    output.status = "timeout".into();
                    let _ = killpg(Pid::from_raw(group.0), Signal::SIGKILL);
                    break child.wait().await?;
                }
                value = stdout.read(&mut out_chunk), if out_open => {
                    let n = value?; out_open = n > 0;
                    stdout_log.feed(&db, &job.run_id, "stdout", &out_chunk[..n], false).await?;
                    out.feed(&flags, &job, &target.id, &out_chunk[..n], false).await?;
                    append_bounded(&mut output.stdout, &out_chunk[..n]);
                }
                value = stderr.read(&mut err_chunk), if err_open => {
                    let n = value?; err_open = n > 0;
                    stderr_log.feed(&db, &job.run_id, "stderr", &err_chunk[..n], false).await?;
                    err.feed(&flags, &job, &target.id, &err_chunk[..n], false).await?;
                    append_bounded(&mut output.stderr, &err_chunk[..n]);
                }
                _ = interval.tick() => {
                    stdout_log.feed(&db,&job.run_id,"stdout",&[],true).await?;
                    stderr_log.feed(&db,&job.run_id,"stderr",&[],true).await?;
                    farm::runtimes::persist_logs(&db, &job.run_id, &output.stdout, &output.stderr).await?;
                },
            }
        };
        let target_status = interruption.unwrap_or(if status.success() {
            "succeeded"
        } else {
            "failed"
        });
        farm::process_results::finished(
            &db,
            &job.run_id,
            result_target,
            target_status,
            status.code(),
        )
        .await?;
        // A script may leave background children holding its pipes after it exits.
        drop(group);
        let drain = async {
            while out_open || err_open {
                tokio::select! {
                    value = stdout.read(&mut out_chunk), if out_open => {
                        let n = value?; out_open = n > 0;
                        stdout_log.feed(&db, &job.run_id, "stdout", &out_chunk[..n], false).await?;
                    out.feed(&flags, &job, &target.id, &out_chunk[..n], false).await?;
                        append_bounded(&mut output.stdout, &out_chunk[..n]);
                    }
                    value = stderr.read(&mut err_chunk), if err_open => {
                        let n = value?; err_open = n > 0;
                        stderr_log.feed(&db, &job.run_id, "stderr", &err_chunk[..n], false).await?;
                    err.feed(&flags, &job, &target.id, &err_chunk[..n], false).await?;
                        append_bounded(&mut output.stderr, &err_chunk[..n]);
                    }
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        match tokio::time::timeout(Duration::from_millis(500), drain).await {
            Ok(result) => result?,
            Err(_) => append_bounded(&mut output.stderr, b"\n[output drain timed out]\n"),
        }
        farm::runtimes::persist_logs(&db, &job.run_id, &output.stdout, &output.stderr).await?;
        stdout_log
            .feed(&db, &job.run_id, "stdout", &[], true)
            .await?;
        stderr_log
            .feed(&db, &job.run_id, "stderr", &[], true)
            .await?;
        output.exit_code = status.code();
        out.feed(&flags, &job, &target.id, &[], true).await?;
        err.feed(&flags, &job, &target.id, &[], true).await?;
        if ["timeout", "cancelled"].contains(&output.status.as_str()) {
            break;
        }
        if !status.success() {
            output.status = "failed".into();
        }
    }
    farm::runtimes::persist_logs(&db, &job.run_id, &output.stdout, &output.stderr).await?;
    Ok(output)
}

#[derive(Default)]
struct DiskLog {
    pending: Vec<u8>,
}
impl DiskLog {
    async fn feed(
        &mut self,
        db: &Database,
        id: &str,
        stream: &'static str,
        bytes: &[u8],
        flush: bool,
    ) -> anyhow::Result<()> {
        self.pending.extend_from_slice(bytes);
        while self.pending.len() >= 32768 {
            farm::runtimes::chunks::append(db, id, stream, &self.pending[..32768]).await?;
            self.pending.drain(..32768);
        }
        if flush && !self.pending.is_empty() {
            farm::runtimes::chunks::append(db, id, stream, &self.pending).await?;
            self.pending.clear();
        }
        Ok(())
    }
}

#[derive(Default)]
struct Scanner {
    pending: Vec<u8>,
    discard_line: bool,
    search_from: usize,
}
impl Scanner {
    async fn feed(
        &mut self,
        flags: &flags::Flags,
        job: &farm::Job,
        target: &str,
        bytes: &[u8],
        eof: bool,
    ) -> anyhow::Result<()> {
        self.pending.extend_from_slice(bytes);
        let captures = if job.manifest.mode == "batch" {
            let mut collected = Vec::new();
            while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                if !self.discard_line {
                    collected.extend(batch_captures(flags, &self.pending[..end], &job.targets)?);
                }
                self.pending.drain(..=end);
                self.discard_line = false;
            }
            if eof && !self.discard_line {
                collected.extend(batch_captures(flags, &self.pending, &job.targets)?);
                self.pending.clear();
            }
            if self.pending.len() > 65536 {
                self.pending.clear();
                self.discard_line = true;
            }
            collected
        } else {
            let cutoff = if eof {
                self.pending.len()
            } else {
                self.pending.len().saturating_sub(256)
            };
            let mut consumed = cutoff;
            let mut collected = Vec::new();
            let matches = if self.search_from == 0 {
                flags.extract_spans(&self.pending)?
            } else {
                flags.extract_spans_from(&self.pending, self.search_from)?
            };
            for (start, end, value) in matches {
                if end <= cutoff {
                    collected.push((target.to_owned(), value));
                } else if start < cutoff {
                    consumed = consumed.min(start);
                }
            }
            let discard = consumed.saturating_sub(4);
            self.pending.drain(..discard);
            self.search_from = consumed - discard;
            collected
        };
        for (target, value) in captures {
            flags
                .record_farm_captured(value, target, job.run_id.clone(), job.initiated_by.clone())
                .await?;
        }
        Ok(())
    }
}

// Flags are <=256 bytes; overlap keeps matches crossing the scanner's 64 KiB
// input boundary while respecting that API's bound.
#[cfg(test)]
fn extract_output(flags: &flags::Flags, bytes: &[u8]) -> anyhow::Result<Vec<String>> {
    let mut captures = std::collections::BTreeSet::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let end = (offset + 65536).min(bytes.len());
        captures.extend(flags.extract(&bytes[offset..end])?);
        if end == bytes.len() {
            break;
        }
        offset = end - 256;
    }
    Ok(captures.into_iter().take(256).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infrastructure::crypto::Crypto, modules::auth::AuthState};

    async fn fixture(
        script: &[u8],
    ) -> (
        Directory,
        HostRuntimeSettings,
        farm::Job,
        flags::Flags,
        Database,
    ) {
        let directory =
            std::env::temp_dir().join(format!("hydra-host-test-{}", uuid::Uuid::new_v4()));
        private_directory(&directory).unwrap();
        let db = Database::open(directory.join("db")).await.unwrap();
        db.migrate(
            16,
            include_str!("../../../../migrations/0016_farm_catalog_logs.sql"),
        )
        .await
        .unwrap();
        db.migrate(
            20,
            include_str!("../../../../migrations/0020_farm_stream_logs.sql"),
        )
        .await
        .unwrap();
        db.migrate(
            23,
            include_str!("../../../../migrations/0023_farm_process_results.sql"),
        )
        .await
        .unwrap();
        // Minimal row, using the same durable stream columns as the scheduler.
        db.run(|connection| {
            connection.execute("INSERT INTO farm_targets VALUES('team','Team one','127.0.0.1',9000,'tcp',1,1,0,0,'actor')", [])?;
            connection.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at) VALUES('run','exploit','1','{}','[\"team\"]','actor','running',0)", [])?;
            Ok(())
        }).await.unwrap();
        let flags = flags::Flags::open(
            AuthState::new(
                db.clone(),
                Crypto::load(&directory.join("key")).unwrap(),
                "https://localhost".into(),
            ),
            flags::Settings {
                pattern: Some("FLAG_[A-Z]+".into()),
                checker: None,
                auto_submit_captured: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let artifact = directory.join("artifact");
        std::fs::write(&artifact, script).unwrap();
        let job = farm::Job {
            run_id: "run".into(),
            exploit_id: "exploit".into(),
            version: "1".into(),
            artifact,
            initiated_by: "actor".into(),
            manifest: farm::Manifest {
                runtime: "shell".into(),
                mode: "per_target".into(),
                timeout_seconds: 5,
                memory_mib: 32,
                cpu_percent: 10,
                python_packages: vec![],
            },
            targets: vec![farm::Target {
                id: "team".into(),
                name: "Team one".into(),
                host: "127.0.0.1".into(),
                port: 9000,
                transport: "tcp".into(),
                enabled: true,
                revision: 1,
            }],
        };
        let settings = HostRuntimeSettings {
            uv: "/usr/bin/uv".into(),
            work_dir: directory.join("runtime"),
        };
        (Directory(directory), settings, job, flags, db)
    }

    #[tokio::test]
    async fn per_target_attempts_keep_each_exit_status_and_batch_has_one_outcome() {
        for batch in [false, true] {
            let (_directory, settings, mut job, flags, db) =
                fixture(b"if [ \"$1\" = 127.0.0.1 ]; then exit 7; fi; exit 0\n").await;
            let mut second = job.targets[0].clone();
            second.id = "team-two".into();
            second.host = "127.0.0.2".into();
            job.targets.push(second);
            if batch {
                job.manifest.mode = "batch".into();
            }
            let output = run(settings, job, CancellationToken::new(), flags, db.clone())
                .await
                .unwrap();
            assert_eq!(output.status, "failed");
            let results = db.run(|connection| {
                Ok(connection.prepare("SELECT target_id,scope,status,exit_code,started_at,finished_at FROM farm_process_results WHERE run_id='run' ORDER BY target_id")?
                    .query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,Option<i32>>(3)?,row.get::<_,i64>(4)?,row.get::<_,Option<i64>>(5)?)))?
                    .collect::<Result<Vec<_>,_>>()?)
            }).await.unwrap();
            if batch {
                assert_eq!(results.len(), 1);
                assert_eq!(
                    (&*results[0].0, &*results[0].1, &*results[0].2, results[0].3),
                    ("", "batch", "failed", Some(7))
                );
            } else {
                assert_eq!(results.len(), 2);
                assert_eq!(
                    (&*results[0].0, &*results[0].2, results[0].3),
                    ("team", "failed", Some(7))
                );
                assert_eq!(
                    (&*results[1].0, &*results[1].2, results[1].3),
                    ("team-two", "succeeded", Some(0))
                );
            }
            assert!(
                results
                    .iter()
                    .all(|row| row.5.is_some_and(|finished| finished >= row.4))
            );
        }
    }

    #[tokio::test]
    async fn failed_spawn_records_terminal_target_attempt() {
        let (_directory, mut settings, mut job, flags, db) = fixture(b"unused").await;
        job.manifest.runtime = "python".into();
        settings.uv = settings.work_dir.join("missing-uv-executable");
        assert!(
            run(settings, job, CancellationToken::new(), flags, db.clone())
                .await
                .is_err()
        );
        db.run(|connection| {
            let (status,code,end):(String,Option<i32>,Option<i64>)=connection.query_row("SELECT status,exit_code,finished_at FROM farm_process_results WHERE run_id='run' AND target_id='team'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            assert_eq!(status,"failed");assert!(code.is_none());assert!(end.is_some());
            Ok(())
        }).await.unwrap();
    }

    #[tokio::test]
    async fn streams_beyond_one_megabyte_with_bounded_preview_and_late_flag() {
        let (_directory,settings,job,flags,db)=fixture(b"head -c 2200000 /dev/zero | tr '\\0' x; printf '\\nFLAG_LATE\\n'; printf 'FLAG_ERR\\n' >&2\n").await;
        let output = run(settings, job, CancellationToken::new(), flags, db.clone())
            .await
            .unwrap();
        assert_eq!(output.status, "succeeded");
        assert_eq!(output.stdout.len(), farm::runtimes::chunks::PREVIEW_BYTES);
        let actor = crate::modules::audit::Actor {
            kind: "user".into(),
            id: "actor".into(),
        };
        let mut offset = 0;
        let mut length = 0;
        let mut tail = String::new();
        loop {
            let page = farm::runtimes::chunks::page(
                &db,
                actor.clone(),
                "run".into(),
                "stdout".into(),
                offset,
                65536,
            )
            .await
            .unwrap();
            let next = page["next_offset"].as_u64().unwrap();
            assert!(!page["history_lost"].as_bool().unwrap());
            length += page["text"].as_str().unwrap().len();
            tail.push_str(page["text"].as_str().unwrap());
            if tail.len() > 128 {
                tail = tail[tail.len() - 128..].to_owned();
            }
            if next == offset || next == page["end_offset"].as_u64().unwrap() {
                break;
            }
            offset = next;
        }
        assert!(length > 2200000);
        assert!(tail.ends_with("\nFLAG_LATE\n"));
        let values = db
            .run(|db| {
                Ok(db
                    .prepare("SELECT value FROM flags ORDER BY value")?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .await
            .unwrap();
        assert_eq!(values, vec!["FLAG_ERR", "FLAG_LATE"]);
    }

    #[tokio::test]
    async fn streaming_scanner_defers_greedy_matches_and_handles_dense_output() {
        let (_directory, _settings, job, flags, db) = fixture(b"true").await;
        let mut scanner = Scanner::default();
        scanner
            .feed(&flags, &job, "team", b"FLAG_CAP", false)
            .await
            .unwrap();
        scanner
            .feed(&flags, &job, "team", b"TURED ", false)
            .await
            .unwrap();
        let mut dense = Vec::new();
        for n in 0..300 {
            dense.extend_from_slice(
                format!(
                    "FLAG_X{}{} ",
                    (b'A' + (n / 26) as u8) as char,
                    (b'A' + (n % 26) as u8) as char
                )
                .as_bytes(),
            );
        }
        scanner
            .feed(&flags, &job, "team", &dense, false)
            .await
            .unwrap();
        scanner.feed(&flags, &job, "team", &[], true).await.unwrap();
        let (count, partial): (i64, bool) = db
            .run(|db| {
                Ok((
                    db.query_row("SELECT COUNT(*) FROM flags", [], |r| r.get(0))?,
                    db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM flags WHERE value='FLAG_CAP')",
                        [],
                        |r| r.get(0),
                    )?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(count, 301);
        assert!(!partial);
    }

    #[tokio::test]
    async fn streaming_scanner_preserves_anchor_and_unicode_word_context() {
        for pattern in ["^FLAG_[A-Z]+", r"\bFLAG_[A-Z]+"] {
            let (directory, _settings, job, _flags, db) = fixture(b"true").await;
            let flags = flags::Flags::open(
                AuthState::new(
                    db.clone(),
                    Crypto::load(&directory.0.join("key")).unwrap(),
                    "https://localhost".into(),
                ),
                flags::Settings {
                    pattern: Some(pattern.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let mut scanner = Scanner::default();
            let mut input = vec![b'x'; 7936];
            input.extend_from_slice(b"FLAG_GOOD");
            input.extend_from_slice(&vec![b' '; 247]);
            scanner
                .feed(&flags, &job, "team", &input, false)
                .await
                .unwrap();
            scanner
                .feed(&flags, &job, "team", b"        ", false)
                .await
                .unwrap();
            scanner.feed(&flags, &job, "team", &[], true).await.unwrap();
            let count: i64 = db
                .run(|db| Ok(db.query_row("SELECT COUNT(*) FROM flags", [], |r| r.get(0))?))
                .await
                .unwrap();
            assert_eq!(count, 0, "{pattern}");
        }
    }

    #[tokio::test]
    async fn host_shell_receives_resolved_target_and_gzip_logs() {
        let (_directory, settings, job, flags, db) = fixture(br#"printf '%s:%s:%s:%s:%s' "$1" "$2" "$HYDRA_TARGET_ID" "${HYDRA_IMPOSSIBLE_SECRET-unset}" "$HOME"
printf 'error-output' >&2
"#).await;
        let output = run(settings, job, CancellationToken::new(), flags, db.clone())
            .await
            .unwrap();
        assert_eq!(output.status, "succeeded");
        assert!(String::from_utf8_lossy(&output.stdout).contains("127.0.0.1:9000:team:unset:"));
        assert!(output.stderr.ends_with(b"error-output"));
        let (plain, gzip) = db
            .run(|connection| {
                Ok(connection.query_row(
                    "SELECT stdout,stdout_gzip FROM farm_runs WHERE id='run'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )?)
            })
            .await
            .unwrap();
        assert!(plain.is_empty());
        assert_eq!(farm::runtimes::decode_log(&gzip).unwrap(), output.stdout);
    }

    #[tokio::test]
    async fn host_logs_are_live_and_cancel_preserves_them() {
        let (_directory, settings, job, flags, db) =
            fixture(b"printf 'before-cancel\\n'; sleep 20 & wait\n").await;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(settings, job, cancel.clone(), flags, db.clone()));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let gzip = db
                .run(|connection| {
                    Ok(connection.query_row(
                        "SELECT stdout_gzip FROM farm_runs WHERE id='run'",
                        [],
                        |row| row.get::<_, Option<Vec<u8>>>(0),
                    )?)
                })
                .await
                .unwrap();
            if gzip.is_some_and(|bytes| {
                farm::runtimes::decode_log(&bytes)
                    .unwrap()
                    .ends_with(b"before-cancel\n")
            }) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no live log persisted"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        db.run(|connection| {
            connection.execute("UPDATE farm_runs SET status='cancelled' WHERE id='run'", [])?;
            Ok(())
        })
        .await
        .unwrap();
        cancel.cancel();
        let output = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(output.status, "cancelled");
        db.run(|db| {
            let (state,finished):(String,Option<i64>)=db.query_row("SELECT status,finished_at FROM farm_process_results WHERE run_id='run' AND target_id='team'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
            assert_eq!(state,"cancelled"); assert!(finished.is_some()); Ok(())
        }).await.unwrap();
        assert!(output.stdout.ends_with(b"before-cancel\n"));
        let bytes = db
            .run(|connection| {
                Ok(connection.query_row(
                    "SELECT stdout_gzip FROM farm_runs WHERE id='run'",
                    [],
                    |row| row.get::<_, Vec<u8>>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(farm::runtimes::decode_log(&bytes).unwrap(), output.stdout);
    }

    #[tokio::test]
    async fn host_timeout_reaps_background_pipe_holders() {
        let (_directory, settings, mut job, flags, db) =
            fixture(b"printf 'started'; sleep 20 & wait\n").await;
        job.manifest.timeout_seconds = 1;
        let output = tokio::time::timeout(
            Duration::from_secs(3),
            run(settings, job, CancellationToken::new(), flags, db.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        db.run(|db| {
            let (state,finished):(String,Option<i64>)=db.query_row("SELECT status,finished_at FROM farm_process_results WHERE run_id='run' AND target_id='team'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
            assert_eq!(state,"timeout"); assert!(finished.is_some()); Ok(())
        }).await.unwrap();
        assert_eq!(output.status, "timeout");
        assert!(output.stdout.ends_with(b"started"));
        assert_eq!(output.exit_code, None);
    }

    #[tokio::test]
    async fn host_python_uv_receives_packages_and_unbuffered_script_arguments() {
        let (directory, mut settings, mut job, flags, db) = fixture(b"print('hello')").await;
        let uv = directory.0.join("uv");
        std::fs::write(&uv, b"#!/bin/bash\nprintf '<%s>\\n' \"$@\"\n").unwrap();
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o700)).unwrap();
        settings.uv = uv;
        job.manifest.runtime = "python".into();
        job.manifest.python_packages = vec!["requests".into(), "pwntools==4.14.1".into()];
        let output = run(settings, job, CancellationToken::new(), flags, db)
            .await
            .unwrap();
        assert_eq!(output.status, "succeeded");
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("<run>\n<--no-project>\n<--with>\n<requests>\n<--with>\n<pwntools==4.14.1>\n<python>\n<-u>\n"));
        assert!(text.contains("exploit.py>\n<127.0.0.1>\n<9000>"));
    }

    #[tokio::test]
    async fn host_elf_executes_immutable_binary() {
        let bytes = std::fs::read("/bin/echo").unwrap();
        let (_directory, settings, mut job, flags, db) = fixture(&bytes).await;
        job.manifest.runtime = "elf".into();
        let output = run(settings, job, CancellationToken::new(), flags, db)
            .await
            .unwrap();
        assert_eq!(output.status, "succeeded");
        assert!(output.stdout.ends_with(b"127.0.0.1 9000\n"));
    }

    #[tokio::test]
    async fn dropping_runner_future_kills_process_group() {
        let (_directory, settings, job, flags, db) =
            fixture(b"sleep 20 & printf '%s %s\\n' \"$$\" \"$!\"; wait\n").await;
        let task = tokio::spawn(run(
            settings,
            job,
            CancellationToken::new(),
            flags,
            db.clone(),
        ));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let pids = loop {
            let gzip = db
                .run(|connection| {
                    Ok(connection.query_row(
                        "SELECT stdout_gzip FROM farm_runs WHERE id='run'",
                        [],
                        |row| row.get::<_, Option<Vec<u8>>>(0),
                    )?)
                })
                .await
                .unwrap();
            if let Some(bytes) = gzip {
                let bytes = farm::runtimes::decode_log(&bytes).unwrap();
                if let Some(line) = std::str::from_utf8(&bytes).unwrap().lines().last() {
                    let pids = line
                        .split_whitespace()
                        .filter_map(|value| value.parse::<u32>().ok())
                        .collect::<Vec<_>>();
                    if pids.len() == 2 {
                        break pids;
                    }
                }
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(30)).await;
        };
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let running = pids.iter().any(|pid| {
                std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                    !stat
                        .rsplit_once(") ")
                        .is_some_and(|(_, tail)| tail.starts_with('Z'))
                })
            });
            if !running {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "child survived runner drop"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    #[tokio::test]
    async fn bounded_flag_scan_crosses_chunks_and_records_target_provenance() {
        let (_directory, settings, job, flags, db) = fixture(b"printf 'FLAG_CAPTURED\\n'\n").await;
        let mut bytes = vec![b'x'; 65533];
        bytes.extend_from_slice(b"FLAG_CROSS");
        bytes.extend_from_slice(&vec![b' '; 65536]);
        assert!(
            extract_output(&flags, &bytes)
                .unwrap()
                .contains(&"FLAG_CROSS".to_owned())
        );
        run(settings, job, CancellationToken::new(), flags, db.clone())
            .await
            .unwrap();
        let provenance = db
            .run(|connection| {
                Ok(connection.query_row(
                    "SELECT target_id,run_id,initiated_by FROM flags WHERE value='FLAG_CAPTURED'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?)
            })
            .await
            .unwrap();
        assert_eq!(provenance, ("team".into(), "run".into(), "actor".into()));
    }

    #[test]
    fn host_directory_rejects_relative_and_public_paths() {
        assert!(private_directory(Path::new("relative")).is_err());
        assert!(private_directory(Path::new("/tmp")).is_err());
    }
}
