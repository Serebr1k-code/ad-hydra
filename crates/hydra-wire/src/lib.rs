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
                    forwarding: s.forwarding.as_ref().map(Into::into),
                    id: s.id.clone(),
                    transport: match s.transport {
                        Transport::Tcp => 1,
                        Transport::Udp => 2,
                    },
                    listen_address: s.listen_address.to_string(),
                    upstream_address: s.upstream_address.to_string(),
                    protocol_id: s.protocol_id.clone(),
                    enabled: s.enabled,
                    retain_history: s.retain_history,
                    protection: s.protection.as_ref().map(|p| v1::ServiceProtection {
                        max_active_flows: p.max_active_flows,
                        tcp_connections_per_second: p.tcp_connections_per_second,
                        udp_datagrams_per_second: p.udp_datagrams_per_second,
                        bytes_per_second: p.bytes_per_second,
                        idle_timeout_ms: p.idle_timeout_ms,
                    }),
                })
                .collect(),
            ..Default::default()
        }
    }
}
impl TryFrom<&v1::ApplyServiceSnapshotRequest> for ServiceSnapshot {
    type Error = String;
    fn try_from(r: &v1::ApplyServiceSnapshotRequest) -> Result<Self, String> {
        let services =
            r.services
                .iter()
                .map(|s| {
                    Ok(Service {
                        forwarding: s.forwarding.as_ref().map(TryInto::try_into).transpose()?,
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
                        retain_history: s.retain_history,
                        protection: s.protection.as_ref().map(|p| {
                            hydra_domain::ServiceProtection {
                                max_active_flows: p.max_active_flows,
                                tcp_connections_per_second: p.tcp_connections_per_second,
                                udp_datagrams_per_second: p.udp_datagrams_per_second,
                                bytes_per_second: p.bytes_per_second,
                                idle_timeout_ms: p.idle_timeout_ms,
                            }
                        }),
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
                forwarding: None,
                transport: 99,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(ServiceSnapshot::try_from(&r).is_err());
    }
    #[test]
    fn protection_round_trip_and_validation() {
        let mut wire = v1::ApplyServiceSnapshotRequest {
            revision: 1,
            services: vec![v1::ServiceConfig {
                forwarding: None,
                id: "svc".into(),
                transport: 1,
                listen_address: "127.0.0.1:2001".into(),
                upstream_address: "127.0.0.1:2002".into(),
                protocol_id: "raw".into(),
                enabled: true,
                retain_history: true,
                protection: Some(v1::ServiceProtection {
                    max_active_flows: 128,
                    tcp_connections_per_second: 50,
                    udp_datagrams_per_second: 1000,
                    bytes_per_second: 8388608,
                    idle_timeout_ms: 30000,
                }),
            }],
            ..Default::default()
        };
        let decoded = ServiceSnapshot::try_from(&wire).unwrap();
        assert_eq!(v1::ApplyServiceSnapshotRequest::from(&decoded), wire);
        wire.services[0]
            .protection
            .as_mut()
            .unwrap()
            .max_active_flows = 0;
        assert!(ServiceSnapshot::try_from(&wire).is_err());
        wire.services[0]
            .protection
            .as_mut()
            .unwrap()
            .max_active_flows = 1;
        wire.services[0]
            .protection
            .as_mut()
            .unwrap()
            .idle_timeout_ms = 0;
        assert!(ServiceSnapshot::try_from(&wire).is_err());
    }
    #[test]
    fn snapshot_round_trip() {
        let snapshot = ServiceSnapshot::default();
        assert_eq!(
            ServiceSnapshot::try_from(&v1::ApplyServiceSnapshotRequest::from(&snapshot)).unwrap(),
            snapshot
        );
    }

    #[test]
    fn service_history_setting_survives_protobuf_snapshot_round_trip() {
        let snapshot: ServiceSnapshot = serde_json::from_value(serde_json::json!({
            "revision": 7,
            "services": [{
                "id": "svc",
                "transport": "tcp",
                "listen_address": "127.0.0.1:2001",
                "upstream_address": "127.0.0.1:2002",
                "protocol_id": "raw",
                "enabled": true,
                "retain_history": false
            }]
        }))
        .unwrap();
        let wire = v1::ApplyServiceSnapshotRequest::from(&snapshot);
        let decoded = ServiceSnapshot::try_from(&wire).unwrap();
        assert_eq!(decoded, snapshot);
    }
}

pub mod rules;

impl From<&hydra_domain::Forwarding> for v1::ForwardingConfig {
    fn from(f: &hydra_domain::Forwarding) -> Self {
        use hydra_domain::ForwardFraming as D;
        use v1::forwarding_config::Framing as W;
        Self {
            team_id: f.team_id.clone(),
            target_address: f.target_address.to_string(),
            flag_pattern: f.flag_pattern.clone(),
            framing: Some(match &f.framing {
                D::Http1 => W::Http1(true),
                D::Datagram => W::Datagram(true),
                D::Delimited { delimiter } => W::Delimiter(delimiter.clone()),
                D::LengthPrefixed {
                    offset,
                    width,
                    little_endian,
                    length_includes_header,
                } => W::LengthPrefix(v1::ForwardLengthPrefix {
                    offset: (*offset).into(),
                    width: (*width).into(),
                    little_endian: *little_endian,
                    length_includes_header: *length_includes_header,
                }),
            }),
        }
    }
}
impl TryFrom<&v1::ForwardingConfig> for hydra_domain::Forwarding {
    type Error = String;
    fn try_from(f: &v1::ForwardingConfig) -> Result<Self, String> {
        use hydra_domain::ForwardFraming as D;
        use v1::forwarding_config::Framing as W;
        let framing = match f.framing.as_ref() {
            Some(W::Http1(true)) => D::Http1,
            Some(W::Datagram(true)) => D::Datagram,
            Some(W::Delimiter(delimiter)) => D::Delimited {
                delimiter: delimiter.clone(),
            },
            Some(W::LengthPrefix(p)) => D::LengthPrefixed {
                offset: p.offset.try_into().map_err(|_| "invalid prefix offset")?,
                width: p.width.try_into().map_err(|_| "invalid prefix width")?,
                little_endian: p.little_endian,
                length_includes_header: p.length_includes_header,
            },
            _ => return Err("invalid forwarding framing".into()),
        };
        Ok(Self {
            team_id: f.team_id.clone(),
            target_address: f
                .target_address
                .parse()
                .map_err(|_| "invalid forwarding target")?,
            flag_pattern: f.flag_pattern.clone(),
            framing,
        })
    }
}

#[cfg(test)]
mod forwarding_tests {
    use super::*;
    #[test]
    fn forwarding_contract_round_trips_all_framing_kinds() {
        for framing in [
            hydra_domain::ForwardFraming::Http1,
            hydra_domain::ForwardFraming::Datagram,
            hydra_domain::ForwardFraming::Delimited {
                delimiter: "\r\n".into(),
            },
            hydra_domain::ForwardFraming::LengthPrefixed {
                offset: 1,
                width: 4,
                little_endian: false,
                length_includes_header: false,
            },
        ] {
            let config = hydra_domain::Forwarding {
                team_id: "two".into(),
                target_address: "192.0.2.3:9000".parse().unwrap(),
                flag_pattern: "FLAG[A-Z0-9]{8}".into(),
                framing,
            };
            assert_eq!(
                hydra_domain::Forwarding::try_from(&v1::ForwardingConfig::from(&config)).unwrap(),
                config
            );
        }
    }
}
