//! Per-run and per-target counts of distinct flags observed by child processes.
use rusqlite::{Transaction, params};
use serde_json::{Value, json};

pub(super) fn attach(tx: &Transaction<'_>, value: &mut Value, targets: bool) -> anyhow::Result<()> {
    let id = value["id"].as_str().unwrap_or_default();
    let (new, existing, complete): (u32,u32,bool) = tx.query_row(
        "SELECT COALESCE(SUM(is_new),0),COUNT(*)-COALESCE(SUM(is_new),0),NOT EXISTS(SELECT 1 FROM farm_flag_stats_legacy_runs WHERE run_id=?1) FROM farm_flag_observations WHERE run_id=?1",
        [id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    )?;
    let processes = if targets {
        let mut rows = std::collections::BTreeMap::<String, (u32, u32)>::new();
        for target in value["target_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            rows.insert(target.into(), (0, 0));
        }
        let mut statement=tx.prepare("SELECT target_id,SUM(is_new),COUNT(*)-SUM(is_new) FROM farm_flag_observations WHERE run_id=?1 GROUP BY target_id ORDER BY target_id")?;
        for row in statement.query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })? {
            let (target, new, existing) = row?;
            rows.insert(target, (new, existing));
        }
        Some(rows.into_iter().map(|(target_id,(new_flags,existing_flags))|json!({"target_id":target_id,"new_flags":new_flags,"existing_flags":existing_flags})).collect::<Vec<_>>())
    } else {
        None
    };
    value["new_flags"] = new.into();
    value["existing_flags"] = existing.into();
    value["flag_counts_complete"] = complete.into();
    if let Some(processes) = processes {
        value["flag_counts"] = json!(processes);
    }
    Ok(())
}
