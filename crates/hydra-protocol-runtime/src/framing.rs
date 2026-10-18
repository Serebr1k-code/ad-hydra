use crate::{DecodeError, Direction, ParsedMessage, ProtocolDecoder, Value};
use hydra_domain::protocols::{Framing, FramingProfile};
use hydra_protocol_sdk::{FieldSchema, FieldType};
use std::collections::BTreeMap;
/// Per-flow snapshot: profile activation never changes a live decoder.
pub struct FramingDecoder {
    id: String,
    profile: FramingProfile,
    buffers: [Vec<u8>; 2],
    failed: bool,
}
impl FramingDecoder {
    pub fn new(id: String, profile: FramingProfile) -> Result<Self, DecodeError> {
        profile.validate().map_err(|e| DecodeError(e.into()))?;
        Ok(Self {
            id,
            profile,
            buffers: [Vec::new(), Vec::new()],
            failed: false,
        })
    }
    pub fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        if self.failed || matches!(self.profile.framing, Framing::Raw) {
            return Ok(vec![]);
        }
        let index = usize::from(direction == Direction::ServerToClient);
        let mut frames = Vec::new();
        // Consume incrementally so even a very large read never grows parser state past its cap.
        for byte in bytes {
            let buffer = &mut self.buffers[index];
            buffer.push(*byte);
            let complete = match &self.profile.framing {
                Framing::Raw => false,
                Framing::Delimiter { delimiter } => buffer.ends_with(delimiter),
                Framing::FixedSize { size } => buffer.len() == *size,
                Framing::LengthPrefix {
                    offset,
                    width,
                    little_endian,
                    includes_header,
                } => {
                    let header = offset + width;
                    if buffer.len() < header {
                        false
                    } else {
                        let mut n = 0u64;
                        if *little_endian {
                            for (i, b) in buffer[*offset..header].iter().enumerate() {
                                n |= u64::from(*b) << (8 * i);
                            }
                        } else {
                            for b in &buffer[*offset..header] {
                                n = (n << 8) | u64::from(*b);
                            }
                        }
                        let total = usize::try_from(n).ok().and_then(|n| {
                            if *includes_header {
                                Some(n)
                            } else {
                                n.checked_add(header)
                            }
                        });
                        match total {
                            Some(total) if total >= header && total <= self.profile.max_frame => {
                                buffer.len() == total
                            }
                            _ => {
                                self.failed = true;
                                self.buffers = [Vec::new(), Vec::new()];
                                return Err(DecodeError(
                                    "invalid frame length; inspection disabled".into(),
                                ));
                            }
                        }
                    }
                }
            };
            if complete {
                if frames.len() >= 32 {
                    self.failed = true;
                    self.buffers = [Vec::new(), Vec::new()];
                    return Err(DecodeError("frame batch cap; inspection disabled".into()));
                }
                frames.push(ParsedMessage {
                    protocol: self.id.clone(),
                    message_type: "frame".into(),
                    direction,
                    fields: BTreeMap::from([(
                        "payload".into(),
                        Value::Bytes(std::mem::take(buffer)),
                    )]),
                });
            } else if buffer.len() >= self.profile.max_frame {
                self.failed = true;
                self.buffers = [Vec::new(), Vec::new()];
                return Err(DecodeError("frame cap; inspection disabled".into()));
            }
        }
        Ok(frames)
    }
}
impl ProtocolDecoder for FramingDecoder {
    fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        FramingDecoder::feed(self, direction, bytes)
    }
    fn field_schema(&self) -> Vec<FieldSchema> {
        if matches!(self.profile.framing, Framing::Raw) {
            vec![]
        } else {
            vec![FieldSchema {
                path: "payload".into(),
                field_type: FieldType::Bytes,
            }]
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn decoder(framing: Framing, max_frame: usize) -> FramingDecoder {
        FramingDecoder::new("test".into(), FramingProfile { framing, max_frame }).unwrap()
    }
    #[test]
    fn fragmented_delimiter_and_direction_isolation() {
        let mut d = decoder(
            Framing::Delimiter {
                delimiter: b"\r\n".to_vec(),
            },
            16,
        );
        assert!(
            d.feed(Direction::ClientToServer, b"abc\r")
                .unwrap()
                .is_empty()
        );
        assert!(d.feed(Direction::ServerToClient, b"x").unwrap().is_empty());
        assert_eq!(
            d.feed(Direction::ClientToServer, b"\n").unwrap()[0].fields["payload"],
            Value::Bytes(b"abc\r\n".to_vec())
        );
    }
    #[test]
    fn included_length_overflow_and_raw_passthrough() {
        let mut decoder = decoder(
            Framing::LengthPrefix {
                offset: 0,
                width: 1,
                little_endian: false,
                includes_header: true,
            },
            8,
        );
        assert!(
            decoder
                .feed(Direction::ClientToServer, &[3, b'a'])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            decoder.feed(Direction::ClientToServer, b"b").unwrap()[0].fields["payload"],
            Value::Bytes(vec![3, b'a', b'b'])
        );
        let mut decoder = super::FramingDecoder::new(
            "x".into(),
            FramingProfile {
                framing: Framing::LengthPrefix {
                    offset: 0,
                    width: 8,
                    little_endian: false,
                    includes_header: false,
                },
                max_frame: 8,
            },
        )
        .unwrap();
        assert!(decoder.feed(Direction::ClientToServer, &[255; 8]).is_err());
        let mut decoder = super::FramingDecoder::new(
            "x".into(),
            FramingProfile {
                framing: Framing::Raw,
                max_frame: 8,
            },
        )
        .unwrap();
        assert!(
            decoder
                .feed(Direction::ClientToServer, b"opaque unchanged bytes")
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn prefix_semantics_and_cap() {
        for little_endian in [false, true] {
            let mut d = decoder(
                Framing::LengthPrefix {
                    offset: 1,
                    width: 2,
                    little_endian,
                    includes_header: false,
                },
                8,
            );
            let header = if little_endian { [9, 2, 0] } else { [9, 0, 2] };
            assert!(
                d.feed(Direction::ClientToServer, &header)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(d.feed(Direction::ClientToServer, b"ab").unwrap().len(), 1);
        }
        let mut d = decoder(Framing::FixedSize { size: 2 }, 2);
        assert_eq!(d.feed(Direction::ClientToServer, b"abcd").unwrap().len(), 2);
        let mut d = decoder(Framing::Delimiter { delimiter: vec![0] }, 2);
        assert!(d.feed(Direction::ClientToServer, b"abc").is_err());
        assert!(
            d.feed(Direction::ClientToServer, b"unchanged transport")
                .unwrap()
                .is_empty()
        );
    }
}
