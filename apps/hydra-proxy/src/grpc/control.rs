use super::Rpc;
use crate::{
    state::{Provenance, now},
    telemetry::timestamp,
};
use hydra_domain::{RuleSnapshot, ServiceSnapshot};
use hydra_wire::v1::{self, proxy_control_server::ProxyControl};
use std::sync::atomic::Ordering;
use tonic::{Request, Response, Status};

#[tonic::async_trait]
impl ProxyControl for Rpc {
    async fn get_status(
        &self,
        _: Request<v1::GetStatusRequest>,
    ) -> Result<Response<v1::ProxyStatus>, Status> {
        let t = &self.state.telemetry;
        let service_metrics = t
            .budgets
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|b| b.metrics())
            .collect();
        let forwarding_stats = t
            .forwarding
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|store| v1::ForwardingStats {
                mappings: store.created.load(Ordering::Relaxed),
                reused: store.reused.load(Ordering::Relaxed),
                restored: store.restored.load(Ordering::Relaxed),
                errors: store.errors.load(Ordering::Relaxed),
            });
        let (request_checker_errors, last_request_checker_error) = t.request_checks.status();
        Ok(Response::new(v1::ProxyStatus {
            request_checker_errors,
            last_request_checker_error,
            service_metrics,
            forwarding_stats,
            proxy_instance_id: t.epoch.clone(),
            proxy_epoch_id: t.epoch.clone(),
            applied_services_revision: self.state.revision().await,
            applied_rules_revision: self.state.rules_revision().await,
            active_flows: t.active.load(Ordering::Relaxed) as u32,
            dropped_telemetry_events: t.dropped.load(Ordering::Relaxed),
            started_at: Some(t.started_at),
            rejected_flows: t.rejected.load(Ordering::Relaxed),
            dropped_udp_datagrams: t.udp_dropped.load(Ordering::Relaxed),
            critical_spool_bytes: t
                .critical
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map_or(0, |s| s.bytes()),
            critical_dropped: t
                .critical
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map_or(0, |s| s.dropped()),
            sample_dropped: t
                .samples
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map_or(0, |s| s.dropped()),
            history_dropped: t
                .history
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map_or(0, |store| store.dropped()),
            history_per_service_bytes: t
                .history
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map_or(0, |store| store.quota()),
            history_compression_level: t
                .history
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map_or(0, |store| store.compression_level()),
            rule_hit_counts: t
                .hit_counts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|(id, n)| (id.clone(), *n))
                .collect(),
        }))
    }
    async fn get_request_checker_settings(
        &self,
        request: Request<v1::GetRequestCheckerSettingsRequest>,
    ) -> Result<Response<v1::RequestCheckerSettings>, Status> {
        let request = request.into_inner();
        check_provenance(request.actor.as_ref(), &request.request_id)?;
        Ok(Response::new(checker_settings(
            self.state.telemetry.request_checks.settings(),
        )))
    }

    async fn set_request_checker_settings(
        &self,
        request: Request<v1::SetRequestCheckerSettingsRequest>,
    ) -> Result<Response<v1::RequestCheckerSettings>, Status> {
        use crate::request_checks::{Fallback, Policy, Settings, StaleRevision};
        let request = request.into_inner();
        check_provenance(request.actor.as_ref(), &request.request_id)?;
        let requested = request
            .settings
            .ok_or_else(|| Status::invalid_argument("missing checker settings"))?;
        if requested.policies.len() > 4 || requested.policies.iter().any(|p| p.services.len() > 128)
        {
            return Err(Status::invalid_argument(
                "checker settings exceed configured limits",
            ));
        }
        for policy in &requested.policies {
            for id in &policy.services {
                let service = self
                    .state
                    .get_service(id)
                    .await
                    .ok_or_else(|| Status::invalid_argument("unknown checker service"))?;
                if service.forwarding.is_some() {
                    return Err(Status::invalid_argument(
                        "request checkers require an ordinary upstream",
                    ));
                }
            }
        }
        let telemetry = self.state.telemetry.clone();
        let settings = Settings {
            revision: requested.revision,
            policies: requested
                .policies
                .into_iter()
                .map(|policy| Policy {
                    id: policy.id,
                    services: policy.services,
                    timeout_ms: policy.timeout_ms,
                    on_error: if policy.allow_on_error {
                        Fallback::Allow
                    } else {
                        Fallback::Block
                    },
                })
                .collect(),
        };
        let applied =
            tokio::task::spawn_blocking(move || telemetry.request_checks.configure(settings))
                .await
                .map_err(|_| Status::internal("checker configuration task failed"))?
                .map_err(|error| {
                    if error.is::<StaleRevision>() {
                        Status::aborted(error.to_string())
                    } else {
                        Status::invalid_argument(error.to_string())
                    }
                })?;
        Ok(Response::new(checker_settings(applied)))
    }

    async fn configure_history(
        &self,
        request: Request<v1::ConfigureHistoryRequest>,
    ) -> Result<Response<v1::ConfigureHistoryReply>, Status> {
        let request = request.into_inner();
        check_provenance(request.actor.as_ref(), &request.request_id)?;
        use hydra_domain::history::{MAX_PER_SERVICE_MIB, MIB, MIN_PER_SERVICE_MIB};
        if !(hydra_compression::MIN_LEVEL..=hydra_compression::MAX_LEVEL)
            .contains(&request.compression_level)
        {
            return Err(Status::invalid_argument("compression level must be 1..=19"));
        }
        if !(u64::from(MIN_PER_SERVICE_MIB) * MIB..=u64::from(MAX_PER_SERVICE_MIB) * MIB)
            .contains(&request.per_service_bytes)
        {
            return Err(Status::invalid_argument(
                "history quota must be 1..1024 MiB per service",
            ));
        }

        let store = self
            .state
            .telemetry
            .history
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| Status::unavailable("history unavailable"))?;
        store
            .set_policy(request.per_service_bytes, request.compression_level)
            .await
            .map_err(|_| Status::unavailable("history policy could not be applied"))?;
        let actor = request.actor.as_ref().expect("validated actor");
        let actor_id = if actor.user_id.is_empty() {
            &actor.username
        } else {
            &actor.user_id
        };
        tracing::info!(request_id=%request.request_id,actor_id,actor_type=actor.r#type,per_service_bytes=request.per_service_bytes,"traffic history policy applied");
        Ok(Response::new(v1::ConfigureHistoryReply {
            per_service_bytes: store.quota(),
            compression_level: store.compression_level(),
        }))
    }
    async fn apply_config_snapshot(
        &self,
        request: Request<v1::ApplyConfigSnapshotRequest>,
    ) -> Result<Response<v1::ApplyResult>, Status> {
        let r = request.into_inner();
        let services = r
            .services
            .ok_or_else(|| Status::invalid_argument("services required"))?;
        let rules = r
            .rules
            .ok_or_else(|| Status::invalid_argument("rules required"))?;
        check_provenance(services.actor.as_ref(), &services.request_id)?;
        check_provenance(rules.actor.as_ref(), &rules.request_id)?;
        if services.request_id != rules.request_id || services.actor != rules.actor {
            return Err(Status::invalid_argument("combined provenance must agree"));
        }
        let actor = services.actor.as_ref().expect("checked");
        let provenance = Provenance {
            actor_type: actor.r#type.to_string(),
            actor_id: if actor.user_id.is_empty() {
                actor.username.clone()
            } else {
                actor.user_id.clone()
            },
            request_id: services.request_id.clone(),
            applied_at_unix: now(),
        };
        let snapshot = ServiceSnapshot::try_from(&services).map_err(Status::invalid_argument)?;
        let rules = RuleSnapshot::try_from(&rules).map_err(Status::invalid_argument)?;
        let result = match self.state.apply_config(snapshot, rules, provenance).await {
            Ok(revision) => v1::ApplyResult {
                applied_revision: revision,
                applied: true,
                applied_at: Some(timestamp()),
                ..Default::default()
            },
            Err(e) => v1::ApplyResult {
                applied_revision: self.state.revision().await,
                error_code: "snapshot_rejected".into(),
                error_message: e.to_string(),
                ..Default::default()
            },
        };
        Ok(Response::new(result))
    }
    async fn apply_service_snapshot(
        &self,
        request: Request<v1::ApplyServiceSnapshotRequest>,
    ) -> Result<Response<v1::ApplyResult>, Status> {
        let r = request.into_inner();
        let actor = r
            .actor
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("actor required"))?;
        if r.request_id.is_empty()
            || r.request_id.len() > 128
            || actor.username.len() > 128
            || actor.user_id.len() > 128
            || !(1..=4).contains(&actor.r#type)
        {
            return Err(Status::invalid_argument("invalid actor or request ID"));
        }
        let snapshot = ServiceSnapshot::try_from(&r).map_err(Status::invalid_argument)?;
        let provenance = Provenance {
            actor_type: actor.r#type.to_string(),
            actor_id: if actor.user_id.is_empty() {
                actor.username.clone()
            } else {
                actor.user_id.clone()
            },
            request_id: r.request_id.clone(),
            applied_at_unix: now(),
        };
        let applied = self.state.apply(snapshot, provenance).await;
        let result = match applied {
            Ok(revision) => {
                tracing::info!(revision, request_id = %r.request_id, actor_type = actor.r#type, "service snapshot applied");
                v1::ApplyResult {
                    applied_revision: revision,
                    applied: true,
                    applied_at: Some(timestamp()),
                    ..Default::default()
                }
            }
            Err(error) => v1::ApplyResult {
                applied_revision: self.state.revision().await,
                applied: false,
                error_code: "snapshot_rejected".into(),
                error_message: error.to_string(),
                applied_at: None,
            },
        };
        Ok(Response::new(result))
    }
    async fn apply_rule_snapshot(
        &self,
        request: Request<v1::ApplyRuleSnapshotRequest>,
    ) -> Result<Response<v1::ApplyResult>, Status> {
        let r = request.into_inner();
        let actor = r
            .actor
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("actor required"))?;
        if r.request_id.is_empty()
            || r.request_id.len() > 128
            || !(1..=4).contains(&actor.r#type)
            || actor.user_id.len() > 128
            || actor.username.len() > 128
        {
            return Err(Status::invalid_argument("invalid provenance"));
        }
        let rules = RuleSnapshot::try_from(&r).map_err(Status::invalid_argument)?;
        let provenance = Provenance {
            actor_type: actor.r#type.to_string(),
            actor_id: if actor.user_id.is_empty() {
                actor.username.clone()
            } else {
                actor.user_id.clone()
            },
            request_id: r.request_id,
            applied_at_unix: now(),
        };
        let result = match self.state.apply_rules(rules, provenance).await {
            Ok(revision) => v1::ApplyResult {
                applied_revision: revision,
                applied: true,
                applied_at: Some(timestamp()),
                ..Default::default()
            },
            Err(error) => v1::ApplyResult {
                applied_revision: self.state.rules_revision().await,
                error_code: "snapshot_rejected".into(),
                error_message: error.to_string(),
                ..Default::default()
            },
        };
        Ok(Response::new(result))
    }
    async fn install_protocol(
        &self,
        request: Request<v1::InstallProtocolRequest>,
    ) -> Result<Response<v1::InstallProtocolResult>, Status> {
        let r = request.into_inner();
        check_provenance(r.actor.as_ref(), &r.request_id)?;
        let installed = if r.framing_profile_json.is_empty() {
            self.protocols
                .install(&r.protocol_id, &r.version, &r.sha256_hex, r.wasm)
                .await
        } else if !r.wasm.is_empty() {
            Err(anyhow::anyhow!(
                "WASM and framing profile are mutually exclusive"
            ))
        } else {
            self.protocols
                .install_profile(
                    &r.protocol_id,
                    &r.version,
                    &r.framing_profile_json,
                    &r.sha256_hex,
                )
                .await
        };
        let metadata_json = if installed.is_ok() {
            self.protocols
                .metadata(&r.protocol_id, &r.version)
                .unwrap_or_default()
        } else {
            String::new()
        };
        let result = v1::InstallProtocolResult {
            metadata_json,
            protocol_id: r.protocol_id,
            version: r.version,
            sha256_hex: r.sha256_hex,
            installed: installed.is_ok(),
            error_message: installed.err().map(|e| e.to_string()).unwrap_or_default(),
        };
        Ok(Response::new(result))
    }
    async fn deactivate_protocol(
        &self,
        request: Request<v1::DeactivateProtocolRequest>,
    ) -> Result<Response<v1::ApplyResult>, Status> {
        let r = request.into_inner();
        check_provenance(r.actor.as_ref(), &r.request_id)?;
        let result = match self.protocols.deactivate(&r.service_id).await {
            Ok(()) => v1::ApplyResult {
                applied: true,
                applied_at: Some(timestamp()),
                ..Default::default()
            },
            Err(e) => v1::ApplyResult {
                error_code: "deactivation_rejected".into(),
                error_message: e.to_string(),
                ..Default::default()
            },
        };
        Ok(Response::new(result))
    }
    async fn activate_protocol(
        &self,
        request: Request<v1::ActivateProtocolRequest>,
    ) -> Result<Response<v1::ApplyResult>, Status> {
        let r = request.into_inner();
        check_provenance(r.actor.as_ref(), &r.request_id)?;
        if !self.state.has_service(&r.service_id).await {
            return Err(Status::not_found("service not configured"));
        }
        let result = match self
            .protocols
            .activate(&r.service_id, &r.protocol_id, &r.version)
            .await
        {
            Ok(()) => v1::ApplyResult {
                applied: true,
                applied_at: Some(timestamp()),
                ..Default::default()
            },
            Err(e) => v1::ApplyResult {
                error_code: "activation_rejected".into(),
                error_message: e.to_string(),
                ..Default::default()
            },
        };
        Ok(Response::new(result))
    }
}
fn checker_settings(settings: crate::request_checks::Settings) -> v1::RequestCheckerSettings {
    v1::RequestCheckerSettings {
        revision: settings.revision,
        policies: settings
            .policies
            .into_iter()
            .map(|policy| v1::RequestCheckerPolicy {
                id: policy.id,
                services: policy.services,
                timeout_ms: policy.timeout_ms,
                allow_on_error: matches!(policy.on_error, crate::request_checks::Fallback::Allow),
            })
            .collect(),
    }
}

#[allow(clippy::result_large_err)] // tonic trait boundary uses Status by value.
pub(super) fn check_provenance(actor: Option<&v1::Actor>, request: &str) -> Result<(), Status> {
    let actor = actor.ok_or_else(|| Status::invalid_argument("actor required"))?;
    if request.is_empty()
        || request.len() > 128
        || !(1..=4).contains(&actor.r#type)
        || actor.user_id.len() > 128
        || actor.username.len() > 128
    {
        return Err(Status::invalid_argument("invalid provenance"));
    }
    Ok(())
}
