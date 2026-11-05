use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Forwarding {
    pub team_id: String,
    pub target_address: SocketAddr,
    pub flag_pattern: String,
    pub framing: ForwardFraming,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForwardFraming {
    Datagram,
    Http1,
    Delimited {
        delimiter: String,
    },
    LengthPrefixed {
        offset: u8,
        width: u8,
        little_endian: bool,
        length_includes_header: bool,
    },
}
