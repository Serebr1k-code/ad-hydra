use super::{Conflict, Control};
use crate::modules::audit::Actor;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use hydra_domain::{Service, Transport};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use utoipa::ToSchema;
#[derive(Debug)]
pub enum Error {
    Auth,
    Missing,
    Status(StatusCode),
    Checker(StatusCode, String),
    Invalid(anyhow::Error),
}
impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Self::Invalid(e)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Auth => (
                StatusCode::UNAUTHORIZED,
                "authentication_failed",
                "Authentication failed".into(),
            ),
            Self::Missing => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Record not found".into(),
            ),
            Self::Checker(status, message) => (status, "checker_settings", message),
            Self::Status(status) => (
                status,
                if status == StatusCode::SERVICE_UNAVAILABLE {
                    "proxy_unavailable"
                } else {
                    "preview_failed"
                },
                "Filter preview could not be completed".into(),
            ),
            Self::Invalid(e) => {
                if e.downcast_ref::<Conflict>().is_some() {
                    (StatusCode::CONFLICT, "revision_conflict", e.to_string())
                } else {
                    (StatusCode::BAD_REQUEST, "invalid_request", e.to_string())
                }
            }
        };
        (
            status,
            Json(serde_json::json!({"error":{"code":code,"message":message}})),
        )
            .into_response()
    }
}
pub(crate) async fn actor(control: &Control, headers: &HeaderMap) -> Result<Actor, Error> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(Error::Auth)?;
    control
        .auth
        .authenticate(token)
        .await
        .map_err(|_| Error::Auth)
}
pub(crate) fn request() -> String {
    crate::modules::audit::request_id()
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceInput {
    /// Optional display name; omitted on update preserves the current name.
    pub name: Option<String>,
    pub forwarding: Option<super::forwarding::ForwardingInput>,
    pub expected_revision: u64,
    #[schema(value_type=String)]
    pub transport: Transport,
    #[schema(value_type=String)]
    pub listen_address: SocketAddr,
    #[schema(value_type=String)]
    pub upstream_address: SocketAddr,
    pub protocol_id: String,
    pub enabled: bool,
    #[serde(default = "history_enabled")]
    pub retain_history: bool,
    pub protection: Option<ProtectionInput>,
    /// TLS termination and/or upstream TLS. Omit to keep the service plaintext.
    pub tls: Option<TlsInput>,
}
/// The private key reaches the Proxy over the local socket only. Every response
/// that carries a service replaces it with `private_key_configured`.
pub(crate) fn redact_tls_secrets(value: &mut serde_json::Value) {
    fn redact(service: &mut serde_json::Value) {
        if let Some(tls) = service.get_mut("tls").and_then(|tls| tls.as_object_mut()) {
            let configured = tls
                .remove("private_key_pem")
                .is_some_and(|key| !key.is_null());
            tls.insert("private_key_configured".into(), configured.into());
        }
    }
    fn walk(node: &mut serde_json::Value) {
        let has_tls =
            matches!(node, serde_json::Value::Object(object) if object.contains_key("tls"));
        if has_tls {
            redact(node);
        }
        match node {
            serde_json::Value::Array(items) => items.iter_mut().for_each(walk),
            serde_json::Value::Object(object) => {
                for child in object.values_mut() {
                    walk(child);
                }
            }
            _ => {}
        }
    }
    walk(value)
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TlsInput {
    pub terminate: bool,
    #[serde(default)]
    pub upstream: bool,
    #[serde(default)]
    pub insecure: bool,
    /// PEM chain the Proxy presents. Required when `terminate` is set.
    pub certificate_pem: Option<String>,
    /// PEM private key. Omit on update to keep the stored key.
    pub private_key_pem: Option<String>,
    pub server_name: Option<String>,
}
impl TlsInput {
    /// An update may omit the key: the stored one stays in place.
    fn into_tls(self, previous: Option<&hydra_domain::ServiceTls>) -> hydra_domain::ServiceTls {
        let private_key_pem = self
            .private_key_pem
            .filter(|value| !value.trim().is_empty())
            .or_else(|| previous.and_then(|p| p.private_key_pem.clone()));
        hydra_domain::ServiceTls {
            terminate: self.terminate,
            upstream: self.upstream,
            insecure: self.insecure,
            certificate_pem: self
                .certificate_pem
                .filter(|value| !value.trim().is_empty())
                .or_else(|| previous.and_then(|p| p.certificate_pem.clone())),
            private_key_pem,
            server_name: self.server_name.filter(|value| !value.trim().is_empty()),
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProtectionInput {
    #[schema(minimum = 1)]
    pub max_active_flows: u64,
    pub tcp_connections_per_second: u64,
    pub udp_datagrams_per_second: u64,
    pub bytes_per_second: u64,
    #[schema(minimum = 1)]
    pub idle_timeout_ms: u64,
}
fn history_enabled() -> bool {
    true
}
fn validate_termination(tls: &hydra_domain::ServiceTls) -> Result<(), String> {
    tls.validate()?;
    if !tls.terminate {
        return Ok(());
    }
    let certificate = tls.certificate_pem.as_deref().unwrap_or_default();
    let key = tls.private_key_pem.as_deref().unwrap_or_default();
    let certificates: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut std::io::Cursor::new(certificate))
            .collect::<Result<_, _>>()
            .map_err(|_| "invalid TLS certificate PEM".to_owned())?;
    let private_key = rustls_pemfile::private_key(&mut std::io::Cursor::new(key))
        .map_err(|_| "invalid TLS private-key PEM".to_owned())?
        .ok_or_else(|| "TLS private-key PEM contains no private key".to_owned())?;
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| "unsupported TLS protocol configuration".to_owned())?
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .map_err(|_| "TLS certificate and private key do not match".to_owned())?;
    Ok(())
}

fn redact_tls_service(service: &mut serde_json::Value) {
    if let Some(tls) = service.get_mut("tls").and_then(|v| v.as_object_mut()) {
        let configured = tls
            .remove("private_key_pem")
            .is_some_and(|key| !key.is_null());
        tls.insert("private_key_configured".into(), configured.into());
    }
}

fn redact_tls_private_keys(snapshot: &mut serde_json::Value) {
    if let Some(services) = snapshot.get_mut("services").and_then(|v| v.as_array_mut()) {
        for service in services {
            redact_tls_service(service);
        }
    }
}

/// Convert an internal desired snapshot to its public HTTP representation.
/// TLS private keys remain in Core storage and the local Core-to-Proxy contract only.
pub(crate) fn public_desired(desired: super::Desired) -> serde_json::Value {
    let mut value = serde_json::to_value(desired).expect("desired snapshot is serializable");
    if let Some(services) = value.get_mut("services") {
        redact_tls_private_keys(services);
    }
    value
}

impl ServiceInput {
    async fn into_service(self, id: String, control: &Control) -> Result<Service, Error> {
        self.into_service_with_previous(id, control, None).await
    }
    async fn into_service_with_previous(
        self,
        id: String,
        control: &Control,
        previous: Option<&Service>,
    ) -> Result<Service, Error> {
        let forwarding = match self.forwarding {
            Some(f) => Some(f.resolve(control).await.map_err(Error::from)?),
            None => None,
        };
        let tls = self
            .tls
            .map(|tls| tls.into_tls(previous.and_then(|p| p.tls.as_ref())));
        if let Some(tls) = &tls {
            validate_termination(tls).map_err(|error| Error::Invalid(anyhow::anyhow!(error)))?;
        }
        Ok(Service {
            forwarding,
            tls,
            id,
            transport: self.transport,
            listen_address: self.listen_address,
            upstream_address: self.upstream_address,
            protocol_id: self.protocol_id,
            enabled: self.enabled,
            retain_history: self.retain_history,
            protection: self.protection.map(|p| hydra_domain::ServiceProtection {
                max_active_flows: p.max_active_flows,
                tcp_connections_per_second: p.tcp_connections_per_second,
                udp_datagrams_per_second: p.udp_datagrams_per_second,
                bytes_per_second: p.bytes_per_second,
                idle_timeout_ms: p.idle_timeout_ms,
            }),
        })
    }
}
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    #[serde(default)]
    pub after: String,
    pub q: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Deserialize)]
pub struct Revision {
    expected_revision: u64,
}
#[derive(Serialize, ToSchema)]
pub struct ListResponse {
    pub revision: u64,
    pub applied_revision: u64,
    pub apply_status: String,
    pub apply_error: Option<String>,
    #[schema(value_type=Vec<Object>)]
    pub items: Vec<serde_json::Value>,
    pub next_cursor: Option<String>,
}
pub(crate) async fn list(
    control: &Control,
    actor: Actor,
    query: ListQuery,
    kind: &str,
) -> anyhow::Result<ListResponse> {
    let desired = control.desired().await?;
    let records = control.attribution().await?;
    let names = if kind == "service" {
        control.service_names().await?
    } else {
        Default::default()
    };
    let colors = if kind == "service" {
        control.service_colors().await?
    } else {
        Default::default()
    };
    let mut items = Vec::new();
    let revision = desired.services.revision;
    let applied = if kind == "service" {
        desired.services_applied_revision
    } else {
        desired.rules_applied_revision
    };
    let q = query.q.unwrap_or_default();
    anyhow::ensure!(q.len() <= 256 && query.after.len() <= 128, "query too long");
    let values: Vec<serde_json::Value> = if kind == "service" {
        desired
            .services
            .services
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?
    } else {
        desired
            .rules
            .rules
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?
    };
    for mut value in values {
        let id = value["id"].as_str().unwrap_or_default().to_owned();
        if kind == "service" {
            value["color"] = colors.get(&id).cloned().into();
            value["name"] = names.get(&id).cloned().unwrap_or_else(|| id.clone()).into();
            redact_tls_secrets(&mut value);
        }
        if id <= query.after || (!q.is_empty() && !value.to_string().contains(&q)) {
            continue;
        }
        if let Some(record) = records.iter().find(|r| r.id == id && r.kind == kind) {
            let object = value
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("invalid stored record"))?;
            let attribution = serde_json::to_value(record)?;
            for field in [
                "created_by",
                "updated_by",
                "created_at",
                "updated_at",
                "hit_count",
            ] {
                object.insert(field.into(), attribution[field].clone());
            }
            object.insert("desired_revision".into(), revision.into());
            object.insert("applied_revision".into(), applied.into());
            object.insert("desired_enabled".into(), object["enabled"].clone());
            object.insert(
                "applied_enabled".into(),
                if applied == revision {
                    object["enabled"].clone()
                } else {
                    serde_json::Value::Null
                },
            );
        }
        items.push(value)
    }
    items.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let limit = query.limit.unwrap_or(100).clamp(1, 100);
    let next_cursor = if items.len() > limit {
        items[limit - 1]["id"].as_str().map(str::to_owned)
    } else {
        None
    };
    items.truncate(limit);
    control
        .db
        .run(move |db| {
            let tx = db.transaction()?;
            crate::modules::audit::append(
                &tx,
                &actor,
                "control.list",
                "configuration",
                &request(),
                true,
                crate::modules::auth::now(),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(ListResponse {
        revision,
        applied_revision: applied,
        apply_status: if desired.apply_error.is_some() {
            "error"
        } else if applied == revision {
            "applied"
        } else {
            "pending"
        }
        .into(),
        apply_error: desired.apply_error,
        items,
        next_cursor,
    })
}
#[utoipa::path(get,path="/api/v1/services",params(ListQuery),responses((status=200,body=ListResponse),(status=401)),tag="services")]
pub async fn services(
    State(c): State<Control>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResponse>, Error> {
    let a = actor(&c, &headers).await?;
    list(&c, a, q, "service")
        .await
        .map(Json)
        .map_err(Error::from)
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceColorInput {
    /// Hex #RRGGBB, or null to restore the default palette color.
    color: Option<String>,
}
#[utoipa::path(put,path="/api/v1/services/{id}/color",params(("id"=String,Path)),request_body=ServiceColorInput,responses((status=200,body=ServiceColorInput),(status=400),(status=401)),tag="services")]
pub async fn service_color(
    State(c): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ServiceColorInput>,
) -> Result<Json<ServiceColorInput>, Error> {
    let actor = actor(&c, &headers).await?;
    c.set_service_color(actor, id, input.color.clone(), request())
        .await
        .map_err(Error::from)?;
    Ok(Json(input))
}
#[derive(Serialize)]
struct CreatedServiceResponse {
    id: String,
    #[serde(flatten)]
    desired: super::Desired,
}
#[utoipa::path(post,path="/api/v1/services",request_body=ServiceInput,responses((status=202,description="Created service snapshot; id identifies the newly created service",body=Object),(status=409)),tag="services")]
pub async fn create_service(
    State(c): State<Control>,
    headers: HeaderMap,
    Json(input): Json<ServiceInput>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    let revision = input.expected_revision;
    let name = input.name.clone();
    let service = input
        .into_service(uuid::Uuid::new_v4().to_string(), &c)
        .await?;
    let id = service.id.clone();
    c.put_named_service(a, revision, service, true, name, request())
        .await
        .map(|desired| {
            let mut value =
                serde_json::to_value(CreatedServiceResponse { id, desired }).unwrap_or_default();
            redact_tls_secrets(&mut value);
            (StatusCode::ACCEPTED, Json(value)).into_response()
        })
        .map_err(Error::from)
}
#[utoipa::path(patch,path="/api/v1/services/{id}",params(("id"=String,Path)),request_body=ServiceInput,responses((status=202,body=Object),(status=409)),tag="services")]
pub async fn update_service(
    State(c): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ServiceInput>,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    let revision = input.expected_revision;
    let name = input.name.clone();
    let previous = c
        .desired()
        .await
        .ok()
        .and_then(|desired| desired.services.services.into_iter().find(|s| s.id == id));
    let service = input
        .into_service_with_previous(id, &c, previous.as_ref())
        .await?;
    c.put_named_service(a, revision, service, false, name, request())
        .await
        .map(|desired| {
            let mut value = serde_json::to_value(desired).unwrap_or_default();
            redact_tls_secrets(&mut value);
            (StatusCode::ACCEPTED, Json(value)).into_response()
        })
        .map_err(Error::from)
}
pub(crate) async fn remove(
    c: Control,
    headers: HeaderMap,
    id: String,
    q: Revision,
    kind: &'static str,
) -> Result<Response, Error> {
    let a = actor(&c, &headers).await?;
    c.delete(a, q.expected_revision, kind, id, request())
        .await
        .map(|v| (StatusCode::ACCEPTED, Json(public_desired(v))).into_response())
        .map_err(Error::from)
}
#[utoipa::path(delete,path="/api/v1/services/{id}",params(("id"=String,Path),("expected_revision"=u64,Query)),responses((status=202,body=Object),(status=409)),tag="services")]
pub async fn delete_service(
    State(c): State<Control>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Revision>,
) -> Result<Response, Error> {
    remove(c, headers, id, q, "service").await
}
#[utoipa::path(get,path="/api/v1/config/export",responses((status=200,body=Object),(status=401)),tag="system")]
async fn export_configuration(
    State(c): State<Control>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    let a = actor(&c, &headers).await?;
    let desired = c.desired().await.map_err(Error::from)?;
    let revision = desired.services.revision;
    let request_id = request();
    c.db.run(move |db| {
        let tx = db.transaction()?;
        crate::modules::audit::append_change(
            &tx,
            &a,
            "configuration.export",
            "configuration",
            &request_id,
            true,
            crate::modules::auth::now(),
            &serde_json::json!({"revision":revision,"source":"http"}),
        )?;
        tx.commit()?;
        Ok(())
    })
    .await
    .map_err(Error::from)?;
    let mut snapshot =
        serde_json::json!({"schema_version":1,"services":desired.services,"filters":desired.rules});
    redact_tls_secrets(&mut snapshot);
    Ok(Json(snapshot))
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        services,
        create_service,
        service_color,
        update_service,
        delete_service,
        export_configuration
    ),
    components(schemas(ServiceInput, ServiceColorInput, ListResponse))
)]
struct ServiceApi;
pub struct ControlApi;
impl utoipa::OpenApi for ControlApi {
    fn openapi() -> utoipa::openapi::OpenApi {
        let mut doc = ServiceApi::openapi();
        doc.merge(crate::modules::filters::FilterApi::openapi());
        doc.merge(super::retry::RetryApi::openapi());
        doc
    }
}
pub fn router(control: Control) -> Router {
    Router::new()
        .route("/api/v1/config/export", get(export_configuration))
        .route("/api/v1/services", get(services).post(create_service))
        .route(
            "/api/v1/services/{id}/color",
            axum::routing::put(service_color),
        )
        .route(
            "/api/v1/services/{id}",
            axum::routing::patch(update_service).delete(delete_service),
        )
        .with_state(control.clone())
        .merge(super::retry::router(control.clone()))
        .merge(crate::modules::filters::router(control))
}

#[cfg(test)]
mod history_input_tests {
    use super::*;

    #[tokio::test]
    async fn service_api_input_defaults_and_preserves_history_retention() {
        let (control, path) = crate::modules::services::tests::setup().await;
        let defaults: ServiceInput = serde_json::from_value(serde_json::json!({
            "expected_revision": 0,
            "transport": "tcp",
            "listen_address": "127.0.0.1:21001",
            "upstream_address": "127.0.0.1:21002",
            "protocol_id": "raw",
            "enabled": true
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(defaults.into_service("svc".into(), &control).await.unwrap())
                .unwrap()["retain_history"],
            true
        );

        let disabled: ServiceInput = serde_json::from_value(serde_json::json!({
            "expected_revision": 0,
            "transport": "tcp",
            "listen_address": "127.0.0.1:21001",
            "upstream_address": "127.0.0.1:21002",
            "protocol_id": "raw",
            "enabled": true,
            "retain_history": false
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(disabled.into_service("svc".into(), &control).await.unwrap())
                .unwrap()["retain_history"],
            false
        );
        drop(control);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn public_desired_redacts_private_keys_and_preserves_configured_state() {
        let desired = super::super::Desired {
            services: hydra_domain::ServiceSnapshot {
                revision: 3,
                services: vec![Service {
                    forwarding: None,
                    tls: Some(hydra_domain::ServiceTls {
                        terminate: true,
                        upstream: false,
                        insecure: false,
                        certificate_pem: Some("certificate".into()),
                        private_key_pem: Some("highly-sensitive-key".into()),
                        server_name: None,
                    }),
                    id: "svc".into(),
                    transport: Transport::Tcp,
                    listen_address: "127.0.0.1:24001".parse().unwrap(),
                    upstream_address: "127.0.0.1:24002".parse().unwrap(),
                    protocol_id: "raw".into(),
                    enabled: true,
                    retain_history: true,
                    protection: None,
                }],
            },
            rules: hydra_domain::RuleSnapshot::default(),
            applied_revision: 3,
            services_applied_revision: 3,
            rules_applied_revision: 3,
            apply_error: None,
        };
        let public = public_desired(desired);
        assert_eq!(
            public["services"]["services"][0]["tls"]["private_key_configured"],
            true
        );
        assert!(
            public["services"]["services"][0]["tls"]
                .get("private_key_pem")
                .is_none()
        );
        assert!(!public.to_string().contains("highly-sensitive-key"));
    }

    #[test]
    fn invalid_tls_certificate_is_rejected_before_service_submission() {
        let malformed = hydra_domain::ServiceTls {
            terminate: true,
            certificate_pem: Some("-----BEGIN CERTIFICATE-----\ninvalid".into()),
            private_key_pem: Some("-----BEGIN PRIVATE KEY-----\ninvalid".into()),
            ..Default::default()
        };
        assert_eq!(
            validate_termination(&malformed).unwrap_err(),
            "invalid TLS certificate PEM"
        );
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    #[test]
    fn tls_private_keys_never_leave_the_api() {
        let mut snapshot = serde_json::json!({
            "revision": 3,
            "services": [
                {
                    "id": "web",
                    "tls": {
                        "terminate": true,
                        "certificate_pem": "-----BEGIN CERTIFICATE-----x",
                        "private_key_pem": "-----BEGIN PRIVATE KEY-----secret"
                    }
                },
                { "id": "plain" }
            ],
            "rules": []
        });
        redact_tls_secrets(&mut snapshot);
        let text = snapshot.to_string();
        assert!(!text.contains("private_key_pem"), "{text}");
        assert!(!text.contains("secret"), "{text}");
        assert_eq!(
            snapshot["services"][0]["tls"]["private_key_configured"],
            serde_json::json!(true)
        );
        assert!(snapshot["services"][0]["tls"]["certificate_pem"].is_string());
        assert!(snapshot["services"][1].get("tls").is_none());
    }

    #[test]
    fn redaction_also_covers_nested_payloads() {
        let mut created = serde_json::json!({
            "id": "abc",
            "desired": { "services": [{ "id": "web", "tls": { "private_key_pem": "secret" } }] }
        });
        redact_tls_secrets(&mut created);
        assert_eq!(
            created["desired"]["services"][0]["tls"]["private_key_configured"],
            serde_json::json!(true)
        );
        assert!(!created.to_string().contains("secret"));
    }
}
