use crate::{
    inspection::{network_fields, protocols::ProtocolSession},
    telemetry::Telemetry,
};
use hydra_domain::{FieldValue, Service};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

pub type Decoder = Arc<Mutex<Option<ProtocolSession>>>;

/// Shared parent state for the TCP and UDP relay implementations.
/// Transport-specific scheduling and socket behavior stays in each relay.
pub struct RelayBase {
    pub config: Service,
    pub client: SocketAddr,
    pub telemetry: Arc<Telemetry>,
    pub decoder: Decoder,
}

impl RelayBase {
    pub fn new(
        config: Service,
        client: SocketAddr,
        telemetry: Arc<Telemetry>,
        decoder: Decoder,
    ) -> Self {
        Self {
            config,
            client,
            telemetry,
            decoder,
        }
    }

    /// Merge decoder output with authoritative transport metadata. Network and
    /// packet fields are written last so a plugin cannot spoof host observations.
    pub fn inspected_fields(
        &self,
        direction: &str,
        mut decoded: std::collections::BTreeMap<String, FieldValue>,
        packet_size: usize,
    ) -> std::collections::BTreeMap<String, FieldValue> {
        decoded.extend(network_fields(&self.config, self.client, direction));
        decoded.insert("packet.size".into(), FieldValue::Uint(packet_size as u64));
        decoded
    }
}

/// Common object contract implemented by each transport relay context.
pub trait RelayTransport {
    fn base(&self) -> &RelayBase;
    fn network_fields(&self, direction: &str) -> std::collections::BTreeMap<String, FieldValue> {
        let base = self.base();
        network_fields(&base.config, base.client, direction)
    }
}
