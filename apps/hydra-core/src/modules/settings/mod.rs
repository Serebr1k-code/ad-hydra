//! A redacted view of the actual operator configuration; credentials never leave Core.
use crate::{
    infrastructure::operations,
    modules::{
        audit,
        auth::{AuthState, Authenticated},
    },
};
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;
use utoipa::{OpenApi, ToSchema};
#[derive(Clone, Serialize, ToSchema)]
pub struct SettingsView {
    pub flag_pattern: Option<String>,
    pub automatic_submission: bool,
    pub checker_adapter: Option<String>,
    pub checker_batch_size: usize,
    pub sandbox_configured: bool,
    pub worker_concurrency: usize,
    pub supported_worker_networks: Vec<String>,
    pub access_lifetime_seconds: u32,
    pub refresh_absolute_lifetime_seconds: u32,
}
impl SettingsView {
    pub fn from_settings(settings: &operations::Settings) -> Self {
        let (kind, batch) = match settings.checker.as_ref() {
            Some(operations::Checker::Http { .. }) => (Some("http"), 1),
            Some(operations::Checker::Https { .. }) => (Some("https"), 1),
            Some(operations::Checker::Tcp { .. }) => (Some("tcp"), 1),
            Some(operations::Checker::CustomTcp { batch_size, .. }) => {
                (Some("custom_tcp"), *batch_size)
            }
            None => (None, 1),
        };
        Self {
            flag_pattern: settings.flag_pattern.clone(),
            automatic_submission: settings.auto_submit_captured,
            checker_adapter: kind.map(str::to_owned),
            checker_batch_size: batch,
            sandbox_configured: settings.sandbox.is_some(),
            worker_concurrency: 1,
            supported_worker_networks: vec!["IPv4 TCP".into(), "IPv4 UDP".into()],
            access_lifetime_seconds: 60,
            refresh_absolute_lifetime_seconds: 36000,
        }
    }
}
#[derive(Clone)]
pub struct Settings {
    pub auth: AuthState,
    pub view: SettingsView,
}
#[utoipa::path(get,path="/api/v1/system/settings",responses((status=200,body=SettingsView),(status=401)),tag="system")]
async fn settings(
    State(state): State<Settings>,
    Authenticated(actor): Authenticated,
) -> Result<Json<SettingsView>, StatusCode> {
    let request = audit::request_id();
    state
        .auth
        .db
        .run(move |db| {
            let tx = db.transaction()?;
            audit::append(
                &tx,
                &actor,
                "settings.read",
                "settings",
                &request,
                true,
                crate::modules::auth::now(),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(state.view))
}
// Authenticated's extractor uses AuthState, so authenticate through a small state adapter.
impl axum::extract::FromRef<Settings> for AuthState {
    fn from_ref(state: &Settings) -> Self {
        state.auth.clone()
    }
}
#[derive(OpenApi)]
#[openapi(paths(settings), components(schemas(SettingsView)))]
pub struct SettingsApi;
pub fn router(state: Settings) -> Router {
    Router::new()
        .route("/api/v1/system/settings", get(settings))
        .with_state(state)
}
