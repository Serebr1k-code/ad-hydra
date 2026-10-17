use super::{
    Flags,
    repository::{Browse, FlagView},
};
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::params;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct FlagPage {
    pub items: Vec<FlagView>,
    pub total: u32,
    pub offset: u32,
    pub limit: u32,
}
impl Flags {
    pub async fn list_page(
        &self,
        actor: Actor,
        request: String,
        browse: Browse,
    ) -> anyhow::Result<Vec<FlagView>> {
        Ok(self.browse_flags(actor, request, browse).await?.items)
    }
    pub async fn browse_flags(
        &self,
        actor: Actor,
        request: String,
        browse: Browse,
    ) -> anyhow::Result<FlagPage> {
        browse.validate()?;
        self.auth.db.run(move|db| {
            let tx=db.transaction()?;
            let service_sort=if tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='farm_services')",[],|r|r.get::<_,bool>(0))? {"COALESCE((SELECT name FROM farm_services WHERE id=f.service_id),f.service_id,'')"}else{"COALESCE(f.service_id,'')"};
            let sort=match browse.sort.as_deref().unwrap_or("created_at") {"service"=>service_sort,"target"=>"f.target_id","category"=>"f.category","state"=>"s.state","digest"=>"f.digest",_=>"f.created_at"};
            let order=if browse.order.as_deref()==Some("asc") {"ASC"}else{"DESC"};
            let filter="FROM flags f LEFT JOIN submissions s ON s.flag_id=f.id WHERE f.category!='own_exposed' AND (?1='' OR f.id LIKE ?1 OR f.digest LIKE ?1 OR f.category LIKE ?1 OR f.target_id LIKE ?1 OR f.service_id LIKE ?1) AND (?2 IS NULL OR f.service_id=?2) AND (?3 IS NULL OR s.state=?3)";
            let total=tx.query_row(&format!("SELECT count(*) {filter}"),params![browse.pattern(),browse.service_id,browse.state],|r|r.get::<_,u32>(0))?;
            let sql=format!("SELECT f.id,f.digest,f.category,f.target_id,f.run_id,f.initiated_by,strftime('%Y-%m-%dT%H:%M:%SZ',f.created_at,'unixepoch'),(SELECT resolution FROM flag_triage WHERE flag_id=f.id),f.service_id,s.state {filter} ORDER BY {sort} {order},f.id ASC LIMIT ?4 OFFSET ?5");
            let items=tx.prepare(&sql)?.query_map(params![browse.pattern(),browse.service_id,browse.state,browse.limit(),browse.offset],|r|Ok(FlagView{id:r.get(0)?,digest:r.get(1)?,category:r.get(2)?,target_id:r.get(3)?,run_id:r.get(4)?,initiated_by:r.get(5)?,created_at:r.get(6)?,triage:r.get(7)?,service_id:r.get(8)?,state:r.get(9)?}))?.collect::<Result<Vec<_>,_>>()?;
            audit::append(&tx,&actor,"flags.list","flags",&request,true,now())?;
            tx.commit()?;Ok(FlagPage{items,total,offset:browse.offset,limit:browse.limit()})
        }).await
    }
}
