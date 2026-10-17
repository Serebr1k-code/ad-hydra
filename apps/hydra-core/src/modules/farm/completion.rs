use super::{Farm, RunOutput, runtimes, scheduler::bounded_text};
use crate::modules::{
    audit::{self, Actor},
    auth::now,
};
use rusqlite::params;

pub(super) async fn finish(
    farm: &Farm,
    id: String,
    initiated_by: String,
    output: anyhow::Result<RunOutput>,
    preserve_saved: bool,
) -> anyhow::Result<()> {
    farm.auth.db.run(move |db| {
        let tx=db.transaction()?;
        let (previous,plain_out,plain_err,gzip_out,gzip_err):(String,String,String,Option<Vec<u8>>,Option<Vec<u8>>)=tx.query_row("SELECT status,stdout,stderr,stdout_gzip,stderr_gzip FROM farm_runs WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
        let keep=preserve_saved||output.is_err();
        let (status,out,err,code)=match output {
            Ok(value)=>(value.status,value.stdout,value.stderr,value.exit_code),
            Err(error)=>("failed".into(),Vec::new(),error.to_string().chars().take(1024).collect::<String>().into_bytes(),None),
        };
        let status=if previous=="running" {if ["succeeded","failed","timeout","cancelled","interrupted"].contains(&status.as_str()){status}else{"failed".into()}}else{previous};
        let (out,err)=if keep {
            let saved_out=if let Some(bytes)=gzip_out {runtimes::decode_log(&bytes)?}else{plain_out.into_bytes()};
            let mut saved_err=if let Some(bytes)=gzip_err {runtimes::decode_log(&bytes)?}else{plain_err.into_bytes()};
            if !err.is_empty(){
                let stream_bytes:i64=tx.query_row("SELECT stderr_bytes FROM farm_runs WHERE id=?1",[&id],|r|r.get(0))?;
                if stream_bytes==0 {runtimes::chunks::append_tx(&tx,&id,"stderr",&saved_err)?;}
                runtimes::chunks::append_tx(&tx,&id,"stderr",b"\n")?;
                runtimes::chunks::append_tx(&tx,&id,"stderr",&err)?;
                saved_err.push(b'\n');saved_err.extend_from_slice(&err);
            }
            (saved_out,saved_err)
        }else{(out,err)};
        let out=runtimes::encode_log(bounded_text(&out).as_bytes())?;
        let err=runtimes::encode_log(bounded_text(&err).as_bytes())?;
        tx.execute("UPDATE farm_runs SET logs_complete=1,status=?2,stdout='',stderr='',stdout_gzip=?3,stderr_gzip=?4,exit_code=?5,finished_at=COALESCE(finished_at,?6) WHERE id=?1",params![id,status,out,err,code,now()])?;
        runtimes::chunks::enforce_quota(&tx)?;
        audit::append_change(&tx,&Actor{kind:"system".into(),id:"farm-scheduler".into()},"farm.run.finish",&id,&id,true,now(),&serde_json::json!({"initiated_by":initiated_by,"status":status}))?;
        tx.commit()?;Ok(())
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn runner_diagnostics_are_paged_with_live_chunks_and_legacy_stderr() {
        let (farm, path) = super::super::tests::setup().await;
        for live in [false, true] {
            let id = if live { "live" } else { "legacy" }.to_owned();
            let row = id.clone();
            let gzip = runtimes::encode_log(b"before-error").unwrap();
            farm.auth.db.run(move|db|{db.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at,stderr_gzip) VALUES(?1,'e','v','{}','[]','a','running',0,?2)",params![row,gzip])?;Ok(())}).await.unwrap();
            if live {
                runtimes::chunks::append(&farm.auth.db, &id, "stderr", b"before-error")
                    .await
                    .unwrap();
            }
            finish(
                &farm,
                id.clone(),
                "a".into(),
                Err(anyhow::anyhow!("spawn failed")),
                false,
            )
            .await
            .unwrap();
            let page = runtimes::chunks::page(
                &farm.auth.db,
                Actor::user("a".into()),
                id,
                "stderr".into(),
                0,
                65536,
            )
            .await
            .unwrap();
            assert_eq!(page["text"], "before-error\nspawn failed");
            assert_eq!(page["done"], true);
        }
        drop(farm);
        std::fs::remove_dir_all(path).unwrap();
    }
}
