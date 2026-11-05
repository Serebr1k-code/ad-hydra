//! Pure, framework-independent service configuration shared by both applications.
mod forwarding;
pub mod history;
pub use forwarding::{ForwardFraming, Forwarding};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::SocketAddr};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Tcp,
    Udp,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Service {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarding: Option<Forwarding>,
    pub id: String,
    pub transport: Transport,
    pub listen_address: SocketAddr,
    pub upstream_address: SocketAddr,
    #[serde(default = "raw")]
    pub protocol_id: String,
    pub enabled: bool,
    #[serde(default = "history_enabled")]
    pub retain_history: bool,
    #[serde(default)]
    pub protection: Option<ServiceProtection>,
}
/// Aggregate service budgets; zero disables only a rate.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceProtection {
    pub max_active_flows: u64,
    pub tcp_connections_per_second: u64,
    pub udp_datagrams_per_second: u64,
    pub bytes_per_second: u64,
    pub idle_timeout_ms: u64,
}
fn raw() -> String {
    "raw".into()
}
fn history_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceSnapshot {
    pub revision: u64,
    pub services: Vec<Service>,
}

impl Service {
    pub fn effective_upstream(&self) -> SocketAddr {
        self.forwarding
            .as_ref()
            .map_or(self.upstream_address, |f| f.target_address)
    }
}

impl ServiceSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.services.len() > 128 {
            return Err("at most 128 services are allowed".into());
        }
        let mut ids = HashSet::new();
        let mut endpoints = HashSet::new();
        for s in &self.services {
            if let Some(f) = &s.forwarding {
                if f.team_id.is_empty()
                    || f.team_id.len() > 128
                    || f.flag_pattern.is_empty()
                    || f.flag_pattern.len() > 4096
                    || f.target_address.port() == 0
                    || f.target_address.ip().is_unspecified()
                    || f.target_address.ip().is_multicast()
                    || f.target_address == s.listen_address
                {
                    return Err("invalid forwarding configuration".into());
                }
                if (s.transport == Transport::Udp) != matches!(f.framing, ForwardFraming::Datagram)
                {
                    return Err(
                        "UDP requires datagram forwarding; TCP requires message framing".into(),
                    );
                }
            }
            if let Some(p) = &s.protection
                && (p.max_active_flows == 0 || p.idle_timeout_ms == 0)
            {
                return Err(
                    "protection active-flow limit and idle timeout must be positive".into(),
                );
            }
            if s.id.is_empty() || s.id.len() > 128 || !ids.insert(&s.id) {
                return Err("service IDs must be unique, nonempty, and at most 128 bytes".into());
            }
            if !matches!(s.protocol_id.as_str(), "raw" | "utf8" | "http") {
                return Err("only raw, utf8, and http profiles are built in".into());
            }
            if s.protocol_id == "http" && s.transport != Transport::Tcp {
                return Err("HTTP profile requires TCP".into());
            }
            if s.listen_address.port() == 0
                || s.upstream_address.port() == 0
                || s.upstream_address.ip().is_unspecified()
                || s.upstream_address.ip().is_multicast()
                || s.listen_address == s.upstream_address
            {
                return Err("invalid or self-referential service endpoint".into());
            }
            if s.enabled && !endpoints.insert((s.transport, s.listen_address)) {
                return Err("duplicate listener".into());
            }
        }
        // Reject known direct relay loops. A wildcard listener is local, not every remote host.
        for s in self.services.iter().filter(|s| s.enabled) {
            if self.services.iter().any(|other| {
                other.enabled
                    && other.transport == s.transport
                    && other.listen_address.port() == s.effective_upstream().port()
                    && (other.listen_address.ip() == s.effective_upstream().ip()
                        || (other.listen_address.ip().is_unspecified()
                            && s.effective_upstream().ip().is_loopback()))
            }) {
                return Err("upstream points to a Hydra listener".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn service() -> Service {
        Service {
            forwarding: None,
            id: "one".into(),
            transport: Transport::Tcp,
            listen_address: "127.0.0.1:2001".parse().unwrap(),
            upstream_address: "127.0.0.1:2002".parse().unwrap(),
            protocol_id: "raw".into(),
            enabled: true,
            retain_history: true,
            protection: None,
        }
    }
    #[test]
    fn rejects_duplicate_ids_listeners_and_loops() {
        let s = service();
        let mut snapshot = ServiceSnapshot {
            revision: 1,
            services: vec![s.clone()],
        };
        assert!(snapshot.validate().is_ok());
        snapshot.services.push(s);
        assert!(snapshot.validate().is_err());
        snapshot.services[1].id = "two".into();
        assert!(snapshot.validate().is_err());
        snapshot.services[1].listen_address = "127.0.0.1:2002".parse().unwrap();
        assert!(snapshot.validate().is_err());
    }
    #[test]
    fn wildcard_listener_can_use_remote_same_port_upstream() {
        let mut s = service();
        s.listen_address = "0.0.0.0:2001".parse().unwrap();
        s.upstream_address = "203.0.113.10:2001".parse().unwrap();
        assert!(
            ServiceSnapshot {
                revision: 1,
                services: vec![s]
            }
            .validate()
            .is_ok()
        );
    }
    #[test]
    fn utf8_is_a_builtin_profile_on_tcp_and_udp() {
        let mut utf8_service = service();
        utf8_service.protocol_id = "utf8".into();
        let tcp = ServiceSnapshot {
            revision: 1,
            services: vec![utf8_service.clone()],
        };
        assert!(tcp.validate().is_ok());
        utf8_service.transport = Transport::Udp;
        assert!(
            ServiceSnapshot {
                revision: 1,
                services: vec![utf8_service]
            }
            .validate()
            .is_ok()
        );
        let mut unknown = service();
        unknown.protocol_id = "unknown-profile".into();
        assert!(
            ServiceSnapshot {
                revision: 1,
                services: vec![unknown]
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn legacy_service_json_defaults_history_retention_to_true() {
        let service: Service = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "transport": "tcp",
            "listen_address": "127.0.0.1:2001",
            "upstream_address": "127.0.0.1:2002",
            "protocol_id": "raw",
            "enabled": true
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(service).unwrap()["retain_history"],
            true
        );
    }

    #[test]
    fn explicit_history_retention_survives_service_json_round_trip() {
        let service: Service = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "transport": "tcp",
            "listen_address": "127.0.0.1:2001",
            "upstream_address": "127.0.0.1:2002",
            "protocol_id": "raw",
            "enabled": true,
            "retain_history": false
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(service).unwrap()["retain_history"],
            false
        );
    }
}

mod decimal;
mod rules;
pub use rules::*;

pub mod protocols;
