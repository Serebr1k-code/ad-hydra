use crate::{
    infrastructure::proxy_client,
    modules::{
        audit, auth,
        services::{
            Control,
            http::{Error, actor, request},
        },
    },
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use hydra_wire::v1::{self, proxy_control_client::ProxyControlClient};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};
use utoipa::ToSchema;

#[derive(Clone)]
pub(super) struct CheckerState {
    pub(super) control: Control,
    pub(super) socket: PathBuf,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestCheckerPolicy {
    id: String,
    services: Vec<String>,
    timeout_ms: u64,
    allow_on_error: bool,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestCheckerSettings {
    revision: u64,
    policies: Vec<RequestCheckerPolicy>,
}

impl From<v1::RequestCheckerSettings> for RequestCheckerSettings {
    fn from(value: v1::RequestCheckerSettings) -> Self {
        Self {
            revision: value.revision,
            policies: value
                .policies
                .into_iter()
                .map(|policy| RequestCheckerPolicy {
                    id: policy.id,
                    services: policy.services,
                    timeout_ms: policy.timeout_ms,
                    allow_on_error: policy.allow_on_error,
                })
                .collect(),
        }
    }
}

pub(super) fn wire_actor(actor: &audit::Actor, request_id: &str) -> v1::Actor {
    v1::Actor {
        r#type: v1::ActorType::User as i32,
        user_id: actor.id.clone(),
        request_id: request_id.into(),
        ..Default::default()
    }
}

pub(super) fn unavailable() -> Error {
    checker_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Request checker settings are unavailable",
    )
}

pub(super) fn checker_error(status: StatusCode, message: &str) -> Error {
    Error::Checker(status, message.into())
}

pub(super) fn map_rpc(error: tonic::Status) -> Error {
    match error.code() {
        tonic::Code::Aborted => checker_error(StatusCode::CONFLICT, error.message()),
        tonic::Code::InvalidArgument => checker_error(StatusCode::BAD_REQUEST, error.message()),
        _ => unavailable(),
    }
}

pub(super) fn rpc_request<T>(body: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(body);
    request.set_timeout(Duration::from_secs(3));
    request
}

async fn log(
    control: &Control,
    actor: audit::Actor,
    action: &'static str,
    request_id: String,
    success: bool,
) -> Result<(), Error> {
    control
        .db
        .run(move |db| {
            let tx = db.transaction()?;
            audit::append(
                &tx,
                &actor,
                action,
                "request-checkers",
                &request_id,
                success,
                auth::now(),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(Error::from)
}

#[utoipa::path(get, path = "/api/v1/filters/request-checkers",
    responses((status = 200, body = RequestCheckerSettings), (status = 401), (status = 503)), tag = "filters")]
pub(super) async fn request_checker_settings(
    State(state): State<CheckerState>,
    headers: HeaderMap,
) -> Result<Json<RequestCheckerSettings>, Error> {
    let actor = actor(&state.control, &headers).await?;
    let id = request();
    let channel = proxy_client::connect(&state.socket)
        .await
        .map_err(|_| unavailable())?;
    let result = ProxyControlClient::new(channel)
        .get_request_checker_settings(rpc_request(v1::GetRequestCheckerSettingsRequest {
            actor: Some(wire_actor(&actor, &id)),
            request_id: id.clone(),
        }))
        .await;
    log(
        &state.control,
        actor,
        "filter.checker_settings.read",
        id,
        result.is_ok(),
    )
    .await?;
    Ok(Json(result.map_err(map_rpc)?.into_inner().into()))
}

#[utoipa::path(put, path = "/api/v1/filters/request-checkers", request_body = RequestCheckerSettings,
    responses((status = 200, body = RequestCheckerSettings), (status = 400), (status = 401), (status = 409), (status = 503)), tag = "filters")]
pub(super) async fn update_request_checker_settings(
    State(state): State<CheckerState>,
    headers: HeaderMap,
    Json(input): Json<RequestCheckerSettings>,
) -> Result<Json<RequestCheckerSettings>, Error> {
    let actor = actor(&state.control, &headers).await?;
    let id = request();
    let channel = proxy_client::connect(&state.socket)
        .await
        .map_err(|_| unavailable())?;
    log(
        &state.control,
        actor.clone(),
        "filter.checker_settings.intent",
        id.clone(),
        true,
    )
    .await?;
    let result = ProxyControlClient::new(channel)
        .set_request_checker_settings(rpc_request(v1::SetRequestCheckerSettingsRequest {
            actor: Some(wire_actor(&actor, &id)),
            request_id: id.clone(),
            settings: Some(v1::RequestCheckerSettings {
                revision: input.revision,
                policies: input
                    .policies
                    .into_iter()
                    .map(|policy| v1::RequestCheckerPolicy {
                        id: policy.id,
                        services: policy.services,
                        timeout_ms: policy.timeout_ms,
                        allow_on_error: policy.allow_on_error,
                    })
                    .collect(),
            }),
        }))
        .await;
    log(
        &state.control,
        actor,
        "filter.checker_settings.apply",
        id,
        result.is_ok(),
    )
    .await?;
    Ok(Json(result.map_err(map_rpc)?.into_inner().into()))
}

pub(super) fn router(control: Control, socket: PathBuf) -> Router {
    let programs = super::checker_program_http::router(control.clone(), socket.clone());
    Router::new()
        .route(
            "/api/v1/filters/request-checkers",
            get(request_checker_settings)
                .put(update_request_checker_settings)
                .layer(DefaultBodyLimit::max(128 * 1024)),
        )
        .with_state(CheckerState { control, socket })
        .merge(programs)
}
