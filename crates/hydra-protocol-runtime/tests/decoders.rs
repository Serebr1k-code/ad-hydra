use hydra_protocol_runtime::*;
use sha2::{Digest, Sha256};
fn fixture(decode: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module (memory (export "memory") 1 64)
    (data (i32.const 1024) "{{\22messages\22:[],\22state\22:[]}}")
    (func (export "hydra_abi_version") (result i32) i32.const 1)
    (func (export "hydra_alloc") (param i32) (result i32) i32.const 2048)
    (func (export "hydra_dealloc") (param i32 i32 i32 i32))
    (func (export "hydra_decode") (param i32 i32) (result i64) {decode}))"#
    ))
    .unwrap()
}
#[test]
fn real_wasm_and_snapshot_retention() {
    let bytes = fixture("i64.const 4398046511130"); // 1024 << 32 | 26
    let version = PluginVersion::load(&bytes, Sha256::digest(&bytes).into()).unwrap();
    let mut registry = PluginRegistry::default();
    registry.activate("custom".into(), version.clone()).unwrap();
    let mut session = WasmSession::new(registry.get("custom").unwrap()).unwrap();
    assert!(
        session
            .feed(Direction::ClientToServer, b"test")
            .unwrap()
            .is_empty()
    );
    let other = fixture("unreachable");
    let new = PluginVersion::load(&other, Sha256::digest(&other).into()).unwrap();
    registry.activate("custom".into(), new).unwrap();
    assert!(std::sync::Arc::ptr_eq(session.version(), &version));
    assert!(session.feed(Direction::ClientToServer, b"test").is_ok());
}
#[test]
fn fuel_and_malformed_are_errors() {
    for decode in ["(loop br 0) i64.const 0", "i64.const 0", "unreachable"] {
        let bytes = fixture(decode);
        let v = PluginVersion::load(&bytes, Sha256::digest(&bytes).into()).unwrap();
        let mut s = WasmSession::new(v).unwrap();
        assert!(s.feed(Direction::ClientToServer, b"a").is_err());
        assert!(s.feed(Direction::ClientToServer, b"a").is_err());
    }
    assert!(PluginVersion::load(b"broken", [0; 32]).is_err());
}
#[test]
fn requests_fragment_body_pipeline_and_exact_bytes() {
    let mut d = HttpDecoder::default();
    assert!(
        d.feed(
            Direction::ClientToServer,
            b"POST /a HTTP/1.1\r\nContent-Length: 3\r\nUser-Agent: Hydra\r\n\r\na"
        )
        .unwrap()
        .is_empty()
    );
    let requests = d
        .feed(Direction::ClientToServer, b"bcGET /b HTTP/1.1\r\n\r\n")
        .unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].bytes.ends_with(b"abc"));
    assert_eq!(
        requests[0].message.fields["http.user_agent"],
        Value::String("Hydra".into())
    );
    assert_eq!(
        requests[0].message.fields["http.body_text"],
        Value::String("abc".into())
    );
    assert!(!d.has_pending());
}
#[test]
fn ambiguous_http_fails_and_raw_has_no_fake_messages() {
    let mut d = HttpDecoder::default();
    assert!(
        d.feed(
            Direction::ClientToServer,
            b"GET / HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n"
        )
        .is_err()
    );
    assert_eq!(
        SessionDecoder::Raw(hydra_protocol_runtime::RawDecoder::default())
            .feed(Direction::ClientToServer, b"opaque\0")
            .unwrap()[0]
            .fields["raw.bytes"],
        hydra_protocol_runtime::Value::Bytes(b"opaque\0".to_vec())
    );
}

#[test]
fn utf8_decoder_handles_fragmented_and_invalid_text_without_losing_data() {
    let mut decoder = SessionDecoder::Utf8(hydra_protocol_runtime::Utf8Decoder::default());
    assert!(
        decoder
            .feed(Direction::ClientToServer, &[0xe2, 0x82])
            .unwrap()
            .is_empty()
    );
    let messages = decoder
        .feed(Direction::ClientToServer, &[0xac, b' ', 0xff, b'!'])
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].protocol, "utf8");
    assert_eq!(messages[0].message_type, "text");
    assert_eq!(messages[0].direction, Direction::ClientToServer);
    assert_eq!(
        messages[0].fields["utf8.text"],
        Value::String("€ �!".into())
    );
    let other = decoder.feed(Direction::ServerToClient, b"reply").unwrap();
    assert_eq!(other[0].direction, Direction::ServerToClient);
    assert_eq!(other[0].fields["utf8.text"], Value::String("reply".into()));
}

#[test]
fn utf8_decoder_keeps_fragment_tails_separate_between_directions() {
    let mut decoder = SessionDecoder::Utf8(hydra_protocol_runtime::Utf8Decoder::default());
    assert!(
        decoder
            .feed(Direction::ClientToServer, &[0xe2, 0x82])
            .unwrap()
            .is_empty()
    );
    let response = decoder.feed(Direction::ServerToClient, b"reply").unwrap();
    assert_eq!(
        response[0].fields["utf8.text"],
        Value::String("reply".into())
    );
    let request = decoder.feed(Direction::ClientToServer, &[0xac]).unwrap();
    assert_eq!(request[0].fields["utf8.text"], Value::String("€".into()));
}

#[test]
fn raw_bytes_decoder_retains_only_bounded_overlap_and_matches_across_reads() {
    use hydra_protocol_runtime::{ProtocolDecoder, RawDecoder};
    use hydra_protocol_sdk::{Direction, Value};

    let mut decoder = RawDecoder::default();
    assert!(
        decoder
            .field_schema()
            .iter()
            .any(|field| field.path == "raw.bytes")
    );
    let first = decoder
        .feed(Direction::ClientToServer, b"prefix\xde\xad")
        .unwrap();
    assert_eq!(
        first[0].fields["raw.bytes"],
        Value::Bytes(b"prefix\xde\xad".to_vec())
    );
    let second = decoder
        .feed(Direction::ClientToServer, b"\xbe\xefsuffix")
        .unwrap();
    assert!(matches!(
        &second[0].fields["raw.bytes"],
        Value::Bytes(bytes) if bytes.windows(4).any(|window| window == [0xde, 0xad, 0xbe, 0xef])
    ));
    assert!(
        decoder.feed(Direction::ServerToClient, b"reply").unwrap()[0]
            .fields
            .contains_key("raw.bytes")
    );
}

#[test]
fn built_in_decoders_share_a_typed_schema_interface() {
    use hydra_protocol_runtime::{HttpDecoder, ProtocolDecoder, Utf8Decoder};

    fn paths(decoder: &dyn ProtocolDecoder) -> Vec<String> {
        decoder
            .field_schema()
            .iter()
            .map(|field| field.path.clone())
            .collect()
    }

    assert!(paths(&HttpDecoder::default()).contains(&"http.path".into()));
    assert!(paths(&Utf8Decoder::default()).contains(&"utf8.text".into()));
}

#[test]
fn utf8_decoder_splits_large_reads_into_bounded_text_events() {
    let mut decoder = SessionDecoder::Utf8(hydra_protocol_runtime::Utf8Decoder::default());
    let input = vec![b'a'; 16 * 1024];
    let messages = decoder.feed(Direction::ClientToServer, &input).unwrap();
    assert!(messages.len() > 1);
    assert!(messages.len() <= 8);
    let joined = messages
        .iter()
        .map(|message| match &message.fields["utf8.text"] {
            Value::String(value) => value.as_str(),
            _ => panic!("text field must be a string"),
        })
        .collect::<String>();
    assert_eq!(joined, "a".repeat(16 * 1024));
    assert!(messages.iter().all(|m| match &m.fields["utf8.text"] {
        Value::String(value) => value.len() <= 4096,
        _ => false,
    }));
}
#[test]
fn failed_pipeline_recovers_every_unforwarded_byte() {
    let bytes = b"GET /a HTTP/1.1\r\n\r\nGET /bad HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
    let mut decoder = HttpDecoder::default();
    assert!(decoder.feed(Direction::ClientToServer, bytes).is_err());
    assert_eq!(decoder.take_pending(), bytes);
}
#[test]
fn real_wasm_typed_custom_output() {
    let output = r#"{"messages":[{"protocol":"custom","message_type":"frame","direction":"ClientToServer","fields":{"payload":{"type":"bytes","value":[97,98]}}}],"state":[1]}"#;
    let escaped = output
        .bytes()
        .map(|b| format!("\\{b:02x}"))
        .collect::<String>();
    let packed = (1024_u64 << 32) | output.len() as u64;
    let wasm = wat::parse_str(format!(
        r#"(module (memory (export "memory") 1 64)
      (data (i32.const 1024) "{escaped}")
      (func (export "hydra_abi_version") (result i32) i32.const 1)
      (func (export "hydra_alloc") (param i32) (result i32) i32.const 2048)
      (func (export "hydra_dealloc") (param i32 i32 i32 i32))
      (func (export "hydra_decode") (param i32 i32) (result i64) i64.const {packed}))"#
    ))
    .unwrap();
    let plugin = PluginVersion::load(&wasm, Sha256::digest(&wasm).into()).unwrap();
    let mut session = WasmSession::new(plugin).unwrap();
    let messages = session.feed(Direction::ClientToServer, b"ab").unwrap();
    assert_eq!(messages[0].fields["payload"], Value::Bytes(b"ab".to_vec()));
}
#[test]
fn registry_is_bounded_and_checksum_is_required() {
    let bytes = fixture("i64.const 4398046511130");
    assert!(PluginVersion::load(&bytes, [0; 32]).is_err());
    let version = PluginVersion::load(&bytes, Sha256::digest(&bytes).into()).unwrap();
    let mut registry = PluginRegistry::default();
    for n in 0..16 {
        registry.activate(n.to_string(), version.clone()).unwrap();
    }
    assert!(registry.activate("overflow".into(), version).is_err());
}
#[test]
fn available_keeps_valid_prefix_separate_from_malformed_remainder() {
    let prefix = b"GET /blocked HTTP/1.1\r\n\r\n";
    let remainder = b"GET /bad HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
    let input = [prefix.as_slice(), remainder.as_slice()].concat();
    let mut decoder = HttpDecoder::default();
    let batch = decoder.feed_available(Direction::ClientToServer, &input);
    assert!(batch.failure.is_some());
    assert_eq!(batch.requests.len(), 1);
    assert_eq!(batch.requests[0].bytes, prefix);
    assert_eq!(
        batch.requests[0].message.fields["http.path"],
        Value::String("/blocked".into())
    );
    assert_eq!(decoder.take_pending(), remainder);
}
#[test]
fn expect_continue_falls_back_before_waiting_for_body() {
    let headers = b"POST /upload HTTP/1.1\r\nContent-Length: 12\r\nExpect: 100-continue\r\n\r\n";
    let mut decoder = HttpDecoder::default();
    let batch = decoder.feed_available(Direction::ClientToServer, headers);
    assert!(batch.failure.is_some());
    assert!(batch.requests.is_empty());
    assert_eq!(decoder.take_pending(), headers);
}
#[test]
fn optional_metadata_is_typed_bounded_and_legacy_compatible() {
    let bytes = fixture("i64.const 4398046511130");
    let plugin = PluginVersion::load(&bytes, Sha256::digest(&bytes).into()).unwrap();
    assert!(
        WasmSession::new(plugin)
            .unwrap()
            .metadata()
            .unwrap()
            .is_none()
    );
    for (metadata, accepted) in [
        (
            r#"{"name":"demo","fields":[{"path":"payload","field_type":"bytes"}]}"#,
            true,
        ),
        (
            r#"{"name":"demo","fields":[{"path":"payload","field_type":"unknown"}]}"#,
            false,
        ),
    ] {
        let escaped = metadata
            .bytes()
            .map(|b| format!("\\{b:02x}"))
            .collect::<String>();
        let packed = (1024u64 << 32) | metadata.len() as u64;
        let wasm = wat::parse_str(format!(
            r#"(module (memory (export "memory") 1 64)
        (data (i32.const 1024) "{escaped}")
        (func (export "hydra_abi_version") (result i32) i32.const 1)
        (func (export "hydra_alloc") (param i32) (result i32) i32.const 2048)
        (func (export "hydra_dealloc") (param i32 i32 i32 i32))
        (func (export "hydra_decode") (param i32 i32) (result i64) i64.const 0)
        (func (export "hydra_metadata") (result i64) i64.const {packed}))"#
        ))
        .unwrap();
        let result = PluginVersion::load(&wasm, Sha256::digest(&wasm).into());
        assert_eq!(result.is_ok(), accepted);
        if let Ok(plugin) = result {
            assert_eq!(
                WasmSession::new(plugin)
                    .unwrap()
                    .metadata()
                    .unwrap()
                    .unwrap()
                    .fields[0]
                    .field_type,
                hydra_protocol_sdk::FieldType::Bytes
            );
        }
    }
}

#[test]
fn server_response_fragmented_text_is_indexed_without_consuming_pending_request() {
    let mut d = HttpDecoder::default();
    d.feed(Direction::ClientToServer, b"GET / HTTP/1.1\r\n\r\n")
        .unwrap();
    d.feed(Direction::ClientToServer, b"GET /next HT").unwrap();
    assert!(
        d.feed(
            Direction::ServerToClient,
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhe"
        )
        .unwrap()
        .is_empty()
    );
    let messages = d.feed(Direction::ServerToClient, b"llo").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message.direction, Direction::ServerToClient);
    assert_eq!(
        messages[0].message.fields["http.status_code"],
        Value::Unsigned(200)
    );
    assert_eq!(
        messages[0].message.fields["http.body_text"],
        Value::String("hello".into())
    );
    assert_eq!(d.take_pending(), b"GET /next HT");
}

#[test]
fn response_pipeline_head_informational_chunked_and_close_delimited() {
    let mut d = HttpDecoder::default();
    d.feed(
        Direction::ClientToServer,
        b"HEAD / HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\n",
    )
    .unwrap();
    let frames = d.feed(Direction::ServerToClient, b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nHTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2;ext=yes\r\n{}\r\n0\r\nX-Trailer: value\r\n\r\nHTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\nbye").unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(
        frames[0].message.fields["http.body_length"],
        Value::Unsigned(0)
    );
    assert_eq!(
        frames[1].message.fields["http.status_code"],
        Value::Unsigned(100)
    );
    assert_eq!(
        frames[2].message.fields["http.body_text"],
        Value::String("{}".into())
    );
    let final_frame = d.finish_response().unwrap();
    assert_eq!(
        final_frame[0].message.fields["http.body_text"],
        Value::String("bye".into())
    );
}

#[test]
fn response_failure_does_not_disable_request_inspection() {
    let mut d = HttpDecoder::default();
    assert!(
        d.feed(
            Direction::ServerToClient,
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n"
        )
        .is_err()
    );
    assert_eq!(
        d.feed(
            Direction::ClientToServer,
            b"GET /still-works HTTP/1.1\r\n\r\n"
        )
        .unwrap()
        .len(),
        1
    );
    assert!(
        d.feed(Direction::ServerToClient, b"opaque")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn response_preview_bounded_utf8_and_binary_not_logged() {
    for (content_type, encoding, expected) in [
        ("text/plain", "identity", true),
        ("application/octet-stream", "identity", false),
        ("text/plain", "gzip", false),
    ] {
        let mut d = HttpDecoder::default();
        let body = "€".repeat(2000);
        let input = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Encoding: {encoding}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let frames = d.feed(Direction::ServerToClient, input.as_bytes()).unwrap();
        let fields = &frames[0].message.fields;
        assert_eq!(fields["http.body_length"], Value::Unsigned(6000));
        assert_eq!(fields["http.body_preview_truncated"], Value::Bool(true));
        assert_eq!(fields.contains_key("http.body_text"), expected);
        if expected {
            assert!(matches!(&fields["http.body_text"], Value::String(s) if s.len() == 4095));
        }
    }
    let mut d = HttpDecoder::default();
    assert!(
        d.feed(
            Direction::ServerToClient,
            b"HTTP/1.1 200 OK\r\nContent-Length: 999999999999\r\n\r\n"
        )
        .is_err()
    );
}

fn header(name: &str, value: &str) -> Value {
    Value::Object(std::collections::BTreeMap::from([
        ("name".into(), Value::String(name.into())),
        ("value".into(), Value::String(value.into())),
    ]))
}

#[test]
fn request_and_response_headers_preserve_order_duplicates_case_and_whitespace() {
    let mut decoder = HttpDecoder::default();
    let request = b"GET / HTTP/1.1\r\nHost: hydra.test\r\nX-Custom:\t first \t\r\nx-custom:second\r\nCookie: session=observed\r\n\r\n";
    let messages = decoder.feed(Direction::ClientToServer, request).unwrap();
    assert_eq!(messages[0].bytes, request);
    assert_eq!(
        messages[0].message.fields["http.headers"],
        Value::List(vec![
            header("Host", " hydra.test"),
            header("X-Custom", "\t first \t"),
            header("x-custom", "second"),
            header("Cookie", " session=observed"),
        ])
    );
    let response = b"HTTP/1.1 200 OK\r\nSet-Cookie: a=one\r\nset-cookie:\tb=two \r\nX-Empty:\r\nContent-Length: 0\r\n\r\n";
    let messages = decoder.feed(Direction::ServerToClient, response).unwrap();
    assert_eq!(messages[0].bytes, response);
    assert_eq!(
        messages[0].message.fields["http.headers"],
        Value::List(vec![
            header("Set-Cookie", " a=one"),
            header("set-cookie", "\tb=two "),
            header("X-Empty", ""),
            header("Content-Length", " 0"),
        ])
    );
}

#[test]
fn headers_have_explicit_byte_and_count_bounds_in_both_directions() {
    for direction in [Direction::ClientToServer, Direction::ServerToClient] {
        let start = if direction == Direction::ClientToServer {
            "GET / HTTP/1.1\r\n"
        } else {
            "HTTP/1.1 204 No Content\r\n"
        };
        let fixed = start.len() + "X:\r\n\r\n".len();
        let accepted = format!("{start}X:{}\r\n\r\n", "v".repeat(16_384 - fixed));
        let mut decoder = HttpDecoder::default();
        let messages = decoder.feed(direction, accepted.as_bytes()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].message.fields["http.headers"],
            Value::List(vec![header("X", &"v".repeat(16_384 - fixed))])
        );
        let oversized = format!("{start}X:{}\r\n\r\n", "v".repeat(16_385 - fixed));
        assert!(
            HttpDecoder::default()
                .feed(direction, oversized.as_bytes())
                .is_err()
        );
        let accepted = format!("{start}{}\r\n", "X:v\r\n".repeat(128));
        let messages = HttpDecoder::default()
            .feed(direction, accepted.as_bytes())
            .unwrap();
        assert!(
            matches!(&messages[0].message.fields["http.headers"], Value::List(headers) if headers.len()==128)
        );
        let excessive = format!("{start}{}\r\n", "X:v\r\n".repeat(129));
        assert!(
            HttpDecoder::default()
                .feed(direction, excessive.as_bytes())
                .is_err()
        );
    }
}

#[test]
fn chunked_response_trailers_preserve_all_headers_and_leave_pipeline_intact() {
    let mut decoder = HttpDecoder::default();
    let first = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: X-Result\r\n\r\n2\r\nOK\r\n0\r\nX-Result: first\r\nx-result:\tsecond \r\nX-Empty:\r\n";
    assert!(
        decoder
            .feed(Direction::ServerToClient, first)
            .unwrap()
            .is_empty()
    );
    let messages = decoder
        .feed(
            Direction::ServerToClient,
            b"\r\nHTTP/1.1 204 No Content\r\n\r\n",
        )
        .unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].bytes, [first.as_slice(), b"\r\n"].concat());
    assert_eq!(
        messages[0].message.fields["http.body_length"],
        Value::Unsigned(2)
    );
    assert_eq!(
        messages[0].message.fields["http.trailers"],
        Value::List(vec![
            header("X-Result", " first"),
            header("x-result", "\tsecond "),
            header("X-Empty", ""),
        ])
    );
    assert!(!messages[1].message.fields.contains_key("http.trailers"));
}

#[test]
fn chunked_response_trailer_bytes_count_and_syntax_are_bounded() {
    let prefix = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n";
    for (trailers, accepted) in [
        ("X:v\r\n".repeat(128), true),
        ("X:v\r\n".repeat(129), false),
        (format!("X:{}\r\n", "v".repeat(16_384 - 6)), true),
        (format!("X:{}\r\n", "v".repeat(16_385 - 6)), false),
        ("Not a header\r\n".into(), false),
        ("X:bad\u{7f}\r\n".into(), false),
    ] {
        let input = format!("{prefix}{trailers}\r\n");
        let result = HttpDecoder::default().feed(Direction::ServerToClient, input.as_bytes());
        assert_eq!(result.is_ok(), accepted);
    }
    let incomplete = format!("{prefix}X:{}", "v".repeat(16_384));
    assert!(
        HttpDecoder::default()
            .feed(Direction::ServerToClient, incomplete.as_bytes())
            .is_err()
    );
}

#[test]
fn large_feed_still_traps_infinite_wasm_at_the_hard_fuel_cap() {
    let bytes = fixture("(loop br 0) i64.const 0");
    let version = PluginVersion::load(&bytes, Sha256::digest(&bytes).into()).unwrap();
    let mut session = WasmSession::new(version).unwrap();
    let error = session
        .feed(Direction::ClientToServer, &vec![0; 4096])
        .unwrap_err();
    assert!(error.to_string().contains("fuel"));
    assert!(
        session
            .feed(Direction::ClientToServer, b"next")
            .unwrap_err()
            .to_string()
            .contains("disabled")
    );
}
