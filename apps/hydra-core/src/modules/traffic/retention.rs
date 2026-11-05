use rusqlite::{OptionalExtension, Transaction, params};
pub(super) const SERVICE_BYTES: i64 = 256 * 1024 * 1024;

/// Only the over-budget service is rotated. Whole closed sessions go together;
/// active sessions keep their cards and a bounded tail rather than becoming orphans.
pub(super) fn enforce(tx: &Transaction<'_>, limit: i64) -> anyhow::Result<()> {
    let services = {
        let mut q = tx.prepare("SELECT service_id FROM traffic_service_budget WHERE bytes>?1")?;
        q.query_map([limit], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for service in services {
        let mut previous_bytes = i64::MAX;
        while tx.query_row(
            "SELECT bytes FROM traffic_service_budget WHERE service_id=?1",
            [&service],
            |r| r.get::<_, i64>(0),
        )? > limit
        {
            let used: i64 = tx.query_row(
                "SELECT bytes FROM traffic_service_budget WHERE service_id=?1",
                [&service],
                |r| r.get(0),
            )?;
            // Each step must free compressed bytes. This bounds work by stored
            // data, without a retry deadlock from an arbitrary victim count.
            anyhow::ensure!(
                used < previous_bytes,
                "decoded service rotation made no progress"
            );
            previous_bytes = used;
            let orphan:Option<String>=tx.query_row("SELECT e.flow_id FROM traffic_events_zstd e WHERE e.service_id=?1 AND NOT EXISTS(SELECT 1 FROM traffic_flows_zstd f WHERE f.id=e.flow_id) AND NOT EXISTS(SELECT 1 FROM traffic_favorites p WHERE p.flow_id=e.flow_id) ORDER BY e.seq LIMIT 1",[&service],|r|r.get(0)).optional()?;
            let victims = if let Some(id) = orphan {
                vec![id]
            } else {
                let mut q=tx.prepare("SELECT f.id FROM traffic_flows f WHERE f.service_id=?1 AND json_type(f.data,'$.closed')='object' AND NOT EXISTS(SELECT 1 FROM traffic_favorites p WHERE p.flow_id=f.id) ORDER BY f.seq LIMIT 64")?;
                q.query_map([&service], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            if !victims.is_empty() {
                for id in victims {
                    if within_budget(tx, &service, limit)? {
                        break;
                    }
                    tx.execute(
                        "DELETE FROM traffic_events_zstd WHERE service_id=?1 AND flow_id=?2",
                        params![service, id],
                    )?;
                    tx.execute(
                        "DELETE FROM traffic_messages_zstd WHERE service_id=?1 AND flow_id=?2",
                        params![service, id],
                    )?;
                    tx.execute(
                        "DELETE FROM traffic_flows_zstd WHERE service_id=?1 AND id=?2",
                        params![service, id],
                    )?;
                }
                continue;
            }
            let messages = {
                let mut q=tx.prepare("SELECT m.id,m.flow_id,json_extract(hydra_text(m.data),'$.event_id') FROM traffic_messages_zstd m WHERE m.service_id=?1 AND NOT EXISTS(SELECT 1 FROM traffic_favorites p WHERE p.flow_id=m.flow_id) ORDER BY m.seq LIMIT 64")?;
                q.query_map([&service], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
            };
            if messages.is_empty() {
                // Raw/shadow rules can emit hits without any parsed messages.
                let events = {
                    let mut q=tx.prepare("SELECT e.event_id,e.flow_id FROM traffic_events_zstd e WHERE e.service_id=?1 AND e.kind!='flow_opened' AND NOT EXISTS(SELECT 1 FROM traffic_favorites p WHERE p.flow_id=e.flow_id) ORDER BY e.seq LIMIT 64")?;
                    q.query_map([&service], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?
                };
                anyhow::ensure!(
                    !events.is_empty(),
                    "decoded service quota exhausted by protected lifecycle data"
                );
                for (event, flow) in events {
                    if within_budget(tx, &service, limit)? {
                        break;
                    }
                    tx.execute("DELETE FROM traffic_events_zstd WHERE event_id=?1", [event])?;
                    mark_incomplete(tx, &flow)?;
                }
                continue;
            }
            for (id, flow, event) in messages {
                if within_budget(tx, &service, limit)? {
                    break;
                }
                tx.execute("DELETE FROM traffic_messages_zstd WHERE id=?1", [id])?;
                tx.execute("DELETE FROM traffic_events_zstd WHERE event_id=?1", [event])?;
                mark_incomplete(tx, &flow)?;
            }
        }
    }
    Ok(())
}

fn mark_incomplete(tx: &Transaction<'_>, flow: &str) -> rusqlite::Result<()> {
    tx.execute("UPDATE traffic_flows SET data=json_set(data,'$.decoded_incomplete',json('true')) WHERE id=?1 AND COALESCE(json_extract(data,'$.decoded_incomplete'),0)=0",[flow])?;
    Ok(())
}

fn within_budget(tx: &Transaction<'_>, service: &str, limit: i64) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT bytes<=?2 FROM traffic_service_budget WHERE service_id=?1",
        params![service, limit],
        |r| r.get(0),
    )
}
