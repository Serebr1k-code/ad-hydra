use super::{
    Control,
    http::{Error, actor, request},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RetryInput {
    expected_revision: u64,
}

#[derive(Serialize, ToSchema)]
pub(super) struct RetryAccepted {
    revision: u64,
    status: String,
}

#[utoipa::path(post, path = "/api/v1/services/{id}/retry", params(("id" = String, Path)), request_body = RetryInput,
    responses((status = 202, body = RetryAccepted, description = "Retry requested for the unchanged desired service snapshot"), (status = 400), (status = 401), (status = 404), (status = 409)), tag = "services")]
async fn retry_service(
    State(control): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RetryInput>,
) -> Result<(StatusCode, Json<RetryAccepted>), Error> {
    let actor = actor(&control, &headers).await?;
    let revision = control
        .retry_service(actor, input.expected_revision, id, request())
        .await
        .map_err(|cause| {
            if cause
                .downcast_ref::<super::repository::ServiceNotFound>()
                .is_some()
            {
                Error::Missing
            } else {
                Error::from(cause)
            }
        })?;
    Ok((
        StatusCode::ACCEPTED,
        Json(RetryAccepted {
            revision,
            status: "retry_requested".into(),
        }),
    ))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(retry_service), components(schemas(RetryInput, RetryAccepted)))]
pub(super) struct RetryApi;

pub(super) fn router(control: Control) -> Router {
    Router::new()
        .route("/api/v1/services/{id}/retry", post(retry_service))
        .with_state(control)
}
