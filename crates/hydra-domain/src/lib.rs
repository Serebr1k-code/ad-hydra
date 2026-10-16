//! Pure, framework-independent service configuration shared by both applications.
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
    pub id: String,
    pub transport: Transport,
    pub listen_address: SocketAddr,
    pub upstream_address: SocketAddr,
    #[serde(default = "raw")]
    pub protocol_id: String,
    pub enabled: bool,
}
fn raw() -> String {
    "raw".into()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceSnapshot {
    pub revision: u64,
    pub services: Vec<Service>,
}

impl ServiceSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.services.len() > 128 {
            return Err("at most 128 services are allowed".into());
        }
        let mut ids = HashSet::new();
        let mut endpoints = HashSet::new();
        for s in &self.services {
            if s.id.is_empty() || s.id.len() > 128 || !ids.insert(&s.id) {
                return Err("service IDs must be unique, nonempty, and at most 128 bytes".into());
            }
            if !matches!(s.protocol_id.as_str(), "raw" | "http") {
                return Err("only raw and http profiles are built in".into());
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
                    && other.listen_address.port() == s.upstream_address.port()
                    && (other.listen_address.ip() == s.upstream_address.ip()
                        || (other.listen_address.ip().is_unspecified()
                            && s.upstream_address.ip().is_loopback()))
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
            id: "one".into(),
            transport: Transport::Tcp,
            listen_address: "127.0.0.1:2001".parse().unwrap(),
            upstream_address: "127.0.0.1:2002".parse().unwrap(),
            protocol_id: "raw".into(),
            enabled: true,
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
}

mod decimal;
mod rules;
pub use rules::*;

pub mod protocols;
