use super::{Page, Query as TrafficQuery, Traffic};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::get,
};
use serde_json::Value;
use std::{convert::Infallible, time::Duration};
use tokio::sync::{broadcast::error::RecvError, mpsc};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
struct Error(StatusCode);
fn query_error(operation: &'static str, error: anyhow::Error) -> Error {
    tracing::warn!(
        request_id = %crate::modules::audit::request_id(),
        operation,
        error = %error,
        "Traffic query failed"
    );
    Error(StatusCode::BAD_REQUEST)
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (code, message) = if self.0 == StatusCode::NOT_FOUND {
            (
                "traffic_not_retained",
                "This traffic item is no longer retained or does not exist. Refresh Traffic and select a current stream. Save streams as favourites to preserve them during rotation.",
            )
        } else {
            ("traffic_request_failed", "Traffic request failed")
        };
        (
            self.0,
            Json(serde_json::json!({"error":{"code":code,"message":message}})),
        )
            .into_response()
    }
}
async fn authorize(t: &Traffic, h: &HeaderMap) -> Result<String, Error> {
    let token = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(Error(StatusCode::UNAUTHORIZED))?;
    t.auth
        .authenticate(token)
        .await
        .map_err(|_| Error(StatusCode::UNAUTHORIZED))?;
    Ok(token.into())
}
#[utoipa::path(get,path="/api/v1/traffic/flows",params(TrafficQuery),responses((status=200,body=Page),(status=401)),tag="traffic")]
async fn flows(
    State(t): State<Traffic>,
    h: HeaderMap,
    Query(q): Query<TrafficQuery>,
) -> Result<Json<Page>, Error> {
    authorize(&t, &h).await?;
    t.flows(q)
        .await
        .map(Json)
        .map_err(|error| query_error("flows", error))
}
#[utoipa::path(get,path="/api/v1/traffic/flows/{id}",params(("id"=String,Path)),responses((status=200,body=Object),(status=404)),tag="traffic")]
async fn flow(
    State(t): State<Traffic>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    authorize(&t, &h).await?;
    t.flow(id)
        .await
        .map_err(|error| query_error("flow", error))?
        .map(Json)
        .ok_or(Error(StatusCode::NOT_FOUND))
}
#[utoipa::path(get,path="/api/v1/traffic/flows/{id}/messages",params(("id"=String,Path),TrafficQuery),responses((status=200,body=Page),(status=401)),tag="traffic")]
async fn messages(
    State(t): State<Traffic>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<TrafficQuery>,
) -> Result<Json<Page>, Error> {
    authorize(&t, &h).await?;
    t.messages(id, q)
        .await
        .map(Json)
        .map_err(|error| query_error("messages", error))
}
#[utoipa::path(get,path="/api/v1/traffic/messages/{id}",params(("id"=String,Path)),responses((status=200,body=Object),(status=404)),tag="traffic")]
async fn message(
    State(t): State<Traffic>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    authorize(&t, &h).await?;
    t.message(id)
        .await
        .map_err(|error| query_error("message", error))?
        .map(Json)
        .ok_or(Error(StatusCode::NOT_FOUND))
}
#[utoipa::path(get,path="/api/v1/traffic/events/stream",responses((status=200,description="Bounded parsed SSE; closes on access expiry",content_type="text/event-stream",body=String),(status=401)),tag="traffic")]
async fn stream(State(t): State<Traffic>, h: HeaderMap) -> Result<Response, Error> {
    let token = authorize(&t, &h).await?;
    let claims = t
        .auth
        .decode_at(&token, "access", crate::modules::auth::now())
        .map_err(|_| Error(StatusCode::UNAUTHORIZED))?;
    let permit = t
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error(StatusCode::TOO_MANY_REQUESTS))?;
    let mut events = t.events.subscribe();
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(16);
    tokio::spawn(async move {
        let _permit = permit;
        let expiry = tokio::time::sleep(Duration::from_secs(
            (claims.exp - crate::modules::auth::now()).max(0) as u64,
        ));
        tokio::pin!(expiry);
        let mut verify = tokio::time::interval(Duration::from_secs(5));
        loop {
            let event = tokio::select! {_= &mut expiry=>break,_=tx.closed()=>break,_=verify.tick()=>{if t.auth.authenticate(&token).await.is_err(){break}continue},next=events.recv()=>match next{Ok(v)=>Event::default().event("traffic").json_data(v).unwrap_or_else(|_|Event::default().event("loss").data("serialization")),Err(RecvError::Lagged(n))=>Event::default().event("loss").data(n.to_string()),Err(RecvError::Closed)=>break}};
            tokio::select! {_= &mut expiry=>break,result=tx.send(Ok(event))=>if result.is_err(){break}}
        }
    });
    let mut response = Sse::new(
        tokio_stream::once(Ok(Event::default().event("ready").data("{}")))
            .chain(ReceiverStream::new(rx)),
    )
    .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(10)))
    .into_response();
    response.headers_mut().insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    Ok(response)
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(flows, flow, messages, message, stream),
    components(schemas(Page))
)]
pub struct TrafficApi;
pub fn router(t: Traffic) -> Router {
    Router::new()
        .route("/api/v1/traffic/flows", get(flows))
        .route("/api/v1/traffic/flows/{id}", get(flow))
        .route("/api/v1/traffic/flows/{id}/messages", get(messages))
        .route("/api/v1/traffic/messages/{id}", get(message))
        .route("/api/v1/traffic/events/stream", get(stream))
        .with_state(t)
}
