//! Resolve human-readable catalog names while retaining the run's captured target IPs.
use super::Target;
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{Value, json};

pub(super) fn attach(tx: &Transaction<'_>, value: &mut Value) -> anyhow::Result<()> {
    let id = value["id"].as_str().unwrap_or_default();
    let exploit = value["exploit_id"].as_str().unwrap_or_default();
    let exploit_name = tx
        .query_row(
            "SELECT name FROM farm_exploits WHERE id=?1",
            [exploit],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| exploit.into());
    let service = value["service_id"].as_str();
    let service_name = if let Some(service) = service {
        tx.query_row(
            "SELECT name FROM farm_services WHERE id=?1",
            [service],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| service.into())
    } else {
        "Legacy targets".into()
    };
    let snapshot: Option<String> = tx.query_row(
        "SELECT target_snapshot FROM farm_runs WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    let mut teams = Vec::new();
    if let Some(snapshot) = snapshot {
        for target in serde_json::from_str::<Vec<Target>>(&snapshot)? {
            let name = tx
                .query_row(
                    "SELECT name FROM farm_teams WHERE id=?1",
                    [&target.id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .unwrap_or(target.name);
            teams.push(json!({"id":target.id,"name":name,"host":target.host}));
        }
    } else {
        for target in value["target_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let entry = tx
                .query_row(
                    "SELECT name,host FROM farm_targets WHERE id=?1",
                    [target],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?;
            let (name, host) = entry
                .map(|(name, host)| (name, Some(host)))
                .unwrap_or_else(|| (target.into(), None));
            teams.push(json!({"id":target,"name":name,"host":host}));
        }
    }
    value["exploit_name"] = exploit_name.into();
    value["service_name"] = service_name.into();
    value["teams"] = json!(teams);
    Ok(())
}
