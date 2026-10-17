use super::*;
use hydra_wire::v1::event::Payload;

#[tokio::test]
async fn raw_flow_closure_emits_testable_size_metadata_without_payloads() {
    let telemetry = Telemetry::new(1);
    let mut events = telemetry.events.subscribe();
    let service = Service {
        id: "raw".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:9000".parse().unwrap(),
        upstream_address: "127.0.0.1:19000".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: false,
    };
    let flow = telemetry.open(
        &service,
        "127.0.0.1:40000".parse().unwrap(),
        telemetry.permits.clone().acquire_owned().await.unwrap(),
    );
    flow.body_sizes
        .lock()
        .unwrap()
        .feed(Direction::ClientToServer, b"\xff\x00secret", |_| false);
    flow.body_sizes
        .lock()
        .unwrap()
        .feed(Direction::ServerToClient, b"ok", |_| false);
    drop(flow);
    let mut messages = vec![];
    while let Ok(event) = events.try_recv() {
        if let Some(Payload::Message(message)) = event.payload {
            messages.push(message);
        }
    }
    assert_eq!(messages.len(), 2);
    for (message, expected) in messages.iter().zip([8, 2]) {
        assert_eq!(message.message_type, "byte_summary");
        let fields = &message.fields.as_ref().unwrap().fields;
        assert_eq!(
            fields["body.size"].value,
            Some(hydra_wire::v1::field_value::Value::UintValue(expected))
        );
        assert!(!fields.contains_key("payload"));
        assert!(!fields.contains_key("utf8.text"));
    }
}

#[tokio::test]
async fn byte_limit_stops_incomplete_uploads_before_forwarding_the_excess() {
    use hydra_domain::{Comparison, Expression, FieldValue, Mode, Rule, RuleSnapshot};
    use sha2::{Digest, Sha256};
    const LIMIT: usize = 512 * 1024;
    for profile in ["raw", "utf8", "wasm", "http", "chunked"] {
        let protocol = if profile == "chunked" {
            "http"
        } else {
            profile
        };
        let directory =
            std::env::temp_dir().join(format!("hydra-live-limit-{}", uuid::Uuid::new_v4()));
        let protocols = crate::inspection::protocols::Protocols::open(&directory)
            .await
            .unwrap();
        let service = Service {
            id: "upload".into(),
            transport: hydra_domain::Transport::Tcp,
            listen_address: "127.0.0.1:9000".parse().unwrap(),
            upstream_address: "127.0.0.1:19000".parse().unwrap(),
            protocol_id: protocol.into(),
            enabled: true,
            retain_history: false,
        };
        if profile == "wasm" {
            let module =
                wat::parse_str(include_str!("../../../../plugins/fixtures/noop.wat")).unwrap();
            protocols
                .install(
                    "size-test",
                    "v1",
                    &format!("{:x}", Sha256::digest(&module)),
                    module,
                )
                .await
                .unwrap();
            protocols
                .activate(&service.id, "size-test", "v1")
                .await
                .unwrap();
        }
        let telemetry = Telemetry::new(1);
        telemetry.replace_rules(
            hydra_filter_engine::CompiledSnapshot::compile(
                RuleSnapshot {
                    revision: 1,
                    rules: vec![Rule {
                        id: "size-limit".into(),
                        service_id: service.id.clone(),
                        name: "limit".into(),
                        description: String::new(),
                        enabled: true,
                        mode: Mode::Enforce,
                        priority: 10,
                        expression: Expression::Predicate {
                            field: "body.size".into(),
                            op: Comparison::Gt,
                            value: FieldValue::Uint(LIMIT as u64),
                        },
                        action: Action::DropConnection,
                        backend: "userspace".into(),
                        expires_at_ms: None,
                    }],
                },
                std::slice::from_ref(&service),
            )
            .unwrap(),
        );
        let client = "127.0.0.1:40000".parse().unwrap();
        let flow = telemetry.open(
            &service,
            client,
            telemetry.permits.clone().acquire_owned().await.unwrap(),
        );
        flow.body_sizes.lock().unwrap().set_http(protocol == "http");
        let decoder = Arc::new(Mutex::new(Some(
            protocols.session(&service.id, protocol).unwrap(),
        )));
        let context = Context {
            config: &service,
            client,
            telemetry: &telemetry,
            flow: &flow,
            decoder: &decoder,
            idle: Duration::from_secs(5),
        };
        let (mut sender, input) = tokio::io::duplex(4096);
        let (output, mut backend) = tokio::io::duplex(4096);
        let (block, _receiver) = mpsc::channel(1);
        let header = if profile == "chunked" {
            b"POST /upload HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n100000\r\n".as_slice()
        } else if profile == "http" {
            b"POST /upload HTTP/1.1\r\nContent-Length: 1048576\r\n\r\n".as_slice()
        } else {
            b"".as_slice()
        };
        let mut payload = header.to_vec();
        payload.resize(header.len() + LIMIT + 1, 0);
        let relay = client_to_server(input, output, &context, block);
        let producer = async {
            let _ = sender.write_all(&payload).await;
            std::future::pending::<()>().await;
        };
        let drive = async {
            tokio::pin!(producer);
            tokio::select! { result=relay=>result, _=&mut producer=>unreachable!() }
        };
        let receive = async {
            let mut bytes = vec![];
            backend.read_to_end(&mut bytes).await.unwrap();
            bytes
        };
        let (result, forwarded) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(drive, receive)
        })
        .await
        .expect("must block without waiting for request EOF");
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied,
            "{profile}"
        );
        assert!(
            forwarded.len() <= LIMIT + header.len(),
            "excess body bytes forwarded for {profile}"
        );
        assert_eq!(
            forwarded,
            payload[..forwarded.len()],
            "accepted bytes changed for {profile}"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn utf8_tcp_inspection_emits_text_events_in_both_directions() {
    let directory = std::env::temp_dir().join(format!("hydra-utf8-relay-{}", uuid::Uuid::new_v4()));
    let protocols = crate::inspection::protocols::Protocols::open(&directory)
        .await
        .unwrap();
    let service = Service {
        id: "text".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:9000".parse().unwrap(),
        upstream_address: "127.0.0.1:19000".parse().unwrap(),
        protocol_id: "utf8".into(),
        enabled: true,
        retain_history: false,
    };
    let telemetry = Telemetry::new(1);
    let mut events = telemetry.events.subscribe();
    let client = "127.0.0.1:40000".parse().unwrap();
    let flow = telemetry.open(
        &service,
        client,
        telemetry.permits.clone().acquire_owned().await.unwrap(),
    );
    let decoder = Arc::new(Mutex::new(Some(
        protocols.session(&service.id, "utf8").unwrap(),
    )));
    let context = Context {
        config: &service,
        client,
        telemetry: &telemetry,
        flow: &flow,
        decoder: &decoder,
        idle: Duration::from_secs(1),
    };
    assert!(!inspect_raw(
        &context,
        Direction::ClientToServer,
        b"request\n"
    ));
    assert!(!inspect_raw(
        &context,
        Direction::ServerToClient,
        b"reply\n"
    ));
    let mut messages = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let Some(Payload::Message(message)) = event.payload {
            messages.push(message);
        }
    }
    assert_eq!(
        messages.len(),
        2,
        "the actual TCP inspection path must feed the UTF-8 decoder"
    );
    for (message, direction, expected) in
        [(&messages[0], 1, "request\n"), (&messages[1], 2, "reply\n")]
    {
        assert_eq!(message.protocol_id, "utf8");
        assert_eq!(message.direction, direction);
        let field = &message.fields.as_ref().unwrap().fields["utf8.text"];
        assert_eq!(
            field.value,
            Some(hydra_wire::v1::field_value::Value::StringValue(
                expected.into()
            ))
        );
        assert_eq!(
            message
                .fields
                .as_ref()
                .unwrap()
                .fields
                .get("body.size")
                .and_then(|f| f.value.as_ref()),
            Some(&hydra_wire::v1::field_value::Value::UintValue(
                expected.len() as u64
            )),
            "body size must use received bytes regardless of the protocol's text fields",
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
