use crate::{DecodeError, ProtocolDecoder};
use hydra_protocol_sdk::{Direction, FieldSchema, FieldType, MAX_INPUT, ParsedMessage, Value};
use std::collections::BTreeMap;

const TEXT_CHUNK: usize = 4096;

/// Bounded best-effort text decoder. It retains at most three trailing bytes
/// when a UTF-8 code point is split across transport reads.
#[derive(Default)]
pub struct Utf8Decoder {
    tails: [Vec<u8>; 2],
}

impl Utf8Decoder {
    pub fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        if bytes.len() > MAX_INPUT {
            return Err(DecodeError("UTF-8 decoder input exceeds 64 KiB".into()));
        }
        let tail = &mut self.tails[match direction {
            Direction::ClientToServer => 0,
            Direction::ServerToClient => 1,
        }];
        let mut input = Vec::with_capacity(tail.len() + bytes.len());
        input.extend_from_slice(tail);
        input.extend_from_slice(bytes);
        tail.clear();
        let mut decoded = TextMessages::new(direction);
        let mut offset = 0;
        while offset < input.len() {
            match std::str::from_utf8(&input[offset..]) {
                Ok(valid) => {
                    decoded.text(valid);
                    break;
                }
                Err(error) => {
                    let valid_end = offset + error.valid_up_to();
                    decoded.text(
                        std::str::from_utf8(&input[offset..valid_end])
                            .map_err(|error| DecodeError(error.to_string()))?,
                    );
                    offset = valid_end;
                    if let Some(invalid_len) = error.error_len() {
                        decoded.invalid(&input[offset..offset + invalid_len]);
                        offset += invalid_len;
                    } else {
                        tail.extend_from_slice(&input[offset..]);
                        if tail.len() > 3 {
                            return Err(DecodeError(
                                "UTF-8 decoder retained an invalid tail".into(),
                            ));
                        }
                        break;
                    }
                }
            }
        }
        decoded.flush();
        Ok(decoded.messages)
    }
}

impl ProtocolDecoder for Utf8Decoder {
    fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        Utf8Decoder::feed(self, direction, bytes)
    }
    fn field_schema(&self) -> Vec<FieldSchema> {
        vec![
            FieldSchema {
                path: "utf8.text".into(),
                field_type: FieldType::String,
            },
            FieldSchema {
                path: "utf8.spans".into(),
                field_type: FieldType::List,
            },
        ]
    }
}

/// Bound annotation cardinality as well as text size, even for alternating valid/invalid bytes.
struct TextMessages {
    direction: Direction,
    messages: Vec<ParsedMessage>,
    text: String,
    spans: Vec<Value>,
    invalid: bool,
}
impl TextMessages {
    fn new(direction: Direction) -> Self {
        Self {
            direction,
            messages: Vec::new(),
            text: String::new(),
            spans: Vec::new(),
            invalid: false,
        }
    }
    fn text(&mut self, mut value: &str) {
        while !value.is_empty() {
            if self.spans.len() >= 64 || self.text.len() >= TEXT_CHUNK {
                self.flush();
            }
            let mut end = value.len().min(TEXT_CHUNK - self.text.len());
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            if end == 0 {
                self.flush();
                continue;
            }
            self.text.push_str(&value[..end]);
            self.spans.push(Value::String(value[..end].into()));
            value = &value[end..];
        }
    }
    fn invalid(&mut self, bytes: &[u8]) {
        if self.spans.len() >= 64 || self.text.len() + 3 > TEXT_CHUNK {
            self.flush();
        }
        self.text.push('\u{fffd}');
        self.spans.push(Value::Bytes(bytes.to_vec()));
        self.invalid = true;
    }
    fn flush(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let mut fields = BTreeMap::new();
        fields.insert(
            "utf8.text".into(),
            Value::String(std::mem::take(&mut self.text)),
        );
        let spans = std::mem::take(&mut self.spans);
        if self.invalid {
            fields.insert("utf8.spans".into(), Value::List(spans));
        }
        self.invalid = false;
        self.messages.push(ParsedMessage {
            protocol: "utf8".into(),
            message_type: "text".into(),
            direction: self.direction,
            fields,
        });
    }
}
