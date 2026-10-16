//! Mutation attempts are durable even when validation rejects the business transaction.
use crate::modules::{
    audit::{self, Actor},
    auth::AuthState,
};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
pub async fn middleware(State(auth): State<AuthState>, request: Request, next: Next) -> Response {
    let id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let path = request.uri().path().chars().take(256).collect::<String>();
    let method = request.method().as_str().to_owned();
    let mutation = path.starts_with("/api/v1/")
        && !path.starts_with("/api/v1/auth/")
        && matches!(
            *request.method(),
            Method::POST | Method::PUT | Method::PATCH | Method::DELETE
        );
    let actor = if mutation {
        let token = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        Some(
            match token {
                Some(token) => auth.authenticate(token).await.ok(),
                None => None,
            }
            .unwrap_or(Actor {
                kind: "service".into(),
                id: "unauthenticated-http-request".into(),
            }),
        )
    } else {
        None
    };
    if let Some(actor) = actor.as_ref()
        && audit::http_attempt(
            &auth.db,
            actor.clone(),
            id.clone(),
            path.clone(),
            method.clone(),
            None,
        )
        .await
        .is_err()
    {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_unavailable",
            "Mutation audit unavailable",
            &id,
        );
    }
    let mut response = audit::scope_request(id.clone(), next.run(request)).await;
    if let Some(actor) = actor
        && audit::http_attempt(
            &auth.db,
            actor,
            id.clone(),
            path.clone(),
            method,
            Some(response.status().as_u16()),
        )
        .await
        .is_err()
    {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_result_unavailable",
            "Mutation result could not be audited; check its current state",
            &id,
        );
    }
    if path.starts_with("/api/v1/") {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if response.status().is_client_error() || response.status().is_server_error() {
            let status = response.status();
            let bytes = to_bytes(std::mem::replace(response.body_mut(), Body::empty()), 65536)
                .await
                .unwrap_or_default();
            let mut value=serde_json::from_slice::<serde_json::Value>(&bytes).ok().filter(|v|v.get("error").is_some()).unwrap_or_else(||serde_json::json!({"error":{"code":"request_failed","message":status.canonical_reason().unwrap_or("Request failed")}}));
            value["request_id"] = id.clone().into();
            *response.body_mut() =
                Body::from(serde_json::to_vec(&value).expect("error serialization"));
            response.headers_mut().remove(header::CONTENT_LENGTH);
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
        }
    }
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&id).expect("UUID header"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}
#[derive(Clone)]
pub struct RequestId(pub String);
pub(super) fn failure(status: StatusCode, code: &str, message: &str, id: &str) -> Response {
    let mut response = (
        status,
        Json(serde_json::json!({"error":{"code":code,"message":message},"request_id":id})),
    )
        .into_response();
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(id).expect("UUID header"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        infrastructure::{crypto::Crypto, db::Database},
        modules::users,
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn rejected_mutation_has_actor_result_trace_and_no_handler_when_audit_unavailable() {
        use std::os::unix::fs::DirBuilderExt;
        let directory =
            std::env::temp_dir().join(format!("hydra-attempt-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let auth = AuthState::new(
            Database::open(directory.join("db")).await.unwrap(),
            Crypto::load(&directory.join("key")).unwrap(),
            "https://hydra.test".into(),
        );
        let user = users::create(
            &auth.db,
            &auth.crypto,
            "operator".into(),
            "test-password-123".into(),
        )
        .await
        .unwrap();
        let token = auth
            .login(
                "operator".into(),
                "test-password-123".into(),
                "login".into(),
            )
            .await
            .unwrap()
            .response
            .access_token;
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let app = axum::Router::new()
            .route(
                "/api/v1/rejected",
                axum::routing::post(move || {
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        StatusCode::BAD_REQUEST
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                auth.clone(),
                middleware,
            ));
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/api/v1/rejected")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let response = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["request_id"], id);
        auth.db.run(move|db|{let n:i64=db.query_row("SELECT count(*) FROM audit WHERE actor_id=?1 AND request_id=?2 AND action LIKE 'http.mutation.%'",rusqlite::params![user,id],|r|r.get(0))?;assert_eq!(n,2);db.execute_batch("CREATE TRIGGER audit_unavailable BEFORE INSERT ON audit WHEN NEW.action LIKE 'http.%' BEGIN SELECT RAISE(ABORT,'test failure'); END;")?;Ok(())}).await.unwrap();
        let response = app.oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        drop(auth);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
