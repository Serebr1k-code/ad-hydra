//! Portable ABI data types. No host, networking or async dependencies.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const ABI_VERSION: i32 = 1;
pub const MAX_INPUT: usize = 65_536;
pub const MAX_OUTPUT: usize = 65_536;
pub const MAX_STATE: usize = 65_536;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    String(String),
    Signed(i64),
    Unsigned(u64),
    Bool(bool),
    Double(f64),
    Bytes(Vec<u8>),
    Object(BTreeMap<String, Value>),
    List(Vec<Value>),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ParsedMessage {
    pub protocol: String,
    pub message_type: String,
    pub direction: Direction,
    pub fields: BTreeMap<String, Value>,
}
#[derive(Serialize, Deserialize)]
pub struct DecodeInput {
    pub direction: Direction,
    pub bytes: Vec<u8>,
    pub state: Vec<u8>,
}
#[derive(Default, Serialize, Deserialize)]
pub struct DecodeOutput {
    pub messages: Vec<ParsedMessage>,
    pub state: Vec<u8>,
}
impl DecodeOutput {
    pub fn validate(&self) -> bool {
        fn value(v: &Value, depth: usize, count: &mut usize) -> bool {
            *count += 1;
            if depth > 8 || *count > 256 {
                return false;
            }
            match v {
                Value::String(s) => s.len() <= 8192,
                Value::Bytes(b) => b.len() <= 8192,
                Value::Double(f) => f.is_finite(),
                Value::Object(m) => m
                    .iter()
                    .all(|(k, v)| k.len() <= 128 && value(v, depth + 1, count)),
                Value::List(l) => l.iter().all(|v| value(v, depth + 1, count)),
                _ => true,
            }
        }
        let mut count = 0;
        self.state.len() <= MAX_STATE
            && self.messages.len() <= 32
            && self.messages.iter().all(|m| {
                m.protocol.len() <= 128
                    && m.message_type.len() <= 128
                    && m.fields
                        .iter()
                        .all(|(k, v)| k.len() <= 128 && value(v, 0, &mut count))
            })
    }
}

/// Schema paths use the same dotted field paths as rule predicates.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    Signed,
    Unsigned,
    Bool,
    Double,
    Bytes,
    Object,
    List,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSchema {
    pub path: String,
    pub field_type: FieldType,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParserMetadata {
    pub name: String,
    pub fields: Vec<FieldSchema>,
}
impl ParserMetadata {
    pub fn validate(&self) -> bool {
        !self.name.is_empty()
            && self.name.len() <= 128
            && self.fields.len() <= 64
            && self
                .fields
                .iter()
                .all(|f| !f.path.is_empty() && f.path.len() <= 128)
    }
}
