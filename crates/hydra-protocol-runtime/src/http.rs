use crate::{DecodeError, Direction, ParsedMessage, Value};
use std::collections::BTreeMap;
/// Complete request bytes, suitable for a decision BEFORE forwarding any bytes.
pub struct HttpRequest {
    pub bytes: Vec<u8>,
    pub message: ParsedMessage,
}
/// Complete prefix frames must be evaluated before handling the failed remainder.
pub struct HttpBatch {
    pub requests: Vec<HttpRequest>,
    pub failure: Option<DecodeError>,
}
#[derive(Default)]
pub struct HttpDecoder {
    buffer: Vec<u8>,
    failed: bool,
}
impl HttpDecoder {
    pub fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<HttpRequest>, DecodeError> {
        if self.failed {
            return Err(DecodeError("decoder disabled".into()));
        }
        let original = self.buffer.clone();
        let batch = self.feed_available(direction, bytes);
        match batch.failure {
            Some(error) => {
                self.buffer = original;
                if direction == Direction::ClientToServer && bytes.len() <= 65_536 {
                    self.buffer.extend_from_slice(bytes);
                }
                Err(error)
            }
            None => Ok(batch.requests),
        }
    }
    /// Emits every safely framed prefix request, even when a later request fails.
    /// On failure take_pending contains ONLY the undecoded remainder.
    pub fn feed_available(&mut self, direction: Direction, bytes: &[u8]) -> HttpBatch {
        let mut requests = vec![];
        let failure = if self.failed {
            Some(DecodeError("decoder disabled".into()))
        } else {
            self.decode(direction, bytes, &mut requests).err()
        };
        if failure.is_some() {
            self.failed = true;
        }
        HttpBatch { requests, failure }
    }
    fn decode(
        &mut self,
        direction: Direction,
        bytes: &[u8],
        requests: &mut Vec<HttpRequest>,
    ) -> Result<(), DecodeError> {
        if direction == Direction::ServerToClient {
            return Ok(());
        }
        if bytes.len() > 65_536 {
            return Err(DecodeError("HTTP buffer limit".into()));
        }
        self.buffer.extend_from_slice(bytes);
        while let Some(end) = self
            .buffer
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
        {
            if end > 16_384 {
                return Err(DecodeError("HTTP header limit".into()));
            }
            let header = std::str::from_utf8(&self.buffer[..end])
                .map_err(|_| DecodeError("HTTP header encoding".into()))?;
            let mut lines = header.split("\r\n");
            let parts: Vec<_> = lines.next().unwrap_or("").split(' ').collect();
            if parts.len() != 3
                || parts[2] != "HTTP/1.1"
                || parts[0].is_empty()
                || !parts[0].bytes().all(|b| b.is_ascii_uppercase())
                || !parts[1].starts_with('/')
                || parts[1].len() > 8192
                || parts[1].bytes().any(|b| b <= 32 || b == 127)
            {
                return Err(DecodeError("unsupported HTTP request line".into()));
            }
            let mut fields = BTreeMap::from([
                ("http.method".into(), Value::String(parts[0].into())),
                ("http.path".into(), Value::String(parts[1].into())),
            ]);
            let mut length = None;
            for line in lines.filter(|s| !s.is_empty()) {
                let (name, value) = line
                    .split_once(':')
                    .ok_or_else(|| DecodeError("HTTP header syntax".into()))?;
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                    || value.bytes().any(|b| b < 32 && b != 9)
                {
                    return Err(DecodeError("HTTP header syntax".into()));
                }
                let value = value.trim();
                if name.eq_ignore_ascii_case("transfer-encoding")
                    || name.eq_ignore_ascii_case("upgrade")
                    || name.eq_ignore_ascii_case("expect")
                {
                    return Err(DecodeError(
                        "unsupported HTTP framing/upgrade/expect".into(),
                    ));
                }
                if name.eq_ignore_ascii_case("content-length") {
                    if length.is_some()
                        || value.is_empty()
                        || !value.bytes().all(|b| b.is_ascii_digit())
                    {
                        return Err(DecodeError("ambiguous HTTP content length".into()));
                    }
                    length = Some(
                        value
                            .parse::<usize>()
                            .map_err(|_| DecodeError("HTTP content length".into()))?,
                    );
                }
                if name.eq_ignore_ascii_case("user-agent") {
                    if value.len() > 8192 {
                        return Err(DecodeError("HTTP field limit".into()));
                    }
                    fields.insert("http.user_agent".into(), Value::String(value.into()));
                }
            }
            let length = length.unwrap_or(0);
            if length > 49_152 || end + length > 65_536 {
                return Err(DecodeError("HTTP body limit".into()));
            }
            if self.buffer.len() < end + length {
                break;
            }
            fields.insert("http.body_length".into(), Value::Unsigned(length as u64));
            let bytes = self.buffer.drain(..end + length).collect();
            requests.push(HttpRequest {
                bytes,
                message: ParsedMessage {
                    protocol: "http1".into(),
                    message_type: "request".into(),
                    direction,
                    fields,
                },
            });
        }
        if self.buffer.len() > 16_384 && !self.buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            return Err(DecodeError("HTTP header limit".into()));
        }
        Ok(())
    }
    /// Recover unforwarded bytes on failure/EOF; never contains already emitted frames.
    pub fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }
}
