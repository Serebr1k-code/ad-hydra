//! Declarative, bounded framing configuration. Length counts include or exclude the header.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Framing {
    Raw,
    Delimiter {
        delimiter: Vec<u8>,
    },
    FixedSize {
        size: usize,
    },
    LengthPrefix {
        offset: usize,
        width: usize,
        little_endian: bool,
        includes_header: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramingProfile {
    pub framing: Framing,
    pub max_frame: usize,
}
impl FramingProfile {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(1..=8192).contains(&self.max_frame) {
            return Err("max_frame must be 1..8192");
        }
        match &self.framing {
            Framing::Delimiter { delimiter }
                if delimiter.is_empty()
                    || delimiter.len() > 64
                    || delimiter.len() > self.max_frame =>
            {
                Err("delimiter must contain 1..64 bytes and fit max_frame")
            }
            Framing::FixedSize { size } if *size == 0 || *size > self.max_frame => {
                Err("fixed size must fit max_frame")
            }
            Framing::LengthPrefix { offset, width, .. }
                if ![1, 2, 4, 8].contains(width)
                    || offset
                        .checked_add(*width)
                        .is_none_or(|n| n > self.max_frame) =>
            {
                Err("invalid length header")
            }
            _ => Ok(()),
        }
    }
}
