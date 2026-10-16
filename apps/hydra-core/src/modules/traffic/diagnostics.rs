//! Audited, explicitly requested diagnostic bytes; separate from parsed telemetry.
use crate::{
    infrastructure::proxy_client,
    modules::{audit, auth::AuthState},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use hydra_wire::v1::{self, proxy_diagnostics_client::ProxyDiagnosticsClient};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use utoipa::{OpenApi, ToSchema};
#[derive(Clone)]
pub struct Diagnostics {
    pub auth: AuthState,
    pub socket: PathBuf,
}
#[derive(Serialize, ToSchema)]
pub struct Capture {
    pub sample_id: String,
}
#[derive(Serialize, ToSchema)]
pub struct Sample {
    pub data: Vec<u8>,
    pub truncated: bool,
    pub expires_at: String,
}
async fn actor(
    state: &Diagnostics,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<audit::Actor, StatusCode> {
    if mutation
        && headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
            != Some(state.auth.origin.as_str())
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .auth
        .authenticate(token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)
}
async fn record(
    state: &Diagnostics,
    actor: audit::Actor,
    action: &'static str,
    target: String,
    request: String,
    success: bool,
) -> Result<(), StatusCode> {
    state
        .auth
        .db
        .run(move |db| {
            let tx = db.transaction()?;
            audit::append(
                &tx,
                &actor,
                action,
                &target,
                &request,
                success,
                crate::modules::auth::now(),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
#[utoipa::path(post,path="/api/v1/traffic/flows/{id}/capture",params(("id"=String,Path)),responses((status=202,body=Capture),(status=404),(status=503)),tag="traffic")]
async fn capture(
    State(state): State<Diagnostics>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Capture>), StatusCode> {
    let actor = actor(&state, &headers, true).await?;
    let request = uuid::Uuid::new_v4().to_string();
    record(
        &state,
        actor.clone(),
        "traffic.capture.request",
        id.clone(),
        request.clone(),
        true,
    )
    .await?;
    let channel = proxy_client::connect(&state.socket)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let result = ProxyDiagnosticsClient::new(channel)
        .capture_sample(v1::CaptureSampleRequest {
            flow_id: id.clone(),
            request_id: request.clone(),
            actor: Some(v1::Actor {
                r#type: 1,
                user_id: actor.id.clone(),
                ..Default::default()
            }),
        })
        .await;
    record(
        &state,
        actor,
        "traffic.capture.result",
        id,
        request,
        result.is_ok(),
    )
    .await?;
    let result = result.map_err(map_status)?.into_inner();
    Ok((
        StatusCode::ACCEPTED,
        Json(Capture {
            sample_id: result.sample_id,
        }),
    ))
}
#[utoipa::path(get,path="/api/v1/traffic/samples/{id}",params(("id"=String,Path)),responses((status=200,body=Sample),(status=410,description="Sample expired, not yet captured, or unavailable"),(status=503)),tag="traffic")]
async fn sample(
    State(state): State<Diagnostics>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Sample>, StatusCode> {
    let actor = actor(&state, &headers, false).await?;
    let request = uuid::Uuid::new_v4().to_string();
    record(
        &state,
        actor.clone(),
        "traffic.sample.read",
        id.clone(),
        request.clone(),
        true,
    )
    .await?;
    let channel = proxy_client::connect(&state.socket)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let result = ProxyDiagnosticsClient::new(channel)
        .get_sample(v1::GetSampleRequest {
            sample_id: id.clone(),
        })
        .await;
    record(
        &state,
        actor,
        "traffic.sample.result",
        id,
        request,
        result.is_ok(),
    )
    .await?;
    let result = result
        .map_err(|e| {
            if e.code() == tonic::Code::NotFound {
                StatusCode::GONE
            } else {
                map_status(e)
            }
        })?
        .into_inner();
    let seconds = result.expires_at.map(|v| v.seconds).unwrap_or_default();
    let expires_at = state
        .auth
        .db
        .run(move |db| {
            Ok(db.query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%SZ',?1,'unixepoch')",
                [seconds],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(Sample {
        data: result.data,
        truncated: result.truncated,
        expires_at,
    }))
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProbeInput {
    pub transport: String,
    pub payload: Vec<u8>,
    pub timeout_ms: u32,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceProbeInput {
    pub service_id: String,
    pub transport: String,
    pub payload: Vec<u8>,
    pub timeout_ms: u32,
}
#[utoipa::path(post,path="/api/v1/diagnostics/probe",request_body=ServiceProbeInput,responses((status=200,body=ProbeOutput),(status=400),(status=503)),tag="services")]
async fn diagnostic_probe(
    State(state): State<Diagnostics>,
    headers: HeaderMap,
    Json(input): Json<ServiceProbeInput>,
) -> Result<Json<ProbeOutput>, StatusCode> {
    probe(
        State(state),
        headers,
        Path(input.service_id),
        Json(ProbeInput {
            transport: input.transport,
            payload: input.payload,
            timeout_ms: input.timeout_ms,
        }),
    )
    .await
}
#[derive(Serialize, ToSchema)]
pub struct ProbeOutput {
    pub response: Vec<u8>,
    pub truncated: bool,
    pub elapsed_ms: u32,
    pub error_code: String,
}
#[utoipa::path(post,path="/api/v1/services/{id}/probe",params(("id"=String,Path)),request_body=ProbeInput,responses((status=200,body=ProbeOutput),(status=400),(status=503)),tag="services")]
async fn probe(
    State(state): State<Diagnostics>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ProbeInput>,
) -> Result<Json<ProbeOutput>, StatusCode> {
    let actor = actor(&state, &headers, true).await?;
    if input.payload.len() > 4096 || !(50..=5000).contains(&input.timeout_ms) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let transport = match input.transport.as_str() {
        "tcp" => 1,
        "udp" => 2,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let request = uuid::Uuid::new_v4().to_string();
    record(
        &state,
        actor.clone(),
        "service.probe.request",
        id.clone(),
        request.clone(),
        true,
    )
    .await?;
    let channel = proxy_client::connect(&state.socket)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let result = ProxyDiagnosticsClient::new(channel)
        .probe_service(v1::ProbeServiceRequest {
            service_id: id.clone(),
            transport,
            payload: input.payload,
            timeout_ms: input.timeout_ms,
            request_id: request.clone(),
            actor: Some(v1::Actor {
                r#type: 1,
                user_id: actor.id.clone(),
                ..Default::default()
            }),
        })
        .await;
    record(
        &state,
        actor,
        "service.probe.result",
        id,
        request,
        result.is_ok(),
    )
    .await?;
    let result = result.map_err(map_status)?.into_inner();
    Ok(Json(ProbeOutput {
        response: result.response,
        truncated: result.truncated,
        elapsed_ms: result.elapsed_ms,
        error_code: result.error_code,
    }))
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProtocolTest {
    pub sample_id: String,
    pub direction: String,
}
#[utoipa::path(post,path="/api/v1/protocols/{id}/{version}/test",params(("id"=String,Path),("version"=String,Path)),request_body=ProtocolTest,responses((status=200,body=Object),(status=410),(status=400)),tag="protocols")]
async fn test_protocol(
    State(state): State<Diagnostics>,
    headers: HeaderMap,
    Path((id, version)): Path<(String, String)>,
    Json(input): Json<ProtocolTest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let actor = actor(&state, &headers, true).await?;
    let direction = match input.direction.as_str() {
        "client_to_server" => 1,
        "server_to_client" => 2,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let request = uuid::Uuid::new_v4().to_string();
    record(
        &state,
        actor.clone(),
        "protocol.sample.test",
        input.sample_id.clone(),
        request.clone(),
        true,
    )
    .await?;
    let channel = proxy_client::connect(&state.socket)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let result = ProxyDiagnosticsClient::new(channel)
        .test_protocol(v1::TestProtocolRequest {
            protocol_id: id.clone(),
            version: version.clone(),
            sample_id: input.sample_id,
            direction,
            request_id: request.clone(),
            actor: Some(v1::Actor {
                r#type: 1,
                user_id: actor.id.clone(),
                ..Default::default()
            }),
        })
        .await;
    record(
        &state,
        actor,
        "protocol.test.result",
        format!("{id}/{version}"),
        request,
        result.is_ok(),
    )
    .await?;
    let result = result
        .map_err(|e| {
            if e.code() == tonic::Code::NotFound {
                StatusCode::GONE
            } else {
                map_status(e)
            }
        })?
        .into_inner();
    let messages=result.messages.into_iter().map(|m|{
        let fields=m.fields.unwrap_or_default().fields.into_iter().map(|(key,value)|Ok((key,super::repository::field(value,0,&mut 0).map_err(|e|e.to_string())?))).collect::<Result<std::collections::BTreeMap<_,_>,String>>().map_err(|_|StatusCode::BAD_GATEWAY)?;
        Ok(serde_json::json!({"protocol_id":m.protocol_id,"message_type":m.message_type,"direction":m.direction,"fields":fields}))
    }).collect::<Result<Vec<_>,StatusCode>>()?;
    Ok(Json(serde_json::json!({"messages":messages})))
}
fn map_status(e: tonic::Status) -> StatusCode {
    match e.code() {
        tonic::Code::NotFound => StatusCode::NOT_FOUND,
        tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
        tonic::Code::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}
#[derive(OpenApi)]
#[openapi(
    paths(capture, sample, probe, diagnostic_probe, test_protocol),
    components(schemas(
        Capture,
        Sample,
        ProbeInput,
        ServiceProbeInput,
        ProbeOutput,
        ProtocolTest
    ))
)]
pub struct DiagnosticsApi;
pub fn router(state: Diagnostics) -> Router {
    Router::new()
        .route("/api/v1/traffic/flows/{id}/capture", post(capture))
        .route("/api/v1/traffic/samples/{id}", get(sample))
        .route("/api/v1/services/{id}/probe", post(probe))
        .route("/api/v1/diagnostics/probe", post(diagnostic_probe))
        .route("/api/v1/protocols/{id}/{version}/test", post(test_protocol))
        .with_state(state)
}
