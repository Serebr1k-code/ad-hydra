use super::{
    Flags,
    script_config::{SubmitterConfig, SubmitterView},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
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
pub(super) fn router(flags: Flags) -> Router {
    Router::new()
        .route("/api/v1/submitter", get(view).put(configure))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024 + 65536))
        .with_state(flags)
}
