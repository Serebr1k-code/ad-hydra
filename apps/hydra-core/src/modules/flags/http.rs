use super::{
    Browse, Flags, Triage,
    repository::{FlagView, SubmissionView},
};
use crate::modules::audit::Actor;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};
pub(super) struct Error(pub(super) StatusCode, Option<String>);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0,Json(serde_json::json!({"error":{"code":"flags_request_failed","message":self.1.unwrap_or_else(||"Flag operation failed".into())}}))).into_response()
    }
}
pub(super) async fn actor(f: &Flags, h: &HeaderMap, mutation: bool) -> Result<Actor, Error> {
    if mutation && h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(&f.auth.origin) {
        return Err(Error(StatusCode::FORBIDDEN, None));
    }
    let token = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(Error(StatusCode::UNAUTHORIZED, None))?;
    f.auth
        .authenticate(token)
        .await
        .map_err(|_| Error(StatusCode::UNAUTHORIZED, None))
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualFlag {
    pub value: String,
    pub target_id: String,
}
#[derive(Serialize, ToSchema)]
pub struct FlagCreated {
    pub id: String,
}
fn request() -> String {
    uuid::Uuid::new_v4().to_string()
}
#[utoipa::path(get,path="/api/v1/flags",operation_id="flags_list",params(Browse),responses((status=200,body=Vec<FlagView>),(status=401)),tag="flags")]
async fn list(
    State(f): State<Flags>,
    Query(browse): Query<Browse>,
    h: HeaderMap,
) -> Result<Json<Vec<FlagView>>, Error> {
    let a = actor(&f, &h, false).await?;
    f.list_page(a, request(), browse)
        .await
        .map(Json)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, None))
}
#[utoipa::path(post,path="/api/v1/flags",request_body=ManualFlag,responses((status=200,body=FlagCreated),(status=400),(status=401)),tag="flags")]
async fn create(
    State(f): State<Flags>,
    h: HeaderMap,
    Json(body): Json<ManualFlag>,
) -> Result<Json<FlagCreated>, Error> {
    let a = actor(&f, &h, true).await?;
    f.record_captured(body.value, body.target_id, None, a, request())
        .await
        .map(|id| Json(FlagCreated { id }))
        .map_err(|_| Error(StatusCode::BAD_REQUEST, None))
}
#[utoipa::path(post,path="/api/v1/flags/{id}/submit",params(("id"=String,Path)),responses((status=204),(status=400),(status=401)),tag="flags")]
async fn submit(
    State(f): State<Flags>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, Error> {
    let a = actor(&f, &h, true).await?;
    f.submit(id, a, request())
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, None))
}
#[utoipa::path(get,path="/api/v1/submissions",params(Browse),responses((status=200,body=Vec<SubmissionView>),(status=401)),tag="flags")]
async fn submissions(
    State(f): State<Flags>,
    Query(browse): Query<Browse>,
    h: HeaderMap,
) -> Result<Json<Vec<SubmissionView>>, Error> {
    let a = actor(&f, &h, false).await?;
    f.submissions_page(a, request(), browse)
        .await
        .map(Json)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, None))
}
#[utoipa::path(post,path="/api/v1/flags/{id}/triage",params(("id"=String,Path)),request_body=Triage,responses((status=204),(status=400),(status=401)),tag="flags")]
async fn triage(
    State(f): State<Flags>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Triage>,
) -> Result<StatusCode, Error> {
    let a = actor(&f, &h, true).await?;
    f.triage(id, body, a, request())
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, None))
}
#[utoipa::path(get,path="/api/v1/flags/page",operation_id="flags_page",params(Browse),responses((status=200,body=super::browse::FlagPage),(status=401)),tag="flags")]
async fn page(
    State(f): State<Flags>,
    Query(browse): Query<Browse>,
    h: HeaderMap,
) -> Result<Json<super::browse::FlagPage>, Error> {
    let a = actor(&f, &h, false).await?;
    f.browse_flags(a, request(), browse)
        .await
        .map(Json)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, None))
}
#[utoipa::path(post,path="/api/v1/flags/import",operation_id="flags_import",request_body=super::imports::ImportFlags,responses((status=200,body=super::imports::ImportResult),(status=400),(status=401)),tag="flags")]
async fn import(
    State(f): State<Flags>,
    h: HeaderMap,
    Json(input): Json<super::imports::ImportFlags>,
) -> Result<Json<super::imports::ImportResult>, Error> {
    let a = actor(&f, &h, true).await?;
    f.import_text(input, a, request())
        .await
        .map(Json)
        .map_err(|error| Error(StatusCode::BAD_REQUEST, Some(error.to_string())))
}
#[derive(OpenApi)]
#[openapi(
    paths(
        list,
        page,
        import,
        create,
        submit,
        submissions,
        triage,
        super::script_http::view,
        super::script_http::configure,
        super::script_http::run
    ),
    components(schemas(
        super::browse::FlagPage,
        super::imports::ImportFlags,
        super::imports::ImportResult,
        super::manual::RunInput,
        super::manual::RunRequest,
        FlagView,
        SubmissionView,
        ManualFlag,
        FlagCreated,
        Triage,
        super::script_config::SubmitterConfig,
        super::script_config::SubmitterView,
        super::script_config::SubmitterBatch
    ))
)]
pub struct FlagsApi;
pub fn router(flags: Flags) -> Router {
    Router::new()
        .route("/api/v1/flags", get(list).post(create))
        .route("/api/v1/flags/page", get(page))
        .route("/api/v1/flags/import", post(import))
        .route("/api/v1/flags/{id}/submit", post(submit))
        .route("/api/v1/flags/{id}/triage", post(triage))
        .route("/api/v1/submissions", get(submissions))
        .layer(DefaultBodyLimit::max(70 * 1024))
        .with_state(flags.clone())
        .merge(super::script_http::router(flags))
}
