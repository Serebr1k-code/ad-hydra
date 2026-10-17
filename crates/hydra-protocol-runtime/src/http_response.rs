//! Passive HTTP response inspection. Never holds transport bytes for forwarding.
use crate::{DecodeError, Direction, HttpRequest, ParsedMessage, Value};
use std::collections::{BTreeMap, VecDeque};

const LIMIT: usize = 65_536;
const PREVIEW: usize = 4096;
#[derive(Default)]
pub(crate) struct Responses {
    buffer: Vec<u8>,
    methods: VecDeque<bool>,
    failed: bool,
    overflow: bool,
}
impl Responses {
    pub fn request(&mut self, method: &str) {
        if self.methods.len() == 128 {
            self.overflow = true;
            self.methods.clear();
        } else if !self.failed && !self.overflow {
            self.methods.push_back(method == "HEAD");
        }
    }
    pub fn feed(&mut self, bytes: &[u8], eof: bool) -> Result<Vec<HttpRequest>, DecodeError> {
        if self.overflow {
            self.overflow = false;
            self.failed = true;
            self.buffer.clear();
            return Err(error("HTTP outstanding request inspection limit"));
        }
        if self.failed {
            return Ok(vec![]);
        }
        let result = self.decode(bytes, eof);
        if result.is_err() {
            self.failed = true;
            self.buffer.clear();
            self.methods.clear();
        }
        result
    }
    fn decode(&mut self, bytes: &[u8], eof: bool) -> Result<Vec<HttpRequest>, DecodeError> {
        if self.buffer.len().saturating_add(bytes.len()) > LIMIT {
            return Err(error("HTTP response inspection limit"));
        }
        self.buffer.extend_from_slice(bytes);
        let mut output = vec![];
        while let Some(end) = delimiter(&self.buffer, b"\r\n\r\n").map(|p| p + 4) {
            if end > 16_384 {
                return Err(error("HTTP response header limit"));
            }
            let header = std::str::from_utf8(&self.buffer[..end])
                .map_err(|_| error("HTTP response header encoding"))?;
            let mut lines = header.split("\r\n");
            let mut status = lines.next().unwrap_or("").splitn(3, ' ');
            if !matches!(status.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
                return Err(error("HTTP response version"));
            }
            let code = status.next().unwrap_or("");
            if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
                return Err(error("HTTP response status"));
            }
            let code: u16 = code.parse().map_err(|_| error("HTTP response status"))?;
            if !(100..=599).contains(&code) {
                return Err(error("HTTP response status"));
            }
            let reason = status.next().unwrap_or("");
            if reason.bytes().any(|b| b < 32 || b == 127) {
                return Err(error("HTTP response reason"));
            }
            let mut fields = BTreeMap::from([
                ("http.status_code".into(), Value::Unsigned(code.into())),
                ("http.reason".into(), Value::String(reason.into())),
            ]);
            let mut headers = Vec::new();
            let mut length = None;
            let mut chunked = false;
            let mut content_type = "";
            let mut encoded = false;
            for line in lines.filter(|line| !line.is_empty()) {
                let (name, value) = line
                    .split_once(':')
                    .ok_or_else(|| error("HTTP response header syntax"))?;
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                    || value.bytes().any(|b| b < 32 && b != 9 || b == 127)
                {
                    return Err(error("HTTP response header syntax"));
                }
                // Preserve observed spelling, order, duplicates and whitespace separately
                // from the normalized view used to validate message framing.
                if headers.len() == 128 {
                    return Err(DecodeError("HTTP header count limit".into()));
                }
                headers.push(Value::Object(BTreeMap::from([
                    ("name".into(), Value::String(name.into())),
                    ("value".into(), Value::String(value.into())),
                ])));
                let value = value.trim();
                if name.eq_ignore_ascii_case("content-length") {
                    if length.is_some()
                        || value.is_empty()
                        || !value.bytes().all(|b| b.is_ascii_digit())
                    {
                        return Err(error("ambiguous HTTP response length"));
                    }
                    length = Some(
                        value
                            .parse::<usize>()
                            .map_err(|_| error("HTTP response length"))?,
                    );
                } else if name.eq_ignore_ascii_case("transfer-encoding") {
                    if chunked || !value.eq_ignore_ascii_case("chunked") {
                        return Err(error("unsupported HTTP response transfer encoding"));
                    }
                    chunked = true;
                } else if name.eq_ignore_ascii_case("content-type") {
                    content_type = value;
                } else if name.eq_ignore_ascii_case("content-encoding") {
                    encoded = !value.eq_ignore_ascii_case("identity");
                }
            }
            fields.insert("http.headers".into(), Value::List(headers));
            if chunked && length.is_some() {
                return Err(error("ambiguous HTTP response framing"));
            }
            let no_body = code < 200
                || code == 204
                || code == 304
                || self.methods.front().copied().unwrap_or(false);
            let (consumed, body, trailers) = if no_body {
                (0, Vec::new(), None)
            } else if chunked {
                let Some(frame) = chunks(&self.buffer[end..])? else {
                    break;
                };
                (frame.consumed, frame.body, Some(frame.trailers))
            } else if let Some(length) = length {
                if length > LIMIT - end {
                    return Err(error("HTTP response body inspection limit"));
                }
                if self.buffer.len() < end + length {
                    break;
                }
                (length, self.buffer[end..end + length].to_vec(), None)
            } else if eof {
                (self.buffer.len() - end, self.buffer[end..].to_vec(), None)
            } else {
                break;
            };
            if let Some(trailers) = trailers {
                fields.insert("http.trailers".into(), Value::List(trailers));
            }
            fields.insert(
                "http.body_length".into(),
                Value::Unsigned(body.len() as u64),
            );
            fields.insert(
                "http.body_preview_truncated".into(),
                Value::Bool(body.len() > PREVIEW),
            );
            fields.insert(
                "http.content_type".into(),
                Value::String(content_type.into()),
            );
            let media = content_type
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !encoded
                && (media.starts_with("text/")
                    || media == "application/json"
                    || media.ends_with("+json")
                    || media == "application/xml"
                    || media.ends_with("+xml"))
            {
                // UTF-8 only; compressed/binary bodies are never misrepresented as text.
                if let Ok(text) = std::str::from_utf8(&body) {
                    let mut bound = text.len().min(PREVIEW);
                    while !text.is_char_boundary(bound) {
                        bound -= 1;
                    }
                    fields.insert("http.body_text".into(), Value::String(text[..bound].into()));
                }
            }
            if code >= 200 {
                self.methods.pop_front();
            }
            let bytes = self.buffer.drain(..end + consumed).collect();
            output.push(HttpRequest {
                bytes,
                message: ParsedMessage {
                    protocol: "http1".into(),
                    message_type: "response".into(),
                    direction: Direction::ServerToClient,
                    fields,
                },
            });
            if code == 101 {
                self.failed = true;
                self.buffer.clear();
                break;
            }
        }
        if self.buffer.len() > 16_384 && delimiter(&self.buffer, b"\r\n\r\n").is_none() {
            return Err(error("HTTP response header limit"));
        }
        if eof && !self.buffer.is_empty() {
            return Err(error("incomplete HTTP response"));
        }
        Ok(output)
    }
}
fn error(message: &str) -> DecodeError {
    DecodeError(message.into())
}
fn delimiter(bytes: &[u8], pattern: &[u8]) -> Option<usize> {
    bytes.windows(pattern.len()).position(|w| w == pattern)
}
struct ChunkFrame {
    consumed: usize,
    body: Vec<u8>,
    trailers: Vec<Value>,
}
/// Decode chunk framing and retain trailers separately from the entity body.
fn chunks(bytes: &[u8]) -> Result<Option<ChunkFrame>, DecodeError> {
    let mut offset = 0;
    let mut body = Vec::new();
    loop {
        let Some(line_end) = delimiter(&bytes[offset..], b"\r\n") else {
            return Ok(None);
        };
        if line_end > 128 {
            return Err(error("HTTP chunk line limit"));
        }
        let size = std::str::from_utf8(&bytes[offset..offset + line_end])
            .map_err(|_| error("HTTP chunk size"))?
            .split(';')
            .next()
            .unwrap_or("");
        if size.is_empty() || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error("HTTP chunk size"));
        }
        let size = usize::from_str_radix(size, 16).map_err(|_| error("HTTP chunk size"))?;
        offset += line_end + 2;
        if size == 0 {
            if bytes[offset..].starts_with(b"\r\n") {
                return Ok(Some(ChunkFrame {
                    consumed: offset + 2,
                    body,
                    trailers: vec![],
                }));
            }
            let Some(end) = delimiter(&bytes[offset..], b"\r\n\r\n").map(|end| end + 4) else {
                if bytes.len() - offset > 16_384 {
                    return Err(error("HTTP response trailer limit"));
                }
                return Ok(None);
            };
            if end > 16_384 {
                return Err(error("HTTP response trailer limit"));
            }
            let trailer_block = std::str::from_utf8(&bytes[offset..offset + end - 4])
                .map_err(|_| error("HTTP response trailer encoding"))?;
            let mut trailers = Vec::new();
            for line in trailer_block.split("\r\n") {
                let (name, value) = line
                    .split_once(':')
                    .ok_or_else(|| error("HTTP response trailer syntax"))?;
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                    || value.bytes().any(|b| b < 32 && b != 9 || b == 127)
                {
                    return Err(error("HTTP response trailer syntax"));
                }
                if trailers.len() == 128 {
                    return Err(error("HTTP response trailer count limit"));
                }
                trailers.push(Value::Object(BTreeMap::from([
                    ("name".into(), Value::String(name.into())),
                    ("value".into(), Value::String(value.into())),
                ])));
            }
            return Ok(Some(ChunkFrame {
                consumed: offset + end,
                body,
                trailers,
            }));
        }
        if size > LIMIT.saturating_sub(offset + 2) {
            return Err(error("HTTP chunk inspection limit"));
        }
        if bytes.len() < offset + size + 2 {
            return Ok(None);
        }
        if &bytes[offset + size..offset + size + 2] != b"\r\n" {
            return Err(error("HTTP chunk terminator"));
        }
        body.extend_from_slice(&bytes[offset..offset + size]);
        offset += size + 2;
    }
}
