use super::{Farm, Target, identifier};
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

fn enabled() -> bool {
    true
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FarmService {
    pub id: String,
    pub name: String,
    pub port: u16,
    #[serde(default = "tcp")]
    pub transport: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub revision: i64,
}
fn tcp() -> String {
    "tcp".into()
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FarmTeam {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub revision: i64,
}
impl Farm {
    pub async fn save_service(&self, actor: Actor, value: FarmService) -> anyhow::Result<()> {
        identifier(&value.id)?;
        anyhow::ensure!(
            !value.name.trim().is_empty()
                && value.name.len() <= 128
                && value.port > 0
                && ["tcp", "udp"].contains(&value.transport.as_str()),
            "name and valid service port/transport required"
        );
        self.auth.db.run(move |db| {
            let tx=db.transaction()?;
            check_revision(&tx,"farm_services",&value.id,value.revision,128)?;
            tx.execute("INSERT INTO farm_services(id,name,port,transport,enabled,revision,created_at,updated_at,author_id) VALUES(?1,?2,?3,?4,?5,1,?6,?6,?7) ON CONFLICT(id) DO UPDATE SET name=excluded.name,port=excluded.port,transport=excluded.transport,enabled=excluded.enabled,revision=farm_services.revision+1,updated_at=excluded.updated_at,author_id=excluded.author_id",params![value.id,value.name,value.port,value.transport,value.enabled,now(),actor.id])?;
            audit::append(&tx,&actor,"farm.service.write",&value.id,"farm",true,now())?;
            tx.commit()?; Ok(())
        }).await
    }
    pub async fn save_team(&self, actor: Actor, value: FarmTeam) -> anyhow::Result<()> {
        identifier(&value.id)?;
        let ip = value.host.parse::<std::net::IpAddr>()?;
        anyhow::ensure!(
            !value.name.trim().is_empty()
                && value.name.len() <= 128
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && value.host != "255.255.255.255",
            "team requires a name and numeric unicast IP"
        );
        self.auth.db.run(move |db| {
            let tx=db.transaction()?;
            check_revision(&tx,"farm_teams",&value.id,value.revision,1024)?;
            tx.execute("INSERT INTO farm_teams(id,name,host,enabled,revision,created_at,updated_at,author_id) VALUES(?1,?2,?3,?4,1,?5,?5,?6) ON CONFLICT(id) DO UPDATE SET name=excluded.name,host=excluded.host,enabled=excluded.enabled,revision=farm_teams.revision+1,updated_at=excluded.updated_at,author_id=excluded.author_id",params![value.id,value.name,ip.to_string(),value.enabled,now(),actor.id])?;
            audit::append(&tx,&actor,"farm.team.write",&value.id,"farm",true,now())?;
            tx.commit()?; Ok(())
        }).await
    }
    pub async fn catalog_list(
        &self,
        actor: Actor,
        kind: &'static str,
        after: String,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(after.len() <= 128, "invalid cursor");
        self.auth.db.run(move |db| {
            let tx=db.transaction()?;
            let sql=match kind {
                "services"=>"SELECT id,json_object('id',id,'name',name,'port',port,'transport',transport,'enabled',json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),'revision',revision,'created_at',created_at,'updated_at',updated_at,'author_id',author_id) FROM farm_services WHERE id>?1 ORDER BY id LIMIT 101",
                "teams"=>"SELECT id,json_object('id',id,'name',name,'host',host,'enabled',json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),'revision',revision,'created_at',created_at,'updated_at',updated_at,'author_id',author_id) FROM farm_teams WHERE id>?1 ORDER BY id LIMIT 101",
                _=>anyhow::bail!("unknown catalog"),
            };
            let mut rows=tx.prepare(sql)?.query_map([after],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
            let cursor=if rows.len()>100 {rows.truncate(100); rows.last().map(|r|r.0.clone())}else{None};
            let items=rows.into_iter().map(|(_,value)|serde_json::from_str::<Value>(&value)).collect::<Result<Vec<_>,_>>()?;
            audit::append(&tx,&actor,"farm.catalog.read",kind,"farm",true,now())?;
            tx.commit()?; Ok(json!({"items":items,"next_cursor":cursor}))
        }).await
    }
}
fn check_revision(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    id: &str,
    revision: i64,
    quota: i64,
) -> anyhow::Result<()> {
    let old = tx
        .query_row(
            &format!("SELECT revision FROM {table} WHERE id=?1"),
            [id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    anyhow::ensure!(old.unwrap_or(0) == revision, "revision conflict");
    let count = tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
        r.get::<_, i64>(0)
    })?;
    anyhow::ensure!(old.is_some() || count < quota, "catalog quota exhausted");
    Ok(())
}

pub(super) fn resolve(
    tx: &rusqlite::Transaction<'_>,
    service_id: &str,
    teams: &[String],
    only_team: Option<&str>,
) -> anyhow::Result<Vec<Target>> {
    let (service_name, port, transport): (String, u16, String) = tx.query_row(
        "SELECT name,port,transport FROM farm_services WHERE id=?1 AND enabled=1",
        [service_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let ids = if let Some(team) = only_team {
        vec![team.to_owned()]
    } else if teams.is_empty() {
        tx.prepare("SELECT id FROM farm_teams WHERE enabled=1 ORDER BY id LIMIT 257")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        teams.to_vec()
    };
    anyhow::ensure!(
        !ids.is_empty() && ids.len() <= 256,
        "choose 1..256 enabled teams"
    );
    anyhow::ensure!(
        ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len(),
        "duplicate teams"
    );
    ids.into_iter()
        .map(|id| {
            let (name, host, revision): (String, String, i64) = tx.query_row(
                "SELECT name,host,revision FROM farm_teams WHERE id=?1 AND enabled=1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            Ok(Target {
                id,
                name: format!("{name} / {service_name}"),
                host,
                port,
                transport: transport.clone(),
                enabled: true,
                revision,
            })
        })
        .collect()
}
pub(super) fn validate_snapshot(
    tx: &rusqlite::Transaction<'_>,
    service_id: &str,
    targets: &[Target],
) -> anyhow::Result<()> {
    for target in targets {
        let current = resolve(tx, service_id, &[], Some(&target.id))?;
        anyhow::ensure!(
            current[0].host == target.host
                && current[0].port == target.port
                && current[0].transport == target.transport,
            "team/service endpoint changed since run was queued; enqueue a fresh run"
        );
    }
    Ok(())
}
