use super::http::{Error, actor};
use super::{Farm, FarmService, FarmTeam};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Default, Deserialize)]
pub(super) struct Cursor {
    after: Option<String>,
}
#[derive(Deserialize)]
pub(super) struct Revision {
    revision: i64,
}
#[utoipa::path(get,operation_id="farm_services",path="/api/v1/farm/services",responses((status=200,body=Object)),tag="farm")]
pub(super) async fn services(
    State(s): State<Farm>,
    h: HeaderMap,
    Query(c): Query<Cursor>,
) -> Result<Json<Value>, Error> {
    Ok(Json(
        s.catalog_list(
            actor(&s, &h).await?,
            "services",
            c.after.unwrap_or_default(),
        )
        .await?,
    ))
}
#[utoipa::path(put,operation_id="farm_service_write",path="/api/v1/farm/services",request_body=FarmService,responses((status=200),(status=400)),tag="farm")]
pub(super) async fn service(
    State(s): State<Farm>,
    h: HeaderMap,
    Json(value): Json<FarmService>,
) -> Result<Json<Value>, Error> {
    s.save_service(actor(&s, &h).await?, value).await?;
    Ok(Json(json!({"saved":true})))
}
#[utoipa::path(get,operation_id="farm_teams",path="/api/v1/farm/teams",responses((status=200,body=Object)),tag="farm")]
pub(super) async fn teams(
    State(s): State<Farm>,
    h: HeaderMap,
    Query(c): Query<Cursor>,
) -> Result<Json<Value>, Error> {
    Ok(Json(
        s.catalog_list(actor(&s, &h).await?, "teams", c.after.unwrap_or_default())
            .await?,
    ))
}
#[utoipa::path(put,operation_id="farm_team_write",path="/api/v1/farm/teams",request_body=FarmTeam,responses((status=200),(status=400)),tag="farm")]
pub(super) async fn team(
    State(s): State<Farm>,
    h: HeaderMap,
    Json(value): Json<FarmTeam>,
) -> Result<Json<Value>, Error> {
    s.save_team(actor(&s, &h).await?, value).await?;
    Ok(Json(json!({"saved":true})))
}
#[utoipa::path(delete,operation_id="farm_service_delete",path="/api/v1/farm/services/{id}",params(("id"=String,Path),("revision"=i64,Query)),responses((status=200)),tag="farm")]
pub(super) async fn delete_service(
    State(s): State<Farm>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(r): Query<Revision>,
) -> Result<Json<Value>, Error> {
    s.delete(actor(&s, &h).await?, "services".into(), id, r.revision)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
#[utoipa::path(delete,operation_id="farm_team_delete",path="/api/v1/farm/teams/{id}",params(("id"=String,Path),("revision"=i64,Query)),responses((status=200)),tag="farm")]
pub(super) async fn delete_team(
    State(s): State<Farm>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(r): Query<Revision>,
) -> Result<Json<Value>, Error> {
    s.delete(actor(&s, &h).await?, "teams".into(), id, r.revision)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
pub(super) fn router(s: Farm) -> Router {
    Router::new()
        .route("/api/v1/farm/services", get(services).put(service))
        .route("/api/v1/farm/teams", get(teams).put(team))
        .route(
            "/api/v1/farm/services/{id}",
            axum::routing::delete(delete_service),
        )
        .route(
            "/api/v1/farm/teams/{id}",
            axum::routing::delete(delete_team),
        )
        .with_state(s)
}
