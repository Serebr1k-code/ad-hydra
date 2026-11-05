pub mod body_size;
pub mod flags;
pub mod protocols;
use hydra_domain::{FieldValue, Service, Transport};
use std::{collections::BTreeMap, net::SocketAddr};

pub fn network_fields(
    service: &Service,
    client: SocketAddr,
    direction: &str,
) -> BTreeMap<String, FieldValue> {
    BTreeMap::from([
        (
            "network.src_ip".into(),
            FieldValue::String(client.ip().to_string()),
        ),
        (
            "network.src_port".into(),
            FieldValue::Uint(client.port().into()),
        ),
        (
            "network.dst_ip".into(),
            FieldValue::String(service.effective_upstream().ip().to_string()),
        ),
        (
            "network.dst_port".into(),
            FieldValue::Uint(service.effective_upstream().port().into()),
        ),
        (
            "transport.kind".into(),
            FieldValue::String(
                match service.transport {
                    Transport::Tcp => "tcp",
                    Transport::Udp => "udp",
                }
                .into(),
            ),
        ),
        ("direction".into(), FieldValue::String(direction.into())),
    ])
}
pub fn domain_value(value: &hydra_protocol_sdk::Value) -> FieldValue {
    use hydra_protocol_sdk::Value as V;
    match value {
        V::String(v) => FieldValue::String(v.clone()),
        V::Signed(v) => FieldValue::Int(*v),
        V::Unsigned(v) => FieldValue::Uint(*v),
        V::Double(v) => FieldValue::Double(*v),
        V::Bool(v) => FieldValue::Bool(*v),
        V::Bytes(v) => FieldValue::Bytes(v.clone()),
        V::Object(v) => FieldValue::Object(
            v.iter()
                .map(|(k, v)| (k.clone(), domain_value(v)))
                .collect(),
        ),
        V::List(v) => FieldValue::List(v.iter().map(domain_value).collect()),
    }
}
