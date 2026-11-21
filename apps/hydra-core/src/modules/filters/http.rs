use super::checker_http::{self, RequestCheckerPolicy, RequestCheckerSettings};
use super::checker_program_http::{
    self as checker_program_http, DeleteProgramInput, PutProgramInput, RequestCheckerProgram,
    RequestCheckerPrograms,
};
use crate::modules::services::{
    Control,
    http::{Error, ListQuery, ListResponse, Revision, actor, list, remove, request},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use hydra_domain::{Action, Expression, Mode, Rule};
use hydra_domain::{FieldValue, RuleSnapshot, Transport};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::{collections::BTreeMap, time::Duration};
use utoipa::ToSchema;

#[derive(Clone)]
struct PreviewState {
    control: Control,
    socket: std::path::PathBuf,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterPreviewInput {
    pub service_id: String,
    pub flow_ids: Vec<String>,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    #[schema(value_type=String)]
    pub mode: Mode,
    pub priority: u32,
    /// Expression AST supports all/any/not/predicate and {direction, scoped}; scoped conditions are inapplicable outside their direction, even under NOT.
    #[schema(value_type=Object)]
    pub expression: Expression,
    #[schema(value_type=Object)]
    pub action: Action,
    pub backend: String,
    pub expires_at_ms: Option<u64>,
}

fn preview_field_object(fields: BTreeMap<String, FieldValue>) -> hydra_wire::v1::FieldObject {
    hydra_wire::v1::FieldObject {
        fields: fields
            .into_iter()
            .map(|(key, value)| (key, hydra_wire::rules::value_to_wire(&value)))
            .collect(),
    }
}

async fn preview_targets(
    control: &Control,
    service: &hydra_domain::Service,
    ids: &[String],
) -> Result<Vec<hydra_wire::v1::FilterPreviewTarget>, Error> {
    let ids = ids.to_vec();
    let expected_service = service.id.clone();
    let transport = service.transport;
    control
        .db
        .run(move |db| {
            let mut targets = Vec::with_capacity(ids.len());
            for id in ids {
                let row: Option<(String, String)> = db
                    .query_row(
                        "SELECT service_id,data FROM traffic_flows WHERE id=?1",
                        [&id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let Some((service_id, data)) = row else {
                    targets.push(hydra_wire::v1::FilterPreviewTarget {
                        flow_id: id,
                        network_fields: None,
                    });
                    continue;
                };
                anyhow::ensure!(
                    service_id == expected_service,
                    "selected stream belongs to another service"
                );
                let data: serde_json::Value = serde_json::from_str(&data)?;
                let mut fields = BTreeMap::new();
                if let Some(client) = data.get("client") {
                    if let Some(ip) = client.get("ip").and_then(serde_json::Value::as_str) {
                        fields.insert("network.src_ip".into(), FieldValue::String(ip.to_owned()));
                    }
                    if let Some(port) = client.get("port").and_then(serde_json::Value::as_u64) {
                        fields.insert("network.src_port".into(), FieldValue::Uint(port));
                    }
                }
                if let Some(upstream) = data.get("upstream") {
                    if let Some(ip) = upstream.get("ip").and_then(serde_json::Value::as_str) {
                        fields.insert("network.dst_ip".into(), FieldValue::String(ip.to_owned()));
                    }
                    if let Some(port) = upstream.get("port").and_then(serde_json::Value::as_u64) {
                        fields.insert("network.dst_port".into(), FieldValue::Uint(port));
                    }
                }
                fields.insert(
                    "transport.kind".into(),
                    FieldValue::String(
                        match transport {
                            Transport::Tcp => "tcp",
                            Transport::Udp => "udp",
                        }
                        .into(),
                    ),
                );
                targets.push(hydra_wire::v1::FilterPreviewTarget {
                    flow_id: id,
                    network_fields: Some(preview_field_object(fields)),
                });
            }
            Ok(targets)
        })
        .await
        .map_err(Error::from)
}

#[utoipa::path(post, path="/api/v1/filters/preview", request_body=FilterPreviewInput, responses((status=200,body=Object),(status=400),(status=401)),tag="filters")]
async fn preview_filter(
    State(state): State<PreviewState>,
    headers: HeaderMap,
    Json(input): Json<FilterPreviewInput>,
) -> Result<Json<serde_json::Value>, Error> {
    if input.flow_ids.is_empty()
        || input.flow_ids.len() > 50
        || input
            .flow_ids
            .iter()
            .any(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(Error::Invalid(anyhow::anyhow!(
            "preview accepts 1 to 50 valid stream IDs"
        )));
    }
    let actor = actor(&state.control, &headers).await?;
    let desired = state.control.desired().await.map_err(Error::from)?;
    let service = desired
        .services
        .services
        .iter()
        .find(|s| s.id == input.service_id)
        .cloned()
        .ok_or(Error::Missing)?;
    let targets = preview_targets(&state.control, &service, &input.flow_ids).await?;
    let request_id = request();
    let rule = Rule {
        id: "__preview__".into(),
        service_id: input.service_id.clone(),
        additional_service_ids: vec![],
        name: input.name,
        description: input.description,
        enabled: input.enabled,
        mode: input.mode,
        priority: input.priority,
        expression: input.expression,
        action: input.action,
        backend: input.backend,
        expires_at_ms: input.expires_at_ms,
    };
    let mut wire = hydra_wire::v1::ApplyRuleSnapshotRequest::from(&RuleSnapshot {
        revision: 1,
        rules: vec![rule],
    });
    let rule = wire
        .rules
        .pop()
        .ok_or_else(|| Error::Invalid(anyhow::anyhow!("invalid candidate filter")))?;
    let rpc = async {
        let channel = crate::infrastructure::proxy_client::connect(&state.socket).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let mut client = hydra_wire::v1::proxy_diagnostics_client::ProxyDiagnosticsClient::new(channel);
        let request = hydra_wire::v1::PreviewFilterRequest {
            targets, rule: Some(rule),
            actor: Some(hydra_wire::v1::Actor { r#type: 1, user_id: actor.id.clone(), request_id: request_id.clone(), ..Default::default() }),
            request_id: request_id.clone(),
        };
        let mut request = tonic::Request::new(request);
        request.set_timeout(Duration::from_secs(8));
        let result = client.preview_filter(request).await.map_err(|status| match status.code() {
            tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
            tonic::Code::NotFound => StatusCode::NOT_FOUND,
            tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_GATEWAY,
        })?.into_inner();
        Ok::<_, StatusCode>(serde_json::json!({"results": result.results.into_iter().map(|r| serde_json::json!({"flow_id":r.flow_id,"state":r.state,"matched":r.matched,"would_block":r.would_block,"matched_messages":r.matched_messages})).collect::<Vec<_>>() }))
    }.await;
    let audit_success = rpc.is_ok();
    state
        .control
        .db
        .run({
            let actor = actor.clone();
            let request_id = request_id.clone();
            let target = input.service_id.clone();
            move |db| {
                let tx = db.transaction()?;
                crate::modules::audit::append(
                    &tx,
                    &actor,
                    "filter.preview",
                    &target,
                    &request_id,
                    audit_success,
                    crate::modules::auth::now(),
                )?;
                tx.commit()?;
                Ok(())
            }
        })
        .await
        .map_err(Error::from)?;
    rpc.map(Json).map_err(Error::Status)
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterInput {
    pub expected_revision: u64,
    pub service_id: String,
    /// Additional services governed by this filter. IDs must be unique.
    #[serde(default)]
    pub additional_service_ids: Vec<String>,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    #[schema(value_type=String)]
    pub mode: Mode,
    pub priority: u32,
    /// Expression AST supports all/any/not/predicate and {direction, scoped}; scoped conditions are inapplicable outside their direction, even under NOT.
    #[schema(value_type=Object)]
    pub expression: Expression,
    #[schema(value_type=Object)]
    pub action: Action,
    pub backend: String,
    pub expires_at_ms: Option<u64>,
}
impl FilterInput {
    fn into_rule(self, id: String) -> Rule {
        Rule {
            id,
            service_id: self.service_id,
            additional_service_ids: self.additional_service_ids,
            name: self.name,
            description: self.description,
            enabled: self.enabled,
            mode: self.mode,
            priority: self.priority,
            expression: self.expression,
            action: self.action,
            backend: self.backend,
            expires_at_ms: self.expires_at_ms,
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StateInput {
    expected_revision: u64,
    enabled: bool,
}
#[utoipa::path(get,path="/api/v1/filters",params(ListQuery),responses((status=200,body=ListResponse),(status=401)),tag="filters")]
pub async fn filters(
    State(c): State<Control>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResponse>, Error> {
    let a = actor(&c, &headers).await?;
    list(&c, a, q, "filter")
        .await
        .map(Json)
        .map_err(Error::from)
}
#[utoipa::path(post,path="/api/v1/filters",request_body=FilterInput,responses((status=202,body=Object),(status=409)),tag="filters")]
pub async fn create_filter(
    State(c): State<Control>,
    headers: HeaderMap,
    Json(input): Json<FilterInput>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    let revision = input.expected_revision;
    c.put_rule(
        a,
        revision,
        input.into_rule(uuid::Uuid::new_v4().to_string()),
        true,
        request(),
    )
    .await
    .map(|v| {
        (
            StatusCode::ACCEPTED,
            Json(crate::modules::services::http::public_desired(v)),
        )
            .into_response()
    })
    .map_err(Error::from)
}
#[utoipa::path(patch,path="/api/v1/filters/{id}",params(("id"=String,Path)),request_body=FilterInput,responses((status=202,body=Object),(status=409)),tag="filters")]
pub async fn update_filter(
    State(c): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<FilterInput>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    let revision = input.expected_revision;
    c.put_rule(a, revision, input.into_rule(id), false, request())
        .await
        .map(|v| {
            (
                StatusCode::ACCEPTED,
                Json(crate::modules::services::http::public_desired(v)),
            )
                .into_response()
        })
        .map_err(Error::from)
}
#[utoipa::path(put,path="/api/v1/filters/{id}/state",params(("id"=String,Path)),request_body=StateInput,responses((status=202,body=Object),(status=409)),tag="filters")]
pub async fn filter_state(
    State(c): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<StateInput>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    let mut rule = c
        .desired()
        .await
        .map_err(Error::from)?
        .rules
        .rules
        .into_iter()
        .find(|r| r.id == id)
        .ok_or(Error::Missing)?;
    rule.enabled = input.enabled;
    c.put_rule(a, input.expected_revision, rule, false, request())
        .await
        .map(|v| {
            (
                StatusCode::ACCEPTED,
                Json(crate::modules::services::http::public_desired(v)),
            )
                .into_response()
        })
        .map_err(Error::from)
}
#[utoipa::path(delete,path="/api/v1/filters/{id}",params(("id"=String,Path),("expected_revision"=u64,Query)),responses((status=202,body=Object),(status=409)),tag="filters")]
pub async fn delete_filter(
    State(c): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Revision>,
) -> Result<Response, Error> {
    remove(c, headers, id, q, "filter").await
}
#[utoipa::path(get,path="/api/v1/filters/{id}",params(("id"=String,Path)),responses((status=200,body=Object),(status=404)),tag="filters")]
pub async fn get_filter(
    State(c): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    let mut after = String::new();
    loop {
        let result = list(
            &c,
            a.clone(),
            ListQuery {
                after,
                q: Some(id.clone()),
                limit: Some(100),
            },
            "filter",
        )
        .await
        .map_err(Error::from)?;
        if let Some(rule) = result
            .items
            .into_iter()
            .find(|r| r["id"].as_str() == Some(&id))
        {
            return Ok(Json(rule).into_response());
        }
        match result.next_cursor {
            Some(cursor) => after = cursor,
            None => return Err(Error::Missing),
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TestInput {
    pub message_id: String,
}
#[utoipa::path(post,path="/api/v1/filters/{id}/test",params(("id"=String,Path)),request_body=TestInput,responses((status=200,body=Object)),tag="filters")]
pub async fn test_filter(
    State(c): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<TestInput>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    let (service_id, fields) = crate::modules::traffic::saved_context(&c.db, input.message_id)
        .await
        .map_err(Error::from)?;
    c.test_rule(a, id, service_id, fields, request())
        .await
        .map(|v| (StatusCode::OK, Json(v)).into_response())
        .map_err(Error::from)
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkStateInput {
    expected_revision: u64,
    ids: Vec<String>,
    enabled: bool,
}
#[utoipa::path(put,path="/api/v1/filters/state",request_body=BulkStateInput,responses((status=202,body=Object),(status=400),(status=409)),tag="filters")]
pub async fn bulk_state(
    State(c): State<Control>,
    headers: HeaderMap,
    Json(input): Json<BulkStateInput>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    c.bulk_rule_state(
        a,
        input.expected_revision,
        input.ids,
        input.enabled,
        request(),
    )
    .await
    .map(|v| {
        (
            StatusCode::ACCEPTED,
            Json(crate::modules::services::http::public_desired(v)),
        )
            .into_response()
    })
    .map_err(Error::from)
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FromMessageInput {
    expected_revision: u64,
    message_id: String,
    field: String,
    name: String,
    #[schema(value_type=Object)]
    action: Action,
}
#[utoipa::path(post,path="/api/v1/filters/from-message",request_body=FromMessageInput,responses((status=202,body=Object),(status=400),(status=409)),tag="filters")]
async fn from_message(
    State(c): State<Control>,
    headers: HeaderMap,
    Json(input): Json<FromMessageInput>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    let (service_id, mut fields) =
        crate::modules::traffic::saved_context(&c.db, input.message_id.clone())
            .await
            .map_err(Error::from)?;
    let value = fields.remove(&input.field).ok_or(Error::Missing)?;
    let rule = Rule {
        id: uuid::Uuid::new_v4().to_string(),
        service_id,
        additional_service_ids: vec![],
        name: input.name,
        description: format!("From saved message {}", input.message_id),
        enabled: true,
        mode: Mode::Enforce,
        priority: 10,
        expression: Expression::Predicate {
            field: input.field,
            op: hydra_domain::Comparison::Eq,
            value,
        },
        action: input.action,
        backend: "userspace".into(),
        expires_at_ms: None,
    };
    c.put_rule(a, input.expected_revision, rule, true, request())
        .await
        .map(|v| (StatusCode::ACCEPTED, Json(v)).into_response())
        .map_err(Error::from)
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        filters,
        create_filter,
        update_filter,
        filter_state,
        delete_filter,
        get_filter,
        test_filter,
        bulk_state,
        from_message,
        preview_filter,
        checker_http::request_checker_settings,
        checker_http::update_request_checker_settings,
        checker_program_http::list_request_checker_programs,
        checker_program_http::put_request_checker_program,
        checker_program_http::delete_request_checker_program
    ),
    components(schemas(
        FilterInput,
        StateInput,
        TestInput,
        BulkStateInput,
        FromMessageInput,
        FilterPreviewInput,
        RequestCheckerPolicy,
        RequestCheckerSettings,
        RequestCheckerProgram,
        RequestCheckerPrograms,
        PutProgramInput,
        DeleteProgramInput,
        ListResponse
    ))
)]
pub struct FilterApi;
pub fn router(control: Control) -> Router {
    Router::new()
        .route("/api/v1/filters", get(filters).post(create_filter))
        .route(
            "/api/v1/filters/{id}",
            get(get_filter).patch(update_filter).delete(delete_filter),
        )
        .route("/api/v1/filters/state", put(bulk_state))
        .route("/api/v1/filters/from-message", post(from_message))
        .route("/api/v1/filters/{id}/state", put(filter_state))
        .route("/api/v1/filters/{id}/test", post(test_filter))
        .with_state(control)
}

pub fn preview_router(control: Control, socket: std::path::PathBuf) -> Router {
    Router::new()
        .route("/api/v1/filters/preview", post(preview_filter))
        .with_state(PreviewState {
            control: control.clone(),
            socket: socket.clone(),
        })
        .merge(checker_http::router(control, socket))
}
