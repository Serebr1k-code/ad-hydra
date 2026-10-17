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
            if !err.is_empty(){saved_err.push(b'\n');saved_err.extend_from_slice(&err);}
            (saved_out,saved_err)
        }else{(out,err)};
        let out=runtimes::encode_log(bounded_text(&out).as_bytes())?;
        let err=runtimes::encode_log(bounded_text(&err).as_bytes())?;
        tx.execute("UPDATE farm_runs SET status=?2,stdout='',stderr='',stdout_gzip=?3,stderr_gzip=?4,exit_code=?5,finished_at=COALESCE(finished_at,?6) WHERE id=?1",params![id,status,out,err,code,now()])?;
        audit::append_change(&tx,&Actor{kind:"system".into(),id:"farm-scheduler".into()},"farm.run.finish",&id,&id,true,now(),&serde_json::json!({"initiated_by":initiated_by,"status":status}))?;
        tx.commit()?;Ok(())
    }).await
}
