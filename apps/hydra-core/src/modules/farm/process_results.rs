//! Persist actual child boundaries, independently from aggregate run outcomes.
use crate::infrastructure::db::Database;
use rusqlite::params;

pub async fn started(
    db: &Database,
    run_id: &str,
    target_id: &str,
    scope: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(scope, "batch" | "per_target"),
        "invalid process scope"
    );
    let (run, target, scope) = (run_id.to_owned(), target_id.to_owned(), scope.to_owned());
    db.run(move|db|{db.execute("INSERT INTO farm_process_results(run_id,target_id,scope,status,started_at) VALUES(?1,?2,?3,'running',?4)",params![run,target,scope,timestamp_millis()])?;Ok(())}).await
}
pub async fn finished(
    db: &Database,
    run_id: &str,
    target_id: &str,
    status: &str,
    exit_code: Option<i32>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(
            status,
            "succeeded" | "failed" | "timeout" | "cancelled" | "interrupted"
        ),
        "invalid process result"
    );
    let (run, target, status) = (run_id.to_owned(), target_id.to_owned(), status.to_owned());
    db.run(move|db|{db.execute("UPDATE farm_process_results SET status=?3,finished_at=?4,exit_code=?5 WHERE run_id=?1 AND target_id=?2 AND status='running'",params![run,target,status,timestamp_millis(),exit_code])?;Ok(())}).await
}

pub(super) fn timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
