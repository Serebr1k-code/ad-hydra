use crate::{DecodeError, Direction, ParsedMessage};
use hydra_protocol_sdk::FieldSchema;

/// Shared host-side contract for built-in and WASM decoders.
pub trait ProtocolDecoder {
    fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError>;
    fn field_schema(&self) -> Vec<FieldSchema>;
}
