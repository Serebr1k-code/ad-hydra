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
    assert!(
        SessionDecoder::Raw
            .feed(Direction::ClientToServer, b"opaque\0")
            .unwrap()
            .is_empty()
    );
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
