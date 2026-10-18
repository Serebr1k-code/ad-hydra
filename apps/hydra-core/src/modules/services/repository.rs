use crate::{
    infrastructure::db::Database,
    modules::{
        audit::{self, Actor},
        auth::AuthState,
    },
};
use hydra_domain::{Rule, RuleSnapshot, Service, ServiceSnapshot};
use rusqlite::params;
use sha2::{Digest, Sha256};
fn fingerprint(rule: &Rule) -> anyhow::Result<String> {
    let mut rule = rule.clone();
    rule.mode = hydra_domain::Mode::Shadow;
    rule.enabled = true;
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&rule)?)))
}
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Notify;
#[derive(Debug)]
pub struct Conflict;
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "revision conflict")
    }
}
impl std::error::Error for Conflict {}
#[derive(Clone)]
pub struct Control {
    pub db: Database,
    pub auth: AuthState,
    pub changed: Arc<Notify>,
}
#[derive(Clone, Serialize)]
pub struct Desired {
    pub services: ServiceSnapshot,
    pub rules: RuleSnapshot,
    pub applied_revision: u64,
    pub services_applied_revision: u64,
    pub rules_applied_revision: u64,
    pub apply_error: Option<String>,
}
#[derive(Serialize)]
pub struct Record {
    pub kind: String,
    pub id: String,
    pub created_by: String,
    pub updated_by: String,
    pub created_at: String,
    pub updated_at: String,
    pub hit_count: u64,
}
fn load(tx: &rusqlite::Connection) -> anyhow::Result<Desired> {
    let(revision,applied,error,services,rules,services_applied,rules_applied)=tx.query_row("SELECT revision,applied_revision,apply_error,services,rules,services_applied_revision,rules_applied_revision FROM control_state WHERE id=1",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?,r.get::<_,i64>(6)?)))?;
    Ok(Desired {
        services: ServiceSnapshot {
            revision: revision as u64,
            services: serde_json::from_str(&services)?,
        },
        rules: RuleSnapshot {
            revision: revision as u64,
            rules: serde_json::from_str(&rules)?,
        },
        applied_revision: applied as u64,
        services_applied_revision: services_applied as u64,
        rules_applied_revision: rules_applied as u64,
        apply_error: error,
    })
}

impl Control {
    pub async fn seed_snapshot(&self, snapshot: ServiceSnapshot) -> anyhow::Result<()> {
        snapshot.validate().map_err(anyhow::Error::msg)?;
        anyhow::ensure!(snapshot.revision <= i64::MAX as u64, "revision too large");
        self.db.run(move|db|{let tx=db.transaction()?;let desired=load(&tx)?;if desired.services.revision!=0 || !desired.services.services.is_empty() || !desired.rules.rules.is_empty(){return Ok(())}let revision=snapshot.revision.max(1);let services=serde_json::to_string(&snapshot.services)?;anyhow::ensure!(services.len()<=1024*1024,"snapshot too large");tx.execute("UPDATE control_state SET revision=?1,services=?2 WHERE id=1",params![revision as i64,services])?;let actor=Actor::local_cli();let now=crate::modules::auth::now();for service in snapshot.services{tx.execute("INSERT INTO control_attribution(kind,id,created_by,updated_by,created_at,updated_at) VALUES('service',?1,?2,?2,?3,?3)",params![service.id,actor.id,now])?;}audit::append(&tx,&actor,"control.bootstrap","services","cli",true,now)?;tx.commit()?;Ok(())}).await?;
        self.changed.notify_one();
        Ok(())
    }

    pub async fn test_rule(
        &self,
        actor: Actor,
        id: String,
        fields: std::collections::BTreeMap<String, hydra_domain::FieldValue>,
        request: String,
    ) -> anyhow::Result<serde_json::Value> {
        anyhow::ensure!(
            serde_json::to_vec(&fields)?.len() <= 65536,
            "test fields too large"
        );
        self.db.run(move|db|{let tx=db.transaction()?;let desired=load(&tx)?;let mut rule=desired.rules.rules.iter().find(|r|r.id==id).cloned().ok_or_else(||anyhow::anyhow!("filter not found"))?;let hash=fingerprint(&rule)?;rule.enabled=true;rule.mode=hydra_domain::Mode::Shadow;let service=desired.services.services.iter().find(|s|s.id==rule.service_id).ok_or_else(||anyhow::anyhow!("service not found"))?;let snapshot=hydra_filter_engine::CompiledSnapshot::compile(RuleSnapshot{revision:desired.rules.revision,rules:vec![rule]},&desired.services.services)?;let evaluated=snapshot.evaluate(&hydra_domain::EvaluationContext{service_id:&service.id,transport:service.transport,fields:&fields,now_ms:crate::modules::auth::now() as u64*1000,safe_http:service.protocol_id=="http"});tx.execute("INSERT INTO rule_tests VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET fingerprint=excluded.fingerprint,tested_at=excluded.tested_at",params![id,hash,crate::modules::auth::now()])?;audit::append(&tx,&actor,"filter.test",&id,&request,true,crate::modules::auth::now())?;tx.commit()?;Ok(serde_json::json!({"matched":!evaluated.hits.is_empty(),"mode":"shadow","forwarding_performed":false}))}).await
    }

    pub async fn open(auth: AuthState) -> anyhow::Result<Self> {
        Self::new(auth.db.clone(), auth).await
    }
    pub async fn mark_services(&self, revision: u64, error: Option<String>) -> anyhow::Result<()> {
        self.mark_part("services", revision, error).await
    }
    pub async fn mark_rules(&self, revision: u64, error: Option<String>) -> anyhow::Result<()> {
        self.mark_part("rules", revision, error).await
    }
    async fn mark_part(
        &self,
        kind: &'static str,
        revision: u64,
        error: Option<String>,
    ) -> anyhow::Result<()> {
        self.db.run(move|db|{let tx=db.transaction()?;let success=error.is_none();let column=if kind=="services"{"services_applied_revision"}else{"rules_applied_revision"};if let Some(error)=error{tx.execute("UPDATE control_state SET apply_error=?2 WHERE revision=?1",params![revision as i64,error.chars().take(2048).collect::<String>()])?;}else{tx.execute(&format!("UPDATE control_state SET {column}=MAX({column},?1),apply_error=NULL WHERE revision=?1"),[revision as i64])?;tx.execute("UPDATE control_state SET applied_revision=MIN(services_applied_revision,rules_applied_revision) WHERE id=1",[])?;}audit::append(&tx,&Actor{kind:"service".into(),id:"proxy_sync".into()},"control.apply",kind,"proxy-sync",success,crate::modules::auth::now())?;tx.commit()?;Ok(())}).await
    }
    pub async fn observe_counters(
        &self,
        epoch: String,
        counts: std::collections::HashMap<String, u64>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            epoch.len() <= 128 && counts.len() <= 256,
            "counter snapshot limit"
        );
        self.db.run(move|db|{
            let tx=db.transaction()?;
            let (old_epoch,old):(String,String)=tx.query_row("SELECT proxy_epoch,counts FROM control_counters WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let old:std::collections::HashMap<String,u64>=serde_json::from_str(&old)?;
            if old_epoch==epoch && old==counts {return Ok(())}
            for (id,n) in &counts {
                let previous=if old_epoch==epoch {old.get(id).copied().unwrap_or(0)}else{0};
                let delta=if *n>=previous{n-previous}else{*n};
                tx.execute("UPDATE control_attribution SET hit_count=MIN(9223372036854775807,hit_count+?2) WHERE kind='filter' AND id=?1",params![id,delta.min(i64::MAX as u64) as i64])?;
            }
            tx.execute("UPDATE control_counters SET proxy_epoch=?1,counts=?2 WHERE id=1",params![epoch,serde_json::to_string(&counts)?])?;
            tx.commit()?;Ok(())
        }).await
    }
    pub async fn observe_hit(&self, id: String) -> anyhow::Result<()> {
        self.db.run(move|db|{db.execute("UPDATE control_attribution SET hit_count=MIN(hit_count+1,9223372036854775807) WHERE kind='filter' AND id=?1",[id])?;Ok(())}).await
    }

    pub async fn new(db: Database, auth: AuthState) -> anyhow::Result<Self> {
        db.migrate(
            5,
            include_str!("../../../../../migrations/0005_rule_counters.sql"),
        )
        .await?;
        db.migrate(
            2,
            include_str!("../../../../../migrations/0002_services_filters.sql"),
        )
        .await?;
        db.migrate(
            22,
            include_str!("../../../../../migrations/0022_service_colors.sql"),
        )
        .await?;
        Ok(Self {
            db,
            auth,
            changed: Arc::new(Notify::new()),
        })
    }
    pub async fn desired(&self) -> anyhow::Result<Desired> {
        self.db.run(|db| load(db)).await
    }
    pub async fn mark_applied(&self, revision: u64, error: Option<String>) -> anyhow::Result<()> {
        self.db.run(move|db|{let error=error.map(|s|s.chars().take(2048).collect::<String>());let tx=db.transaction()?;if error.is_none(){tx.execute("UPDATE control_state SET applied_revision=MAX(applied_revision,?1),apply_error=NULL WHERE revision=?1",[revision as i64])?;}else{tx.execute("UPDATE control_state SET apply_error=?2 WHERE revision=?1",params![revision as i64,error])?;}audit::append(&tx,&Actor{kind:"service".into(),id:"proxy_sync".into()},"control.apply","snapshot","proxy-sync",error.is_none(),crate::modules::auth::now())?;tx.commit()?;Ok(())}).await
    }
    pub async fn attribution(&self) -> anyhow::Result<Vec<Record>> {
        self.db.run(|db|{Ok(db.prepare("SELECT kind,id,created_by,updated_by,strftime('%Y-%m-%dT%H:%M:%SZ',created_at,'unixepoch'),strftime('%Y-%m-%dT%H:%M:%SZ',updated_at,'unixepoch'),hit_count FROM control_attribution ORDER BY kind,id LIMIT 384")?.query_map([],|r|Ok(Record{kind:r.get(0)?,id:r.get(1)?,created_by:r.get(2)?,updated_by:r.get(3)?,created_at:r.get(4)?,updated_at:r.get(5)?,hit_count:r.get::<_,i64>(6)? as u64}))?.collect::<Result<Vec<_>,_>>()?)}).await
    }
    pub async fn put_service(
        &self,
        actor: Actor,
        expected: u64,
        service: Service,
        create: bool,
        request: String,
    ) -> anyhow::Result<Desired> {
        self.mutate(
            actor,
            expected,
            "service",
            service.id.clone(),
            create,
            false,
            request,
            move |d| {
                if let Some(old) = d.services.services.iter_mut().find(|s| s.id == service.id) {
                    anyhow::ensure!(!create, "service already exists");
                    *old = service;
                } else {
                    anyhow::ensure!(create, "service not found");
                    d.services.services.push(service);
                }
                Ok(())
            },
        )
        .await
    }
    pub async fn put_rule(
        &self,
        actor: Actor,
        expected: u64,
        rule: Rule,
        create: bool,
        request: String,
    ) -> anyhow::Result<Desired> {
        self.mutate(
            actor,
            expected,
            "filter",
            rule.id.clone(),
            create,
            false,
            request,
            move |d| {
                if let Some(old) = d.rules.rules.iter_mut().find(|r| r.id == rule.id) {
                    anyhow::ensure!(!create, "filter already exists");
                    *old = rule;
                } else {
                    anyhow::ensure!(create, "filter not found");
                    d.rules.rules.push(rule);
                }
                Ok(())
            },
        )
        .await
    }
    pub async fn bulk_rule_state(
        &self,
        actor: Actor,
        expected: u64,
        ids: Vec<String>,
        enabled: bool,
        request: String,
    ) -> anyhow::Result<Desired> {
        anyhow::ensure!(!ids.is_empty() && ids.len() <= 256, "select 1..256 filters");
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        anyhow::ensure!(unique.len() == ids.len(), "duplicate filter id");
        let result=self.db.run(move|db|{
            let tx=db.transaction()?;let mut desired=load(&tx)?;
            if desired.rules.revision!=expected{return Err(Conflict.into())}
            let before:Vec<_>=ids.iter().map(|id|desired.rules.rules.iter().find(|r|&r.id==id).map(|r|serde_json::json!({"id":id,"enabled":r.enabled})).ok_or_else(||anyhow::anyhow!("filter not found"))).collect::<anyhow::Result<_>>()?;
            for id in &ids {
                let rule=desired.rules.rules.iter_mut().find(|r|&r.id==id).expect("validated");
                rule.enabled=enabled;
            }
            hydra_filter_engine::CompiledSnapshot::compile(desired.rules.clone(),&desired.services.services)?;
            let revision=expected.checked_add(1).filter(|r|*r<=i64::MAX as u64).ok_or_else(||anyhow::anyhow!("revision exhausted"))?;
            desired.services.revision=revision;desired.rules.revision=revision;desired.apply_error=None;
            let rules=serde_json::to_string(&desired.rules.rules)?;
            anyhow::ensure!(rules.len()+serde_json::to_vec(&desired.services.services)?.len()<=1024*1024,"snapshot quota exceeded");
            tx.execute("UPDATE control_state SET revision=?1,rules=?2,apply_error=NULL WHERE id=1",params![revision as i64,rules])?;
            for item in &before {
                let id=item["id"].as_str().expect("validated id");
                tx.execute("UPDATE control_attribution SET updated_by=?2,updated_at=?3 WHERE kind='filter' AND id=?1",params![id,actor.id,crate::modules::auth::now()])?;
                audit::append_change(&tx,&actor,"filter.state",id,&request,true,crate::modules::auth::now(),&serde_json::json!({"before":{"enabled":item["enabled"]},"after":{"enabled":enabled,"revision":revision},"source":"http","bulk":true}))?;
            }
            audit::append_change(&tx,&actor,"filter.bulk_state","filters",&request,true,crate::modules::auth::now(),&serde_json::json!({"selection_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&ids)?)),"count":ids.len(),"before":{"revision":expected},"after":{"enabled":enabled,"revision":revision},"source":"http"}))?;
            tx.commit()?;Ok(desired)
        }).await?;
        self.changed.notify_one();
        Ok(result)
    }
    pub async fn delete(
        &self,
        actor: Actor,
        expected: u64,
        kind: &'static str,
        id: String,
        request: String,
    ) -> anyhow::Result<Desired> {
        let key = id.clone();
        self.mutate(actor, expected, kind, id, false, true, request, move |d| {
            if kind == "service" {
                let n = d.services.services.len();
                d.services.services.retain(|s| s.id != key);
                anyhow::ensure!(n != d.services.services.len(), "service not found");
            } else {
                let n = d.rules.rules.len();
                d.rules.rules.retain(|r| r.id != key);
                anyhow::ensure!(n != d.rules.rules.len(), "filter not found");
            }
            Ok(())
        })
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn mutate<F>(
        &self,
        actor: Actor,
        expected: u64,
        kind: &'static str,
        id: String,
        create: bool,
        delete: bool,
        request: String,
        change: F,
    ) -> anyhow::Result<Desired>
    where
        F: FnOnce(&mut Desired) -> anyhow::Result<()> + Send + 'static,
    {
        let result=self.db.run(move|db|{let tx=db.transaction()?;let mut desired=load(&tx)?;if desired.services.revision!=expected{return Err(Conflict.into())}let before=serde_json::to_value(&desired)?;change(&mut desired)?;desired.services.revision=desired.services.revision.checked_add(1).filter(|v|*v<=i64::MAX as u64).ok_or_else(||anyhow::anyhow!("revision exhausted"))?;desired.rules.revision=desired.services.revision;desired.apply_error=None;desired.services.validate().map_err(anyhow::Error::msg)?;hydra_filter_engine::CompiledSnapshot::compile(desired.rules.clone(),&desired.services.services)?;
 let services=serde_json::to_string(&desired.services.services)?;let rules=serde_json::to_string(&desired.rules.rules)?;anyhow::ensure!(services.len()+rules.len()<=1024*1024,"snapshot quota exceeded");tx.execute("UPDATE control_state SET revision=?1,services=?2,rules=?3,apply_error=NULL WHERE id=1",params![desired.services.revision as i64,services,rules])?;let now=crate::modules::auth::now();if create{tx.execute("INSERT INTO control_attribution(kind,id,created_by,updated_by,created_at,updated_at) VALUES(?1,?2,?3,?3,?4,?4)",params![kind,id,actor.id,now])?;}else if delete{tx.execute("DELETE FROM rule_tests WHERE id=?1",[&id])?;tx.execute("DELETE FROM control_attribution WHERE kind=?1 AND id=?2",params![kind,id])?;}else{tx.execute("UPDATE control_attribution SET updated_by=?3,updated_at=?4 WHERE kind=?1 AND id=?2",params![kind,id,actor.id,now])?;}
 // Snapshot content may contain sensitive filter literals; only revisions and counts are audited.
 let metadata=serde_json::json!({"before":{"revision":before["services"]["revision"]},"after":{"revision":desired.services.revision,"service_count":desired.services.services.len(),"filter_count":desired.rules.rules.len()},"source":"http"});audit::append_change(&tx,&actor,if delete{"control.delete"}else if create{"control.create"}else{"control.update"},&id,&request,true,now,&metadata)?;tx.commit()?;Ok(desired)}).await?;
        self.changed.notify_one();
        Ok(result)
    }
}
