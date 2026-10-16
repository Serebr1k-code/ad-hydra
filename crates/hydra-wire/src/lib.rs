//! Only this crate compiles the canonical protobuf contract.
#[allow(clippy::result_large_err)] // tonic 0.12 generated signatures use tonic::Status.
pub mod v1 {
    tonic::include_proto!("hydra.proxy.v1");
}
use hydra_domain::{Service, ServiceSnapshot, Transport};

impl From<&ServiceSnapshot> for v1::ApplyServiceSnapshotRequest {
    fn from(s: &ServiceSnapshot) -> Self {
        Self {
            revision: s.revision,
            services: s
                .services
                .iter()
                .map(|s| v1::ServiceConfig {
                    id: s.id.clone(),
                    transport: match s.transport {
                        Transport::Tcp => 1,
                        Transport::Udp => 2,
                    },
                    listen_address: s.listen_address.to_string(),
                    upstream_address: s.upstream_address.to_string(),
                    protocol_id: s.protocol_id.clone(),
                    enabled: s.enabled,
                })
                .collect(),
            ..Default::default()
        }
    }
}
impl TryFrom<&v1::ApplyServiceSnapshotRequest> for ServiceSnapshot {
    type Error = String;
    fn try_from(r: &v1::ApplyServiceSnapshotRequest) -> Result<Self, String> {
        let services = r
            .services
            .iter()
            .map(|s| {
                Ok(Service {
                    id: s.id.clone(),
                    transport: match s.transport {
                        1 => Transport::Tcp,
                        2 => Transport::Udp,
                        _ => return Err("invalid transport".into()),
                    },
                    listen_address: s
                        .listen_address
                        .parse()
                        .map_err(|_| "invalid numeric listen endpoint")?,
                    upstream_address: s
                        .upstream_address
                        .parse()
                        .map_err(|_| "invalid numeric upstream endpoint")?,
                    protocol_id: s.protocol_id.clone(),
                    enabled: s.enabled,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let snapshot = Self {
            revision: r.revision,
            services,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_transport() {
        let r = v1::ApplyServiceSnapshotRequest {
            services: vec![v1::ServiceConfig {
                transport: 99,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(ServiceSnapshot::try_from(&r).is_err());
    }
    #[test]
    fn snapshot_round_trip() {
        let snapshot = ServiceSnapshot::default();
        assert_eq!(
            ServiceSnapshot::try_from(&v1::ApplyServiceSnapshotRequest::from(&snapshot)).unwrap(),
            snapshot
        );
    }
}

pub mod rules;
