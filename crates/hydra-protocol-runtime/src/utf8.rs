use crate::DecodeError;
use hydra_protocol_sdk::{Direction, MAX_INPUT, ParsedMessage, Value};
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
        let mut decoded = String::with_capacity(input.len());
        let mut offset = 0;
        while offset < input.len() {
            match std::str::from_utf8(&input[offset..]) {
                Ok(valid) => {
                    decoded.push_str(valid);
                    break;
                }
                Err(error) => {
                    let valid_end = offset + error.valid_up_to();
                    decoded.push_str(
                        std::str::from_utf8(&input[offset..valid_end])
                            .map_err(|error| DecodeError(error.to_string()))?,
                    );
                    offset = valid_end;
                    if let Some(invalid_len) = error.error_len() {
                        decoded.push('\u{fffd}');
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
        let mut messages = Vec::with_capacity(decoded.len().div_ceil(TEXT_CHUNK));
        let mut start = 0;
        while start < decoded.len() {
            let mut end = (start + TEXT_CHUNK).min(decoded.len());
            while !decoded.is_char_boundary(end) {
                end -= 1;
            }
            let mut fields = BTreeMap::new();
            fields.insert(
                "utf8.text".into(),
                Value::String(decoded[start..end].into()),
            );
            messages.push(ParsedMessage {
                protocol: "utf8".into(),
                message_type: "text".into(),
                direction,
                fields,
            });
            start = end;
        }
        Ok(messages)
    }
}
