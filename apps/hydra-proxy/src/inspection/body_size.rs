//! Streaming byte accounting. Body payload is never retained here.
use hydra_domain::FieldValue;
use hydra_protocol_sdk::{Direction, ParsedMessage, Value};
use std::collections::BTreeMap;

const MAX_HEADER: usize = 16_384;
type Fields = BTreeMap<String, FieldValue>;

#[derive(Default)]
pub struct BodySizes {
    transferred: [u64; 2],
    current: [u64; 2],
    http: Option<HttpBody>,
}
impl BodySizes {
    pub fn summary(&self, direction: Direction, scope: &str) -> Option<(ParsedMessage, usize)> {
        let index = usize::from(direction == Direction::ServerToClient);
        let total = self.transferred[index];
        if total == 0 {
            return None;
        }
        let mut message = ParsedMessage {
            protocol: "transfer".into(),
            message_type: "byte_summary".into(),
            direction,
            fields: BTreeMap::new(),
        };
        self.annotate(&mut message, scope);
        if index == 0
            && let Some(http) = &self.http
        {
            for key in ["http.method", "http.path", "http.user_agent"] {
                if let Some(FieldValue::String(value)) = http.fields.get(key) {
                    message
                        .fields
                        .insert(key.into(), Value::String(value.clone()));
                }
            }
            message
                .fields
                .insert("body.size_scope".into(), Value::String("http-body".into()));
            message.fields.insert(
                "http.body_length".into(),
                Value::Unsigned(self.current[index]),
            );
        }
        Some((message, usize::try_from(total).unwrap_or(usize::MAX)))
    }
    pub fn set_http(&mut self, enabled: bool) {
        self.http = enabled.then(HttpBody::default);
    }
    pub fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
        mut check: impl FnMut(&Fields) -> bool,
    ) -> bool {
        let index = usize::from(direction == Direction::ServerToClient);
        self.transferred[index] = self.transferred[index].saturating_add(bytes.len() as u64);
        let transferred = self.transferred[index];
        if index == 0
            && let Some(http) = self.http.as_mut()
        {
            let outcome = http.feed(bytes, &mut |fields, size| {
                self.current[index] = size;
                let mut fields = fields.clone();
                add_sizes(&mut fields, size, transferred, "http-body");
                fields.insert("http.body_length".into(), FieldValue::Uint(size));
                check(&fields)
            });
            match outcome {
                Ok(blocked) => return blocked,
                Err(()) => self.http = None,
            }
        }
        self.current[index] = transferred;
        let mut fields = Fields::new();
        add_sizes(&mut fields, transferred, transferred, "stream");
        check(&fields)
    }
    pub fn annotate(&self, message: &mut ParsedMessage, fallback_scope: &str) {
        let index = usize::from(message.direction == Direction::ServerToClient);
        let (size, scope) = match message.fields.get("http.body_length") {
            Some(Value::Unsigned(size)) => (*size, "http-body"),
            _ => (self.current[index], fallback_scope),
        };
        message
            .fields
            .insert("body.size".into(), Value::Unsigned(size));
        message.fields.insert(
            "transfer.bytes".into(),
            Value::Unsigned(self.transferred[index]),
        );
        message
            .fields
            .insert("body.size_scope".into(), Value::String(scope.into()));
        message.fields.insert(
            "direction".into(),
            Value::String(
                if index == 0 {
                    "client_to_server"
                } else {
                    "server_to_client"
                }
                .into(),
            ),
        );
    }
    pub fn datagram(&mut self, direction: Direction, bytes: usize) -> Fields {
        let index = usize::from(direction == Direction::ServerToClient);
        self.transferred[index] = self.transferred[index].saturating_add(bytes as u64);
        self.current[index] = bytes as u64;
        let mut fields = Fields::new();
        add_sizes(
            &mut fields,
            bytes as u64,
            self.transferred[index],
            "datagram",
        );
        fields
    }
}
fn add_sizes(fields: &mut Fields, size: u64, transferred: u64, scope: &str) {
    fields.insert("body.size".into(), FieldValue::Uint(size));
    fields.insert("transfer.bytes".into(), FieldValue::Uint(transferred));
    fields.insert("body.size_scope".into(), FieldValue::String(scope.into()));
}

#[derive(Default)]
struct HttpBody {
    buffer: Vec<u8>,
    fields: Fields,
    size: u64,
    state: State,
    trailer_bytes: usize,
}
#[derive(Default)]
enum State {
    #[default]
    Headers,
    Fixed(u64),
    ChunkHeader,
    Chunk(u64),
    ChunkEnd(u8),
    Trailers,
}
impl HttpBody {
    fn feed(
        &mut self,
        bytes: &[u8],
        check: &mut impl FnMut(&Fields, u64) -> bool,
    ) -> Result<bool, ()> {
        let mut offset = 0;
        while offset < bytes.len() {
            match self.state {
                State::Headers | State::ChunkHeader | State::Trailers => {
                    self.buffer.push(bytes[offset]);
                    offset += 1;
                    if self.buffer.len() > MAX_HEADER {
                        return Err(());
                    }
                    if matches!(self.state, State::Headers) {
                        if !self.buffer.ends_with(b"\r\n\r\n") {
                            continue;
                        }
                        self.header()?;
                        self.buffer.clear();
                        if check(&self.fields, self.size) {
                            return Ok(true);
                        }
                        if matches!(self.state, State::Fixed(0)) {
                            self.state = State::Headers;
                        }
                    } else if self.buffer.ends_with(b"\r\n") {
                        if matches!(self.state, State::ChunkHeader) {
                            let text = std::str::from_utf8(&self.buffer[..self.buffer.len() - 2])
                                .map_err(|_| ())?;
                            let number = text.split(';').next().ok_or(())?;
                            if number.is_empty()
                                || number.len() > 16
                                || !number.bytes().all(|b| b.is_ascii_hexdigit())
                            {
                                return Err(());
                            }
                            let length = u64::from_str_radix(number, 16).map_err(|_| ())?;
                            self.state = if length == 0 {
                                self.trailer_bytes = 0;
                                State::Trailers
                            } else {
                                State::Chunk(length)
                            };
                        } else {
                            self.trailer_bytes =
                                self.trailer_bytes.saturating_add(self.buffer.len());
                            if self.trailer_bytes > MAX_HEADER {
                                return Err(());
                            }
                            if self.buffer.len() == 2 {
                                self.state = State::Headers;
                            }
                        }
                        self.buffer.clear();
                    }
                }
                State::Fixed(remaining) | State::Chunk(remaining) => {
                    let take = remaining.min((bytes.len() - offset) as u64);
                    self.size = self.size.saturating_add(take);
                    offset += take as usize;
                    let chunked = matches!(self.state, State::Chunk(_));
                    self.state = if remaining == take {
                        if chunked {
                            State::ChunkEnd(0)
                        } else {
                            State::Headers
                        }
                    } else if chunked {
                        State::Chunk(remaining - take)
                    } else {
                        State::Fixed(remaining - take)
                    };
                    if check(&self.fields, self.size) {
                        return Ok(true);
                    }
                }
                State::ChunkEnd(position) => {
                    if bytes[offset] != b"\r\n"[position as usize] {
                        return Err(());
                    }
                    offset += 1;
                    self.state = if position == 0 {
                        State::ChunkEnd(1)
                    } else {
                        State::ChunkHeader
                    };
                }
            }
        }
        Ok(false)
    }
    fn header(&mut self) -> Result<(), ()> {
        let text = std::str::from_utf8(&self.buffer).map_err(|_| ())?;
        let mut lines = text.split("\r\n");
        let mut first = lines.next().ok_or(())?.split(' ');
        let method = first.next().ok_or(())?;
        let path = first.next().ok_or(())?;
        if method.is_empty()
            || !method.bytes().all(|b| b.is_ascii_uppercase())
            || !path.starts_with('/')
            || path.bytes().any(|b| b <= 32 || b == 127)
            || !matches!(first.next(), Some("HTTP/1.1" | "HTTP/1.0"))
            || first.next().is_some()
        {
            return Err(());
        }
        let mut fields = Fields::from([
            ("http.method".into(), FieldValue::String(method.into())),
            ("http.path".into(), FieldValue::String(path.into())),
        ]);
        let mut length = None;
        let mut chunked = false;
        for line in lines.filter(|line| !line.is_empty()) {
            let (name, value) = line.split_once(':').ok_or(())?;
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || value.bytes().any(|b| b < 32 && b != 9)
            {
                return Err(());
            }
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some()
                    || value.is_empty()
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(());
                }
                length = Some(value.parse::<u64>().map_err(|_| ())?);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                if chunked || !value.eq_ignore_ascii_case("chunked") {
                    return Err(());
                }
                chunked = true;
            } else if name.eq_ignore_ascii_case("user-agent") {
                fields.insert("http.user_agent".into(), FieldValue::String(value.into()));
            }
        }
        if chunked && length.is_some() {
            return Err(());
        }
        self.fields = fields;
        self.size = 0;
        self.state = if chunked {
            State::ChunkHeader
        } else {
            State::Fixed(length.unwrap_or(0))
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn count(fields: &Fields, key: &str) -> u64 {
        match fields.get(key) {
            Some(FieldValue::Uint(n)) => *n,
            other => panic!("missing {key}: {other:?}"),
        }
    }
    #[test]
    fn raw_stream_limit_crosses_read_boundaries_and_is_directional() {
        let mut sizes = BodySizes::default();
        for _ in 0..32 {
            assert!(
                !sizes.feed(Direction::ClientToServer, &[0xff; 16_384], |f| count(
                    f,
                    "body.size"
                ) > 524_288)
            );
        }
        assert!(!sizes.feed(Direction::ServerToClient, b"reply", |f| {
            assert_eq!(count(f, "body.size"), 5);
            false
        }));
        assert!(sizes.feed(Direction::ClientToServer, &[0xff], |f| count(
            f,
            "body.size"
        ) > 524_288));
    }
    #[test]
    fn large_http_bodies_are_counted_live_without_retaining_them() {
        let mut sizes = BodySizes::default();
        sizes.set_http(true);
        assert!(!sizes.feed(
            Direction::ClientToServer,
            b"POST /upload HTTP/1.1\r\nContent-Length: 2000000\r\n\r\n",
            |f| {
                assert_eq!(count(f, "body.size"), 0);
                false
            }
        ));
        for _ in 0..32 {
            assert!(
                !sizes.feed(Direction::ClientToServer, &[0xff; 16_384], |f| count(
                    f,
                    "body.size"
                ) > 524_288)
            );
        }
        assert!(sizes.feed(Direction::ClientToServer, &[0xff], |f| {
            assert_eq!(f["http.path"], FieldValue::String("/upload".into()));
            count(f, "body.size") > 524_288
        }));
        assert!(sizes.http.as_ref().unwrap().buffer.capacity() <= MAX_HEADER);
    }
    #[test]
    fn chunked_sizes_exclude_framing_and_reset_for_pipelined_requests() {
        let mut sizes = BodySizes::default();
        sizes.set_http(true);
        let mut seen = vec![];
        for part in [
            b"POST /a HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n".as_slice(),
            b"\xff\x00a\r\n2\r\nbc\r\n0\r\n\r\nPOST /b HTTP/1.1\r\nContent-Length: 1\r\n\r\nx",
        ] {
            assert!(!sizes.feed(Direction::ClientToServer, part, |f| {
                seen.push((f["http.path"].clone(), count(f, "body.size")));
                false
            }));
        }
        assert!(seen.contains(&(FieldValue::String("/a".into()), 5)));
        assert_eq!(seen.last(), Some(&(FieldValue::String("/b".into()), 1)));
    }
}
