use super::{AuthState, TokenResponse, Tokens, now};
use crate::modules::{audit::Actor, users};
use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, Query, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use utoipa::ToSchema;
pub struct AuthError;
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED,Json(serde_json::json!({"error":{"code":"authentication_failed","message":"Authentication failed"}}))).into_response()
    }
}
impl From<anyhow::Error> for AuthError {
    fn from(_: anyhow::Error) -> Self {
        Self
    }
}
pub struct Authenticated(pub Actor);
impl<S> FromRequestParts<S> for Authenticated
where
    AuthState: axum::extract::FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = <AuthState as axum::extract::FromRef<S>>::from_ref(state);
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(AuthError)?;
        Ok(Self(state.authenticate(token).await?))
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    username: String,
    password: String,
}
fn request_id() -> String {
    crate::modules::audit::request_id()
}
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|part| part.trim().strip_prefix(&prefix).map(str::to_owned))
}
fn origin(state: &AuthState, headers: &HeaderMap) -> Result<(), AuthError> {
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) == Some(state.origin.as_str()) {
        Ok(())
    } else {
        Err(AuthError)
    }
}
fn csrf(state: &AuthState, headers: &HeaderMap) -> Result<(), AuthError> {
    origin(state, headers)?;
    let value = cookie(headers, "hydra_csrf").ok_or(AuthError)?;
    if value.len() >= 32
        && headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) == Some(value.as_str())
    {
        Ok(())
    } else {
        Err(AuthError)
    }
}
fn token_response(tokens: Tokens) -> Result<Response, AuthError> {
    let age = (tokens.deadline - now()).max(0);
    let mut response = Json(tokens.response).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().map_err(|_| AuthError)?,
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        format!(
            "hydra_refresh={}; HttpOnly; Secure; SameSite=Strict; Path=/api/v1/auth; Max-Age={age}",
            tokens.refresh
        )
        .parse()
        .map_err(|_| AuthError)?,
    );
    Ok(response)
}
fn attach_csrf(mut response: Response, value: &str, age: i64) -> Result<Response, AuthError> {
    response.headers_mut().append(
        header::SET_COOKIE,
        format!("hydra_csrf={value}; Secure; SameSite=Strict; Path=/; Max-Age={age}")
            .parse()
            .map_err(|_| AuthError)?,
    );
    Ok(response)
}
#[utoipa::path(post,path="/api/v1/auth/login",request_body=LoginRequest,responses((status=200,body=TokenResponse),(status=401)),tag="auth")]
pub async fn login(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AuthError> {
    origin(&state, &headers)?;
    let tokens = state
        .login(body.username, body.password, request_id())
        .await?;
    let csrf = tokens.response.csrf_token.clone();
    let age = (tokens.deadline - now()).max(0);
    attach_csrf(token_response(tokens)?, &csrf, age)
}
#[utoipa::path(post,path="/api/v1/auth/refresh",params(("X-CSRF-Token"=String,Header,description="Value of the root-scoped hydra_csrf cookie")),responses((status=200,body=TokenResponse),(status=401)),tag="auth")]
pub async fn refresh(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    csrf(&state, &headers)?;
    let tokens = state
        .refresh(
            cookie(&headers, "hydra_refresh").ok_or(AuthError)?,
            request_id(),
        )
        .await?;
    let csrf = tokens.response.csrf_token.clone();
    let age = (tokens.deadline - now()).max(0);
    attach_csrf(token_response(tokens)?, &csrf, age)
}
#[utoipa::path(post,path="/api/v1/auth/logout",params(("X-CSRF-Token"=String,Header,description="Value of hydra_csrf cookie; access bearer and refresh cookie must identify the same session")),responses((status=204),(status=401)),tag="auth")]
pub async fn logout(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    csrf(&state, &headers)?;
    let token = cookie(&headers, "hydra_refresh").ok_or(AuthError)?;
    let access = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or(AuthError)?;
    state.authenticate(access).await?;
    let ac = state.decode_at(access, "access", now())?;
    let rc = state.decode_at(&token, "refresh", now())?;
    if ac.sid != rc.sid || ac.sub != rc.sub {
        return Err(AuthError);
    }
    state.logout(token, request_id()).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    for (name, path) in [("hydra_refresh", "/api/v1/auth"), ("hydra_csrf", "/")] {
        response.headers_mut().append(
            header::SET_COOKIE,
            format!("{name}=; Secure; HttpOnly; SameSite=Strict; Path={path}; Max-Age=0")
                .parse()
                .map_err(|_| AuthError)?,
        );
    }
    Ok(response)
}
#[utoipa::path(get,path="/api/v1/auth/me",responses((status=200,body=Actor),(status=401)),tag="auth")]
pub async fn me(Authenticated(actor): Authenticated) -> Json<Actor> {
    Json(actor)
}
#[utoipa::path(get,path="/api/v1/users",responses((status=200,body=Vec<users::User>),(status=401)),tag="users")]
pub async fn list_users(
    State(state): State<AuthState>,
    Authenticated(actor): Authenticated,
) -> Result<Json<Vec<users::User>>, AuthError> {
    Ok(Json(users::list(&state.db, actor, request_id()).await?))
}
#[utoipa::path(post,path="/api/v1/users/{id}/deactivate",params(("id"=String,Path)),responses((status=204),(status=401)),tag="users")]
pub async fn deactivate(
    State(state): State<AuthState>,
    Authenticated(actor): Authenticated,
    Path(id): Path<String>,
) -> Result<StatusCode, AuthError> {
    users::deactivate(&state.db, actor, id, request_id()).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(patch,path="/api/v1/users/{id}",params(("id"=String,Path)),request_body=users::UpdateUser,responses((status=204),(status=400),(status=409),(status=401)),tag="users")]
pub async fn update_user(
    State(state): State<AuthState>,
    Authenticated(actor): Authenticated,
    Path(id): Path<String>,
    Json(input): Json<users::UpdateUser>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    users::update(&state,actor,id,input,request_id()).await.map_err(|e|{
        let code=if e.to_string()=="revision conflict"{StatusCode::CONFLICT}else{StatusCode::BAD_REQUEST};
        (code,Json(serde_json::json!({"error":{"code":"user_update_failed","message":"Account update failed; check fields and revision"}})))
    })?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(serde::Serialize, ToSchema)]
pub struct Session {
    id: String,
    expires_at: String,
    revoked: bool,
}
#[utoipa::path(get,path="/api/v1/auth/sessions",responses((status=200,body=Vec<Session>),(status=401)),tag="auth")]
pub async fn sessions(
    State(state): State<AuthState>,
    Authenticated(actor): Authenticated,
) -> Result<Json<Vec<Session>>, AuthError> {
    let rows=state.db.run(move|db|{let tx=db.transaction()?;let rows=tx.prepare("SELECT id,strftime('%Y-%m-%dT%H:%M:%SZ',expires_at,'unixepoch'),revoked FROM sessions WHERE user_id=?1 ORDER BY expires_at DESC LIMIT 100")?.query_map([&actor.id],|r|Ok(Session{id:r.get(0)?,expires_at:r.get(1)?,revoked:r.get(2)?}))?.collect::<Result<Vec<_>,_>>()?;crate::modules::audit::append(&tx,&actor,"sessions.list",&actor.id, &request_id(),true,now())?;tx.commit()?;Ok(rows)}).await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    after: i64,
}
#[utoipa::path(get,path="/api/v1/audit",params(("after"=Option<i64>,Query,description="Exclusive event ID cursor")),responses((status=200,body=Vec<crate::modules::audit::AuditEvent>),(status=401)),tag="audit")]
pub async fn audit_events(
    State(state): State<AuthState>,
    Authenticated(actor): Authenticated,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<crate::modules::audit::AuditEvent>>, AuthError> {
    Ok(Json(
        crate::modules::audit::list(&state.db, actor, query.after, request_id()).await?,
    ))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        login,
        refresh,
        logout,
        me,
        sessions,
        list_users,
        deactivate,
        update_user,
        audit_events
    ),
    components(schemas(
        LoginRequest,
        TokenResponse,
        Actor,
        users::User,
        users::UpdateUser,
        Session,
        crate::modules::audit::AuditEvent
    ))
)]
pub struct IdentityApi;
pub fn router(state: AuthState) -> Router {
    Router::new()
        .route("/api/v1/audit", get(audit_events))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/auth/sessions", get(sessions))
        .route("/api/v1/users", get(list_users))
        .route("/api/v1/users/{id}/deactivate", post(deactivate))
        .route("/api/v1/users/{id}", axum::routing::patch(update_user))
        .with_state(state)
}
