use crate::{DecodeError, Direction, ParsedMessage, ProtocolDecoder, Value};
use hydra_protocol_sdk::{FieldSchema, FieldType, MAX_INPUT};
use std::collections::BTreeMap;

/// Maximum pattern size accepted by the raw-byte filter editor.
pub const MAX_RAW_PATTERN: usize = 1024;

/// A raw profile exposes bounded byte windows for local filtering. The proxy
/// must not publish these messages to telemetry because they contain payload.
#[derive(Default)]
pub struct RawDecoder {
    tails: [Vec<u8>; 2],
}

impl ProtocolDecoder for RawDecoder {
    fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        if bytes.len() > MAX_INPUT {
            return Err(DecodeError("raw decoder input exceeds 64 KiB".into()));
        }
        let index = usize::from(direction == Direction::ServerToClient);
        let tail = &mut self.tails[index];
        let mut window = Vec::with_capacity(tail.len() + bytes.len());
        window.extend_from_slice(tail);
        window.extend_from_slice(bytes);
        let retain = MAX_RAW_PATTERN.saturating_sub(1).min(window.len());
        tail.clear();
        tail.extend_from_slice(&window[window.len() - retain..]);
        Ok(vec![ParsedMessage {
            protocol: "raw".into(),
            message_type: "bytes".into(),
            direction,
            fields: BTreeMap::from([("raw.bytes".into(), Value::Bytes(window))]),
        }])
    }

    fn field_schema(&self) -> Vec<FieldSchema> {
        vec![FieldSchema {
            path: "raw.bytes".into(),
            field_type: FieldType::Bytes,
        }]
    }
}

impl RawDecoder {
    /// UDP boundaries define complete messages, so overlap never crosses datagrams.
    pub fn feed_datagram(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        let index = usize::from(direction == Direction::ServerToClient);
        self.tails[index].clear();
        let message = ProtocolDecoder::feed(self, direction, bytes)?;
        self.tails[index].clear();
        Ok(message)
    }
}
