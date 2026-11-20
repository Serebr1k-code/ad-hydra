use super::checker_http::{
    CheckerState, checker_error, map_rpc, rpc_request, unavailable, wire_actor,
};
use crate::{
    infrastructure::proxy_client,
    modules::{
        audit, auth,
        services::http::{Error, actor, request},
    },
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use hydra_wire::v1::{self, proxy_control_client::ProxyControlClient};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use utoipa::ToSchema;

const MAX_SOURCE_BYTES: usize = 256 * 1024;

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestCheckerProgram {
    id: String,
    runtime: String,
    managed: bool,
    source: String,
}

#[derive(Serialize, ToSchema)]
pub(super) struct RequestCheckerPrograms {
    revision: u64,
    programs: Vec<RequestCheckerProgram>,
}

impl From<v1::RequestCheckerPrograms> for RequestCheckerPrograms {
    fn from(value: v1::RequestCheckerPrograms) -> Self {
        Self {
            revision: value.revision,
            programs: value
                .programs
                .into_iter()
                .map(|program| RequestCheckerProgram {
                    id: program.id,
                    runtime: program.runtime,
                    managed: program.managed,
                    source: program.source,
                })
                .collect(),
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PutProgramInput {
    revision: u64,
    source: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DeleteProgramInput {
    revision: u64,
}

fn valid_program_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn validate_source(source: &str) -> Result<(), Error> {
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
        return Err(checker_error(
            StatusCode::BAD_REQUEST,
            "Python source must be between 1 byte and 256 KiB",
        ));
    }
    Ok(())
}

async fn log_program(
    control: &crate::modules::services::Control,
    actor: audit::Actor,
    action: &'static str,
    target: String,
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
                &target,
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

#[utoipa::path(get, path = "/api/v1/filters/request-checkers/programs",
    responses((status = 200, body = RequestCheckerPrograms), (status = 401), (status = 503)), tag = "filters")]
pub(super) async fn list_request_checker_programs(
    State(state): State<CheckerState>,
    headers: HeaderMap,
) -> Result<Json<RequestCheckerPrograms>, Error> {
    let actor = actor(&state.control, &headers).await?;
    let id = request();
    let result = async {
        let channel = proxy_client::connect(&state.socket)
            .await
            .map_err(|_| unavailable())?;
        ProxyControlClient::new(channel)
            .list_request_checker_programs(rpc_request(v1::GetRequestCheckerSettingsRequest {
                actor: Some(wire_actor(&actor, &id)),
                request_id: id.clone(),
            }))
            .await
            .map_err(map_rpc)
            .map(|r| r.into_inner())
    }
    .await;
    log_program(
        &state.control,
        actor,
        "filter.checker_programs.read",
        "request-checker-programs".into(),
        id,
        result.is_ok(),
    )
    .await?;
    result.map(|value| Json(value.into()))
}

#[utoipa::path(put, path = "/api/v1/filters/request-checkers/programs/{program_id}", request_body = PutProgramInput,
    params(("program_id" = String, Path, description = "Managed checker identifier")),
    responses((status = 200, body = RequestCheckerPrograms), (status = 400), (status = 401), (status = 409), (status = 503)), tag = "filters")]
pub(super) async fn put_request_checker_program(
    State(state): State<CheckerState>,
    headers: HeaderMap,
    Path(program_id): Path<String>,
    Json(input): Json<PutProgramInput>,
) -> Result<Json<RequestCheckerPrograms>, Error> {
    let actor = actor(&state.control, &headers).await?;
    if !valid_program_id(&program_id) {
        return Err(checker_error(
            StatusCode::BAD_REQUEST,
            "Checker ID must use 1-32 letters, numbers, underscores, or hyphens",
        ));
    }
    validate_source(&input.source)?;
    let id = request();
    log_program(
        &state.control,
        actor.clone(),
        "filter.checker_programs.write.intent",
        program_id.clone(),
        id.clone(),
        true,
    )
    .await?;
    let result = async {
        let channel = proxy_client::connect(&state.socket)
            .await
            .map_err(|_| unavailable())?;
        ProxyControlClient::new(channel)
            .put_request_checker_program(rpc_request(v1::PutRequestCheckerProgramRequest {
                actor: Some(wire_actor(&actor, &id)),
                request_id: id.clone(),
                revision: input.revision,
                id: program_id.clone(),
                source: input.source,
            }))
            .await
            .map_err(map_rpc)
            .map(|r| r.into_inner())
    }
    .await;
    log_program(
        &state.control,
        actor,
        "filter.checker_programs.write",
        program_id,
        id,
        result.is_ok(),
    )
    .await?;
    result.map(|value| Json(value.into()))
}

#[utoipa::path(delete, path = "/api/v1/filters/request-checkers/programs/{program_id}", request_body = DeleteProgramInput,
    params(("program_id" = String, Path, description = "Managed checker identifier")),
    responses((status = 200, body = RequestCheckerPrograms), (status = 400), (status = 401), (status = 409), (status = 503)), tag = "filters")]
pub(super) async fn delete_request_checker_program(
    State(state): State<CheckerState>,
    headers: HeaderMap,
    Path(program_id): Path<String>,
    Json(input): Json<DeleteProgramInput>,
) -> Result<Json<RequestCheckerPrograms>, Error> {
    let actor = actor(&state.control, &headers).await?;
    if !valid_program_id(&program_id) {
        return Err(checker_error(StatusCode::BAD_REQUEST, "Invalid checker ID"));
    }
    let id = request();
    log_program(
        &state.control,
        actor.clone(),
        "filter.checker_programs.delete.intent",
        program_id.clone(),
        id.clone(),
        true,
    )
    .await?;
    let result = async {
        let channel = proxy_client::connect(&state.socket)
            .await
            .map_err(|_| unavailable())?;
        ProxyControlClient::new(channel)
            .delete_request_checker_program(rpc_request(v1::DeleteRequestCheckerProgramRequest {
                actor: Some(wire_actor(&actor, &id)),
                request_id: id.clone(),
                revision: input.revision,
                id: program_id.clone(),
            }))
            .await
            .map_err(map_rpc)
            .map(|r| r.into_inner())
    }
    .await;
    log_program(
        &state.control,
        actor,
        "filter.checker_programs.delete",
        program_id,
        id,
        result.is_ok(),
    )
    .await?;
    result.map(|value| Json(value.into()))
}

pub(super) fn router(control: crate::modules::services::Control, socket: PathBuf) -> Router {
    Router::new()
        .route(
            "/api/v1/filters/request-checkers/programs",
            get(list_request_checker_programs),
        )
        .route(
            "/api/v1/filters/request-checkers/programs/{program_id}",
            axum::routing::put(put_request_checker_program)
                .delete(delete_request_checker_program)
                .layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .with_state(CheckerState { control, socket })
}
