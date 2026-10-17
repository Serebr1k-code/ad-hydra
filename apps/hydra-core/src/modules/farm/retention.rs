use super::Farm;
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::params;
impl Farm {
    pub async fn prune_runs(&self, actor: Actor, days: u16) -> anyhow::Result<usize> {
        anyhow::ensure!(days <= 365, "retention must be 0..365 days");
        let active = self
            .active
            .lock()
            .await
            .as_ref()
            .map(|(id, _)| id.clone())
            .unwrap_or_default();
        self.auth.db.run(move |db| {
            let tx=db.transaction()?;
            let cutoff=now()-i64::from(days)*86400;
            let count=tx.execute("DELETE FROM farm_runs WHERE status NOT IN ('queued','running') AND id<>?1 AND (COALESCE(finished_at,created_at)<=?2 OR id IN (SELECT id FROM farm_runs WHERE status NOT IN ('queued','running') AND id<>?1 ORDER BY COALESCE(finished_at,created_at) DESC,rowid DESC LIMIT -1 OFFSET 1000) OR id IN (SELECT id FROM (SELECT id,SUM(COALESCE(length(stdout_gzip),length(stdout))+COALESCE(length(stderr_gzip),length(stderr))) OVER (ORDER BY COALESCE(finished_at,created_at) DESC,rowid DESC) AS retained_bytes FROM farm_runs WHERE status NOT IN ('queued','running') AND id<>?1) WHERE retained_bytes>67108864))",params![active,cutoff])?;
            if count>0 || actor.kind!="system" {audit::append_change(&tx,&actor,"farm.logs.rotate","farm-runs","farm",true,now(),&serde_json::json!({"deleted_runs":count,"older_than_days":days,"max_completed":1000,"compression":"gzip","compressed_bytes_quota":67108864}))?;}
            tx.commit()?;Ok(count)
        }).await
    }
}
