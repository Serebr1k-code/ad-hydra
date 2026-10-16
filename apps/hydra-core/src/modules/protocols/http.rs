use super::Protocols;
use crate::modules::audit::Actor;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use serde::Deserialize;
use utoipa::ToSchema;
struct Error(StatusCode, String);
impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Self(StatusCode::BAD_REQUEST, e.to_string())
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(serde_json::json!({"error":{"code":"protocol_request_failed","message":self.1}})),
        )
            .into_response()
    }
}
async fn actor(s: &Protocols, h: &HeaderMap) -> Result<Actor, Error> {
    let token = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| Error(StatusCode::UNAUTHORIZED, "Authentication required".into()))?;
    s.auth
        .authenticate(token)
        .await
        .map_err(|_| Error(StatusCode::UNAUTHORIZED, "Authentication failed".into()))
}
#[utoipa::path(get,path="/api/v1/protocols",responses((status=200,body=Object),(status=401)),tag="protocols")]
async fn list(State(s): State<Protocols>, h: HeaderMap) -> Result<Json<serde_json::Value>, Error> {
    actor(&s, &h).await?;
    s.list().await.map(Json).map_err(Error::from)
}
#[utoipa::path(put,path="/api/v1/protocols/{id}/{version}",params(("id"=String,Path),("version"=String,Path),("X-SHA256"=String,Header)),request_body(content=String,content_type="application/wasm"),responses((status=202,body=Object),(status=400)),tag="protocols")]
async fn upload(
    State(s): State<Protocols>,
    h: HeaderMap,
    Path((id, version)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, Error> {
    let a = actor(&s, &h).await?;
    let hash = h
        .get("x-sha256")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| Error(StatusCode::BAD_REQUEST, "X-SHA256 required".into()))?
        .to_owned();
    let permit = s.admission.clone().try_acquire_owned().map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Protocol operation busy".into(),
        )
    })?;
    let result = tokio::spawn(async move {
        let _permit = permit;
        s.upload(a, id, version, hash, body.to_vec()).await
    })
    .await
    .map_err(|_| {
        Error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Protocol operation failed".into(),
        )
    })??;
    Ok((StatusCode::ACCEPTED, Json(result)).into_response())
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    service_id: String,
}
#[utoipa::path(post,path="/api/v1/protocols/{id}/{version}/activate",params(("id"=String,Path),("version"=String,Path),("Idempotency-Key"=String,Header)),request_body=Activation,responses((status=202,body=Object),(status=400)),tag="protocols")]
async fn activate(
    State(s): State<Protocols>,
    h: HeaderMap,
    Path((id, version)): Path<(String, String)>,
    Json(input): Json<Activation>,
) -> Result<Response, Error> {
    let a = actor(&s, &h).await?;
    let key = h
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let permit = s.admission.clone().try_acquire_owned().map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Protocol operation busy".into(),
        )
    })?;
    let result = tokio::spawn(async move {
        let _permit = permit;
        s.activate(a, id, version, input.service_id, key).await
    })
    .await
    .map_err(|_| {
        Error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Protocol operation failed".into(),
        )
    })??;
    Ok((StatusCode::ACCEPTED, Json(result)).into_response())
}
#[derive(Deserialize, serde::Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FramingInput {
    Raw,
    Delimiter {
        delimiter: Vec<u8>,
    },
    FixedSize {
        size: usize,
    },
    LengthPrefix {
        offset: usize,
        width: usize,
        little_endian: bool,
        includes_header: bool,
    },
}
#[derive(Deserialize, serde::Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FramingProfileInput {
    framing: FramingInput,
    max_frame: usize,
}
#[utoipa::path(put,path="/api/v1/protocol-profiles/{id}/{version}",params(("id"=String,Path),("version"=String,Path)),request_body=FramingProfileInput,responses((status=202,body=Object),(status=400)),tag="protocols")]
async fn profile(
    State(s): State<Protocols>,
    h: HeaderMap,
    Path((id, version)): Path<(String, String)>,
    Json(input): Json<FramingProfileInput>,
) -> Result<Response, Error> {
    let a = actor(&s, &h).await?;
    let input = serde_json::from_value(
        serde_json::to_value(input).map_err(|e| Error(StatusCode::BAD_REQUEST, e.to_string()))?,
    )
    .map_err(|e| Error(StatusCode::BAD_REQUEST, e.to_string()))?;
    let _permit = s.admission.clone().try_acquire_owned().map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Protocol operation busy".into(),
        )
    })?;
    let result = tokio::spawn(async move {
        let _permit = _permit;
        s.profile(a, id, version, input).await
    })
    .await
    .map_err(|_| {
        Error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Profile operation failed".into(),
        )
    })??;
    Ok((StatusCode::ACCEPTED, Json(result)).into_response())
}
#[utoipa::path(delete,path="/api/v1/protocols/activations/{service_id}",operation_id="deactivate_protocol",params(("service_id"=String,Path),("Idempotency-Key"=String,Header)),responses((status=202,body=Object),(status=400),(status=401)),tag="protocols")]
async fn deactivate(
    State(s): State<Protocols>,
    h: HeaderMap,
    Path(service): Path<String>,
) -> Result<Response, Error> {
    let a = actor(&s, &h).await?;
    let key = h
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let permit = s.admission.clone().try_acquire_owned().map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Protocol operation busy".into(),
        )
    })?;
    let request_id = crate::modules::audit::request_id();
    let result = tokio::spawn(crate::modules::audit::scope_request(
        request_id,
        async move {
            let _permit = permit;
            s.deactivate(a, service, key).await
        },
    ))
    .await
    .map_err(|_| {
        Error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Protocol operation failed".into(),
        )
    })??;
    Ok((StatusCode::ACCEPTED, Json(result)).into_response())
}
// Preserve uploads whose existing protocol ID is the newly reserved static segment.
async fn upload_activation_id(
    state: State<Protocols>,
    h: HeaderMap,
    Path(version): Path<String>,
    body: Bytes,
) -> Result<Response, Error> {
    upload(state, h, Path(("activations".to_owned(), version)), body).await
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list, upload, activate, profile, deactivate),
    components(schemas(Activation, FramingInput, FramingProfileInput))
)]
pub struct ProtocolApi;
pub fn router(s: Protocols) -> Router {
    Router::new()
        .route("/api/v1/protocols", get(list))
        .route(
            "/api/v1/protocols/activations/{service_id}",
            delete(deactivate)
                .put(upload_activation_id)
                .layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route(
            "/api/v1/protocol-profiles/{id}/{version}",
            put(profile).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/v1/protocols/{id}/{version}",
            put(upload).layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route("/api/v1/protocols/{id}/{version}/activate", post(activate))
        .with_state(s)
}
