//! Bounded matrix projection. Aggregate observations separately from run counts so joins cannot multiply either.
use super::effectiveness_dto::*;
use crate::{
    infrastructure::db::Database,
    modules::{
        audit::{self, Actor},
        auth::now,
    },
};
use rusqlite::{Transaction, params};

const ROW_PAGE: usize = 20;
const TEAM_PAGE: usize = 10;
// Snapshot names and hosts survive catalog deletion. Legacy targets use a separate identity namespace.
const BASE: &str = r#"
WITH eligible AS (
 SELECT * FROM farm_runs WHERE (?1 IS NULL OR COALESCE(service_id,'')=?1) AND (?2 IS NULL OR created_at>=?2)
), target_runs AS (
 SELECT DISTINCT r.id AS run_id,r.exploit_id,r.service_id,r.created_at,r.started_at,r.finished_at,r.status,r.exit_code,r.manifest,
 CASE WHEN r.service_id IS NULL THEN 'target:' ELSE 'team:' END || CASE WHEN j.type='object' THEN json_extract(j.value,'$.id') ELSE j.value END AS team_key,
 CASE WHEN j.type='object' THEN json_extract(j.value,'$.id') ELSE j.value END AS target_id,
 CASE WHEN j.type='object' THEN json_extract(j.value,'$.name') END AS target_name,
 CASE WHEN j.type='object' THEN json_extract(j.value,'$.host') END AS target_host
 FROM eligible r,json_each(COALESCE(r.target_snapshot,r.target_ids)) j
)
"#;

pub(super) async fn read(
    db: &Database,
    actor: Actor,
    query: EffectivenessQuery,
    window: String,
) -> anyhow::Result<EffectivenessMatrix> {
    let generated_at = now();
    let since = match window.as_str() {
        "1h" => Some(generated_at - 3600),
        "24h" => Some(generated_at - 86400),
        "7d" => Some(generated_at - 7 * 86400),
        _ => None,
    };
    db.run(move |db| {
        let tx=db.transaction()?;
        let mut rows=rows(&tx,&query,since)?;
        let next_row_offset=(rows.len()>ROW_PAGE).then_some(query.row_offset+ROW_PAGE as u32);
        rows.truncate(ROW_PAGE);
        let mut teams=teams(&tx,&query,since)?;
        let next_team_offset=(teams.len()>TEAM_PAGE).then_some(query.team_offset+TEAM_PAGE as u32);
        teams.truncate(TEAM_PAGE);
        let cells=cells(&tx,&query,since,&rows,&teams)?;
        audit::append_change(&tx,&actor,"farm.effectiveness.read","farm-effectiveness","farm",true,generated_at,&serde_json::json!({"service_id":query.service_id,"window":window,"row_offset":query.row_offset,"team_offset":query.team_offset}))?;
        tx.commit()?;
        Ok(EffectivenessMatrix{rows,teams,cells,next_row_offset,next_team_offset,window,since,generated_at})
    }).await
}

fn rows(
    tx: &Transaction<'_>,
    query: &EffectivenessQuery,
    since: Option<i64>,
) -> anyhow::Result<Vec<EffectivenessRow>> {
    let sql = format!(
        "{BASE}, axes AS (SELECT id AS exploit_id,service_id FROM farm_exploits WHERE (?1 IS NULL OR COALESCE(service_id,'')=?1) UNION SELECT exploit_id,service_id FROM eligible) SELECT a.exploit_id,COALESCE(e.name,a.exploit_id),a.service_id,COALESCE(s.name,a.service_id,'Legacy targets') FROM axes a LEFT JOIN farm_exploits e ON e.id=a.exploit_id LEFT JOIN farm_services s ON s.id=a.service_id ORDER BY a.exploit_id,COALESCE(a.service_id,'') LIMIT ?3 OFFSET ?4"
    );
    Ok(tx
        .prepare(&sql)?
        .query_map(
            params![
                query.service_id,
                since,
                (ROW_PAGE + 1) as i64,
                query.row_offset
            ],
            |r| {
                Ok(EffectivenessRow {
                    exploit_id: r.get(0)?,
                    name: r.get(1)?,
                    service_id: r.get(2)?,
                    service_name: r.get(3)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?)
}
fn teams(
    tx: &Transaction<'_>,
    query: &EffectivenessQuery,
    since: Option<i64>,
) -> anyhow::Result<Vec<EffectivenessTeam>> {
    let sql = format!(
        r#"{BASE}, axes AS (
      SELECT 'team:'||id AS key,id,name,host,1 AS current,0 AS seen FROM farm_teams WHERE ?1 IS NULL OR ?1<>''
      UNION ALL SELECT 'target:'||id,id,name,host,1,0 FROM farm_targets WHERE ?1 IS NULL OR ?1=''
      UNION ALL SELECT team_key,target_id,target_name,target_host,0,created_at FROM target_runs
    ), ranked AS (SELECT *,row_number() OVER(PARTITION BY key ORDER BY current DESC,seen DESC,name,host) AS position FROM axes)
    SELECT key,id,COALESCE(name,id),host FROM ranked WHERE position=1 ORDER BY key LIMIT ?3 OFFSET ?4"#
    );
    Ok(tx
        .prepare(&sql)?
        .query_map(
            params![
                query.service_id,
                since,
                (TEAM_PAGE + 1) as i64,
                query.team_offset
            ],
            |r| {
                Ok(EffectivenessTeam {
                    key: r.get(0)?,
                    id: r.get(1)?,
                    name: r.get(2)?,
                    host: r.get(3)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?)
}
fn cells(
    tx: &Transaction<'_>,
    query: &EffectivenessQuery,
    since: Option<i64>,
    rows: &[EffectivenessRow],
    teams: &[EffectivenessTeam],
) -> anyhow::Result<Vec<EffectivenessCell>> {
    let sql = format!(
        r#"{BASE}, selected AS (
      SELECT t.*,row_number() OVER(PARTITION BY t.exploit_id,t.service_id,t.team_key ORDER BY t.created_at DESC,t.run_id DESC) AS position
      FROM target_runs t WHERE EXISTS(SELECT 1 FROM json_each(?3) r WHERE json_extract(r.value,'$.exploit_id')=t.exploit_id AND json_extract(r.value,'$.service_id') IS t.service_id)
      AND t.team_key IN (SELECT json_extract(value,'$.key') FROM json_each(?4))
    ), counts AS (
      SELECT exploit_id,service_id,team_key,COUNT(*) AS runs,
      SUM(EXISTS(SELECT 1 FROM farm_flag_stats_legacy_runs l WHERE l.run_id=s.run_id)) AS partial
      FROM selected s GROUP BY exploit_id,service_id,team_key
    ), observations AS (
      SELECT s.exploit_id,s.service_id,s.team_key,SUM(o.is_new) AS new_flags,COUNT(*)-SUM(o.is_new) AS duplicate_flags
      FROM selected s JOIN farm_flag_observations o ON o.run_id=s.run_id AND o.target_id=s.target_id
      GROUP BY s.exploit_id,s.service_id,s.team_key
    ), accepted AS (
      SELECT s.exploit_id,s.service_id,s.team_key,COUNT(DISTINCT f.id) AS accepted_flags,MAX(f.created_at) AS last_capture
      FROM selected s JOIN flags f ON f.run_id=s.run_id AND f.target_id=s.target_id AND f.category='captured'
      JOIN submissions sub ON sub.flag_id=f.id AND sub.state='accepted'
      GROUP BY s.exploit_id,s.service_id,s.team_key
    ) SELECT s.exploit_id,s.service_id,s.team_key,c.runs,COALESCE(o.new_flags,0),COALESCE(o.duplicate_flags,0),COALESCE(a.accepted_flags,0),a.last_capture,c.partial=0,
      s.run_id,s.status,COALESCE(json_extract(s.manifest,'$.mode'),'unknown'),s.created_at,s.started_at,s.finished_at,s.exit_code,
      p.status,p.started_at,p.finished_at,p.exit_code,
      (SELECT COUNT(*) FROM selected x JOIN farm_process_results pr ON pr.run_id=x.run_id AND pr.target_id=x.target_id AND pr.scope='per_target' WHERE x.exploit_id=s.exploit_id AND x.service_id IS s.service_id AND x.team_key=s.team_key)
    FROM selected s JOIN counts c ON c.exploit_id=s.exploit_id AND c.service_id IS s.service_id AND c.team_key=s.team_key
    LEFT JOIN observations o ON o.exploit_id=s.exploit_id AND o.service_id IS s.service_id AND o.team_key=s.team_key
    LEFT JOIN accepted a ON a.exploit_id=s.exploit_id AND a.service_id IS s.service_id AND a.team_key=s.team_key
    LEFT JOIN farm_process_results p ON p.run_id=s.run_id AND p.target_id=s.target_id AND p.scope='per_target'
    WHERE s.position=1"#
    );
    let mut values = tx
        .prepare(&sql)?
        .query_map(
            params![
                query.service_id,
                since,
                serde_json::to_string(rows)?,
                serde_json::to_string(teams)?
            ],
            |r| {
                let process_status: Option<String> = r.get(16)?;
                Ok(EffectivenessCell {
                    exploit_id: r.get(0)?,
                    service_id: r.get(1)?,
                    team_key: r.get(2)?,
                    runs: r.get(3)?,
                    target_attempts: r.get(20)?,
                    new_flags: r.get(4)?,
                    duplicate_flags: r.get(5)?,
                    accepted_flags: r.get(6)?,
                    last_accepted_capture_at: r.get(7)?,
                    flag_counts_complete: r.get(8)?,
                    latest: Some(EffectivenessLatestRun {
                        id: r.get(9)?,
                        status: r.get(10)?,
                        mode: r.get(11)?,
                        created_at: r.get(12)?,
                        started_at: r.get(13)?,
                        finished_at: r.get(14)?,
                        exit_code: r.get(15)?,
                        process_status,
                        process_started_at: r.get(17)?,
                        process_finished_at: r.get(18)?,
                        process_exit_code: r.get(19)?,
                    }),
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    for row in rows {
        for team in teams {
            if !values.iter().any(|v| {
                v.exploit_id == row.exploit_id
                    && v.service_id == row.service_id
                    && v.team_key == team.key
            }) {
                values.push(EffectivenessCell {
                    exploit_id: row.exploit_id.clone(),
                    service_id: row.service_id.clone(),
                    team_key: team.key.clone(),
                    runs: 0,
                    target_attempts: 0,
                    new_flags: 0,
                    duplicate_flags: 0,
                    accepted_flags: 0,
                    last_accepted_capture_at: None,
                    flag_counts_complete: true,
                    latest: None,
                });
            }
        }
    }
    Ok(values)
}
