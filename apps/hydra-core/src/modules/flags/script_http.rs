use super::{
    Flags,
    script_config::{SubmitterBatch, SubmitterBatchPage, SubmitterConfig, SubmitterView},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
pub(super) struct Error(StatusCode, String);
impl From<anyhow::Error> for Error {
    fn from(value: anyhow::Error) -> Self {
        Self(StatusCode::BAD_REQUEST, value.to_string())
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(serde_json::json!({"error":{"code":"submitter_request_failed","message":self.1}})),
        )
            .into_response()
    }
}
#[utoipa::path(get,path="/api/v1/submitter",operation_id="submitter_config",responses((status=200,body=SubmitterView),(status=401)),tag="flags")]
pub(super) async fn view(
    State(f): State<Flags>,
    h: HeaderMap,
) -> Result<Json<SubmitterView>, Error> {
    let actor = super::http::actor(&f, &h, false)
        .await
        .map_err(|e| Error(e.0, "authentication required".into()))?;
    Ok(Json(f.submitter_view(actor).await?))
}
#[utoipa::path(put,path="/api/v1/submitter",operation_id="submitter_configure",request_body=SubmitterConfig,responses((status=200,body=SubmitterView),(status=400),(status=401)),tag="flags")]
pub(super) async fn configure(
    State(f): State<Flags>,
    h: HeaderMap,
    Json(config): Json<SubmitterConfig>,
) -> Result<Json<SubmitterView>, Error> {
    let actor = super::http::actor(&f, &h, true)
        .await
        .map_err(|e| Error(e.0, "authentication/origin validation failed".into()))?;
    f.configure_submitter(actor.clone(), config).await?;
    Ok(Json(f.submitter_view(actor).await?))
}
#[utoipa::path(post,path="/api/v1/submitter/run",operation_id="submitter_run",request_body=super::manual::RunInput,responses((status=202,body=super::manual::RunRequest),(status=400),(status=401)),tag="flags")]
pub(super) async fn run(
    State(f): State<Flags>,
    h: HeaderMap,
    Json(input): Json<super::manual::RunInput>,
) -> Result<(StatusCode, Json<super::manual::RunRequest>), Error> {
    let actor = super::http::actor(&f, &h, true)
        .await
        .map_err(|e| Error(e.0, "authentication/origin validation failed".into()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(f.request_manual_run(actor, input.count).await?),
    ))
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
pub(super) struct BatchQuery {
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    offset: u32,
}
fn default_limit() -> u32 {
    20
}
#[utoipa::path(get,path="/api/v1/submitter/batches",operation_id="submitter_batches",params(BatchQuery),responses((status=200,body=SubmitterBatchPage),(status=400),(status=401)),tag="flags")]
pub(super) async fn batches(
    State(f): State<Flags>,
    h: HeaderMap,
    Query(query): Query<BatchQuery>,
) -> Result<Json<SubmitterBatchPage>, Error> {
    let actor = super::http::actor(&f, &h, false)
        .await
        .map_err(|e| Error(e.0, "authentication required".into()))?;
    Ok(Json(
        f.submitter_batches(actor, query.limit, query.offset)
            .await?,
    ))
}
#[utoipa::path(get,path="/api/v1/submitter/batches/{id}",operation_id="submitter_batch",params(("id"=String,Path,description="Batch UUID")),responses((status=200,body=SubmitterBatch),(status=400),(status=401),(status=404)),tag="flags")]
pub(super) async fn batch_detail(
    State(f): State<Flags>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<SubmitterBatch>, Error> {
    let actor = super::http::actor(&f, &h, false)
        .await
        .map_err(|e| Error(e.0, "authentication required".into()))?;
    let batch = f
        .submitter_batch(actor, id)
        .await?
        .ok_or_else(|| Error(StatusCode::NOT_FOUND, "submitter batch not found".into()))?;
    Ok(Json(batch))
}
pub(super) fn router(flags: Flags) -> Router {
    Router::new()
        .route("/api/v1/submitter", get(view).put(configure))
        .route("/api/v1/submitter/run", post(run))
        .route("/api/v1/submitter/batches", get(batches))
        .route("/api/v1/submitter/batches/{id}", get(batch_detail))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024 + 65536))
        .with_state(flags)
}
