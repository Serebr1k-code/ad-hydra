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
use serde::Deserialize;
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterInput {
    pub expected_revision: u64,
    pub service_id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    #[schema(value_type=String)]
    pub mode: Mode,
    pub priority: u32,
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
    .map(|v| (StatusCode::ACCEPTED, Json(v)).into_response())
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
        .map(|v| (StatusCode::ACCEPTED, Json(v)).into_response())
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
        .map(|v| (StatusCode::ACCEPTED, Json(v)).into_response())
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
    let fields = crate::modules::traffic::saved_fields(&c.db, input.message_id)
        .await
        .map_err(Error::from)?;
    c.test_rule(a, id, fields, request())
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
    .map(|v| (StatusCode::ACCEPTED, Json(v)).into_response())
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
        from_message
    ),
    components(schemas(
        FilterInput,
        StateInput,
        TestInput,
        BulkStateInput,
        FromMessageInput,
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
