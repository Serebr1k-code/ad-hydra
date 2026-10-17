use super::Flags;
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportFlags {
    pub text: String,
    pub service_id: Option<String>,
    pub target_id: Option<String>,
}
#[derive(Serialize, ToSchema)]
pub struct ImportResult {
    pub matched: u32,
    pub added: u32,
    pub queued: u32,
    pub duplicates: u32,
}
impl Flags {
    pub async fn import_text(
        &self,
        input: ImportFlags,
        actor: Actor,
        request: String,
    ) -> anyhow::Result<ImportResult> {
        anyhow::ensure!(input.text.len() <= 65536, "paste at most 64 KiB");
        let regex = self
            .farm_regex
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| anyhow::anyhow!("configure a flag regex first"))?;
        let matches = regex
            .find_iter(&input.text)
            .take(257)
            .map(|m| m.as_str().to_owned())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            matches.len() <= 256,
            "paste contains more than 256 matches; split it into smaller imports"
        );
        for value in &matches {
            self.validate(value)?;
        }
        let matched = matches.len() as u32;
        let mut seen = std::collections::HashSet::new();
        let values = matches
            .into_iter()
            .filter(|v| seen.insert(v.clone()))
            .collect::<Vec<_>>();
        self.auth.db.run(move|db| {
            let tx=db.transaction()?;
            for value in [&input.service_id,&input.target_id].into_iter().flatten() {anyhow::ensure!(!value.is_empty()&&value.len()<=128,"invalid source ID");}
            if let Some(service)=&input.service_id {
                let farm=if tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='farm_services')",[],|r|r.get::<_,bool>(0))? {tx.query_row("SELECT EXISTS(SELECT 1 FROM farm_services WHERE id=?1 AND enabled=1)",[service],|r|r.get::<_,bool>(0))?}else{false};
                let proxy=tx.query_row("SELECT EXISTS(SELECT 1 FROM control_state c,json_each(c.services) s WHERE json_extract(s.value,'$.id')=?1 AND json_extract(s.value,'$.enabled')=1)",[service],|r|r.get::<_,bool>(0))?;
                anyhow::ensure!(farm||proxy,"service is not enabled");
            }
            if let Some(target)=&input.target_id {anyhow::ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM farm_targets WHERE id=?1 AND enabled=1)",[target],|r|r.get::<_,bool>(0))?,"target is not enabled");}
            let mut added=0;let mut queued=0;
            for value in values {
                let digest=format!("{:x}",Sha256::digest(value.as_bytes()));
                let old=tx.query_row("SELECT id FROM flags WHERE digest=?1 AND category='captured'",[&digest],|r|r.get::<_,String>(0)).optional()?;
                let id=old.clone().unwrap_or_else(||uuid::Uuid::new_v4().to_string());
                if old.is_none() {
                    anyhow::ensure!(tx.query_row("SELECT count(*) FROM flags",[],|r|r.get::<_,u32>(0))?<100000,"flag vault full");
                    tx.execute("INSERT INTO flags(id,digest,category,value,target_id,initiated_by,created_at,service_id) VALUES(?1,?2,'captured',?3,?4,?5,?6,?7)",params![id,digest,value,input.target_id,actor.id,now(),input.service_id])?;added+=1;
                }
                let exists=tx.query_row("SELECT EXISTS(SELECT 1 FROM submissions WHERE flag_id=?1)",[&id],|r|r.get::<_,bool>(0))?;
                super::repository::enqueue(&tx,&id,&actor,&request)?;
                if !exists {queued+=1;}
            }
            audit::append_change(&tx,&actor,"flags.import","flags",&request,true,now(),&serde_json::json!({"matched":matched,"added":added,"queued":queued,"service_id":input.service_id,"target_id":input.target_id}))?;
            tx.commit()?;Ok(ImportResult{matched,added,queued,duplicates:matched-added})
        }).await
    }
}
