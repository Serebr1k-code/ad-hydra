use super::*;
use crate::modules::audit::{self, Actor};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
pub(super) struct Error(StatusCode, String);
impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Self(StatusCode::BAD_REQUEST, e.to_string())
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error":{"code":"farm_request_failed","message":self.1}})),
        )
            .into_response()
    }
}
pub(super) async fn actor(s: &Farm, h: &HeaderMap) -> Result<Actor, Error> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| Error(StatusCode::UNAUTHORIZED, "authentication required".into()))?;
    s.auth
        .authenticate(token)
        .await
        .map_err(|_| Error(StatusCode::UNAUTHORIZED, "authentication failed".into()))
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    exploit_id: String,
    revision: i64,
    #[serde(default)]
    team_id: Option<String>,
}
#[derive(Deserialize, Default)]
struct Cursor {
    after: Option<String>,
}
async fn listing(
    s: Farm,
    h: HeaderMap,
    c: Cursor,
    kind: &'static str,
) -> Result<Json<Value>, Error> {
    let a = actor(&s, &h).await?;
    let after = c.after.unwrap_or_default();
    if after.len() > 128 {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid cursor".into()));
    }
    Ok(Json(s.auth.db.run(move|db|{let tx=db.transaction()?;let sql=match kind {"targets"=>"SELECT id,json_object('id',id,'name',name,'host',host,'port',port,'transport',transport,'enabled',json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),'revision',revision,'created_at',created_at,'updated_at',updated_at,'author_id',author_id) FROM farm_targets WHERE id>?1 ORDER BY id LIMIT 100","exploits"=>"SELECT id,json_object('service_id',service_id,'team_ids',json(team_ids),'id',id,'name',name,'enabled',json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),'revision',revision,'version',version,'manifest',json(manifest),'target_ids',json(target_ids),'interval_seconds',interval_seconds,'initiated_by',initiated_by,'created_at',created_at,'updated_at',updated_at) FROM farm_exploits WHERE id>?1 ORDER BY id LIMIT 100",_=>"SELECT CAST(rowid AS TEXT),json_object('service_id',service_id,'team_ids',json(team_ids),'target_ids',json(target_ids),'id',id,'exploit_id',exploit_id,'version',version,'initiated_by',initiated_by,'origin_run_id',origin_run_id,'status',status,'created_at',created_at,'started_at',started_at,'finished_at',finished_at,'exit_code',exit_code) FROM farm_runs WHERE (?1='' OR rowid<CAST(?1 AS INTEGER)) ORDER BY rowid DESC LIMIT 100"};let rows=tx.prepare(sql)?.query_map([after],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;audit::append(&tx,&a,"farm.list",kind,"farm",true,crate::modules::auth::now())?;tx.commit()?;let cursor=rows.last().map(|r|r.0.clone());let items=rows.into_iter().map(|(_,s)|serde_json::from_str::<Value>(&s)).collect::<Result<Vec<_>,_>>()?;Ok(json!({"items":items,"next_cursor":cursor}))}).await?))
}
#[utoipa::path(get,path="/api/v1/farm/targets",responses((status=200,body=Object)),tag="farm")]
async fn targets(
    State(s): State<Farm>,
    h: HeaderMap,
    Query(c): Query<Cursor>,
) -> Result<Json<Value>, Error> {
    listing(s, h, c, "targets").await
}
#[utoipa::path(get,path="/api/v1/farm/exploits",responses((status=200,body=Object)),tag="farm")]
async fn exploits(
    State(s): State<Farm>,
    h: HeaderMap,
    Query(c): Query<Cursor>,
) -> Result<Json<Value>, Error> {
    listing(s, h, c, "exploits").await
}
#[utoipa::path(get,path="/api/v1/farm/runs",responses((status=200,body=Object)),tag="farm")]
async fn runs(
    State(s): State<Farm>,
    h: HeaderMap,
    Query(c): Query<Cursor>,
) -> Result<Json<Value>, Error> {
    listing(s, h, c, "runs").await
}
#[utoipa::path(put,path="/api/v1/farm/targets",request_body=Target,responses((status=200),(status=400)),tag="farm")]
async fn target(
    State(s): State<Farm>,
    h: HeaderMap,
    Json(t): Json<Target>,
) -> Result<Json<Value>, Error> {
    s.target(actor(&s, &h).await?, t).await?;
    Ok(Json(json!({"saved":true})))
}
#[utoipa::path(put,path="/api/v1/farm/exploits",request_body=Exploit,responses((status=200),(status=400)),tag="farm")]
async fn exploit(
    State(s): State<Farm>,
    h: HeaderMap,
    Json(e): Json<Exploit>,
) -> Result<Json<Value>, Error> {
    s.exploit(actor(&s, &h).await?, e).await?;
    Ok(Json(json!({"saved":true})))
}
#[utoipa::path(put,path="/api/v1/farm/exploits/{id}/versions/{version}",operation_id="farm_upload",params(("id"=String,Path),("version"=String,Path)),request_body(content=String,content_type="application/octet-stream"),responses((status=200,body=Object)),tag="farm")]
async fn upload(
    State(s): State<Farm>,
    h: HeaderMap,
    Path((id, v)): Path<(String, String)>,
    b: Bytes,
) -> Result<Json<Value>, Error> {
    let hash = s.upload(actor(&s, &h).await?, id, v, b.to_vec()).await?;
    Ok(Json(json!({"sha256":hash})))
}
#[utoipa::path(post,path="/api/v1/farm/runs",request_body=RunRequest,params(("Idempotency-Key"=String,Header)),responses((status=202,body=Object)),tag="farm")]
async fn run(
    State(s): State<Farm>,
    h: HeaderMap,
    Json(r): Json<RunRequest>,
) -> Result<(StatusCode, Json<Value>), Error> {
    let key = h
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .into();
    let id = s
        .enqueue_for_team(
            actor(&s, &h).await?,
            r.exploit_id,
            r.revision,
            key,
            r.team_id,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"id":id}))))
}
#[utoipa::path(post,path="/api/v1/farm/runs/{id}/cancel",params(("id"=String,Path)),responses((status=200)),tag="farm")]
async fn cancel(
    State(s): State<Farm>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    s.cancel(actor(&s, &h).await?, id).await?;
    Ok(Json(json!({"cancelled":true})))
}
#[utoipa::path(post,path="/api/v1/farm/runs/{id}/retry",params(("id"=String,Path),("Idempotency-Key"=String,Header)),responses((status=202,body=Object)),tag="farm")]
async fn retry(
    State(s): State<Farm>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), Error> {
    let key = h
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let id = s.retry(actor(&s, &h).await?, id, key).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"id":id}))))
}
#[utoipa::path(get,path="/api/v1/farm/runs/{id}",params(("id"=String,Path)),responses((status=200,body=Object)),tag="farm")]
async fn details(
    State(s): State<Farm>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let a = actor(&s, &h).await?;
    Ok(Json(s.auth.db.run(move|db|{
        let tx=db.transaction()?;
        let (mut result,out,err):(String,Option<Vec<u8>>,Option<Vec<u8>>)=tx.query_row("SELECT json_object('id',id,'exploit_id',exploit_id,'version',version,'origin_run_id',origin_run_id,'status',status,'stdout',stdout,'stderr',stderr,'exit_code',exit_code,'initiated_by',initiated_by,'created_at',created_at,'started_at',started_at,'finished_at',finished_at,'service_id',service_id,'team_ids',json(team_ids),'target_ids',json(target_ids)),stdout_gzip,stderr_gzip FROM farm_runs WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let mut value:Value=serde_json::from_str(&result)?;
        for (key,bytes) in [("stdout",out),("stderr",err)] { if let Some(bytes)=bytes {value[key]=String::from_utf8_lossy(&super::runtimes::decode_log(&bytes)?).into_owned().into();}}
        value["stdout_truncated"]=value["stdout"].as_str().is_some_and(|s|s.contains("output truncated]")).into();
        value["stderr_truncated"]=value["stderr"].as_str().is_some_and(|s|s.contains("output truncated]")).into();
        result.clear();
        audit::append(&tx,&a,"farm.run.read",&id,"farm",true,crate::modules::auth::now())?;tx.commit()?;Ok(value)
    }).await?))
}
#[derive(Deserialize)]
struct Revision {
    revision: i64,
}
#[utoipa::path(delete,path="/api/v1/farm/targets/{id}",params(("id"=String,Path),("revision"=i64,Query)),responses((status=200)),tag="farm")]
async fn delete_target(
    State(s): State<Farm>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(r): Query<Revision>,
) -> Result<Json<Value>, Error> {
    s.delete(actor(&s, &h).await?, "targets".into(), id, r.revision)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
#[utoipa::path(delete,path="/api/v1/farm/exploits/{id}",params(("id"=String,Path),("revision"=i64,Query)),responses((status=200)),tag="farm")]
async fn delete_exploit(
    State(s): State<Farm>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(r): Query<Revision>,
) -> Result<Json<Value>, Error> {
    s.delete(actor(&s, &h).await?, "exploits".into(), id, r.revision)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
#[utoipa::path(get,path="/api/v1/farm/exploits/{id}/versions",params(("id"=String,Path)),responses((status=200,body=Object)),tag="farm")]
async fn versions(
    State(s): State<Farm>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let a = actor(&s, &h).await?;
    Ok(Json(s.auth.db.run(move|db|{let tx=db.transaction()?;let rows=tx.prepare("SELECT version,sha256,created_at,author_id FROM farm_versions WHERE exploit_id=?1 ORDER BY created_at,version LIMIT 256")?.query_map([&id],|r|Ok(json!({"version":r.get::<_,String>(0)?,"sha256":r.get::<_,String>(1)?,"created_at":r.get::<_,i64>(2)?,"author_id":r.get::<_,String>(3)?})))?.collect::<Result<Vec<_>,_>>()?;audit::append(&tx,&a,"farm.versions.list",&id,"farm",true,crate::modules::auth::now())?;tx.commit()?;Ok(json!({"items":rows}))}).await?))
}
#[derive(Deserialize)]
struct Retention {
    older_than_days: Option<u16>,
}
#[utoipa::path(delete,path="/api/v1/farm/runs",params(("older_than_days"=Option<u16>,Query)),responses((status=200,body=Object)),tag="farm")]
async fn rotate(
    State(s): State<Farm>,
    h: HeaderMap,
    Query(r): Query<Retention>,
) -> Result<Json<Value>, Error> {
    let deleted = s
        .prune_runs(actor(&s, &h).await?, r.older_than_days.unwrap_or(7))
        .await?;
    Ok(Json(json!({"deleted":deleted})))
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        targets,
        exploits,
        runs,
        target,
        exploit,
        upload,
        run,
        cancel,
        retry,
        details,
        delete_target,
        delete_exploit,
        versions,
        rotate,
        super::catalog_http::services,
        super::catalog_http::service,
        super::catalog_http::teams,
        super::catalog_http::team,
        super::catalog_http::delete_service,
        super::catalog_http::delete_team
    ),
    components(schemas(Target, Exploit, Manifest, RunRequest, FarmService, FarmTeam))
)]
pub struct FarmApi;
pub fn router(s: Farm) -> Router {
    Router::new()
        .route("/api/v1/farm/exploits/{id}/versions", get(versions))
        .route(
            "/api/v1/farm/targets/{id}",
            axum::routing::delete(delete_target),
        )
        .route(
            "/api/v1/farm/exploits/{id}",
            axum::routing::delete(delete_exploit),
        )
        .route("/api/v1/farm/targets", get(targets).put(target))
        .route("/api/v1/farm/exploits", get(exploits).put(exploit))
        .route(
            "/api/v1/farm/exploits/{id}/versions/{version}",
            put(upload).layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .route("/api/v1/farm/runs", get(runs).post(run).delete(rotate))
        .route("/api/v1/farm/runs/{id}", get(details))
        .route("/api/v1/farm/runs/{id}/cancel", post(cancel))
        .route("/api/v1/farm/runs/{id}/retry", post(retry))
        .with_state(s.clone())
        .merge(super::catalog_http::router(s))
}
