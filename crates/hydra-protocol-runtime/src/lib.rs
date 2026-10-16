//! Bounded decoders; decoding never owns or edits transport buffers.
mod framing;
mod http;
pub use framing::FramingDecoder;
mod wasm;
pub use http::{HttpBatch, HttpDecoder, HttpRequest};
pub use hydra_protocol_sdk::{Direction, ParsedMessage, Value};
pub use wasm::{PluginRegistry, PluginVersion, WasmSession};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);
impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DecodeError {}
pub enum SessionDecoder {
    Raw,
    Framing(FramingDecoder),
    Http(HttpDecoder),
    Wasm(Box<WasmSession>),
}
impl SessionDecoder {
    pub fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        match self {
            Self::Framing(f) => f.feed(direction, bytes),
            Self::Raw => Ok(Vec::new()),
            Self::Http(h) => h
                .feed(direction, bytes)
                .map(|v| v.into_iter().map(|r| r.message).collect()),
            Self::Wasm(w) => w.feed(direction, bytes),
        }
    }
}
