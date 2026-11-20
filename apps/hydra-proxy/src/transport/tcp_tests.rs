use super::*;
use hydra_wire::v1::event::Payload;
use tokio::net::TcpStream;

#[tokio::test]
async fn raw_flow_closure_emits_testable_size_metadata_without_payloads() {
    let telemetry = Telemetry::new(1);
    let mut events = telemetry.events.subscribe();
    let service = Service {
        forwarding: None,
        id: "raw".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:9000".parse().unwrap(),
        upstream_address: "127.0.0.1:19000".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: false,
        protection: None,
        tls: None,
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
async fn raw_byte_filter_matches_across_reads_without_emitting_payload_telemetry() {
    use hydra_domain::{Action, Comparison, Expression, FieldValue, Mode, Rule, RuleSnapshot};

    let telemetry = Telemetry::new(1);
    let mut events = telemetry.events.subscribe();
    let service = Service {
        forwarding: None,
        id: "raw-match".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:9001".parse().unwrap(),
        upstream_address: "127.0.0.1:19001".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: false,
        protection: None,
        tls: None,
    };
    let protocols = crate::inspection::protocols::Protocols::open(
        std::env::temp_dir().join(format!("hydra-raw-filter-{}", uuid::Uuid::new_v4())),
    )
    .await
    .unwrap();
    *telemetry.protocols.write().unwrap() = Some(protocols.clone());
    telemetry.replace_rules(
        hydra_filter_engine::CompiledSnapshot::compile(
            RuleSnapshot {
                revision: 1,
                rules: vec![Rule {
                    id: "raw-rule".into(),
                    service_id: service.id.clone(),
                    additional_service_ids: vec![],
                    name: "raw bytes".into(),
                    description: String::new(),
                    enabled: true,
                    mode: Mode::Enforce,
                    priority: 1,
                    expression: Expression::Predicate {
                        field: "raw.bytes".into(),
                        op: Comparison::Contains,
                        value: FieldValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
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
    let client = "127.0.0.1:40001".parse().unwrap();
    let flow = telemetry.open(
        &service,
        client,
        telemetry.permits.clone().acquire_owned().await.unwrap(),
    );
    let decoder = Arc::new(Mutex::new(Some(
        protocols.session(&service.id, "raw").unwrap(),
    )));
    let context = Context {
        base: crate::transport::RelayBase::new(service.clone(), client, telemetry.clone(), decoder),
        flow: &flow,
        idle: Duration::from_secs(1),
        last_activity: Mutex::new(tokio::time::Instant::now()),
    };
    assert!(!inspect_raw(
        &context,
        Direction::ClientToServer,
        b"prefix\xde\xad"
    ));
    assert!(inspect_raw(
        &context,
        Direction::ClientToServer,
        b"\xbe\xefsuffix"
    ));
    while let Ok(event) = events.try_recv() {
        if let Some(Payload::Message(message)) = event.payload {
            let fields = &message.fields.unwrap().fields;
            assert!(!fields.contains_key("raw.bytes"));
        }
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
            forwarding: None,
            id: "upload".into(),
            transport: hydra_domain::Transport::Tcp,
            listen_address: "127.0.0.1:9000".parse().unwrap(),
            upstream_address: "127.0.0.1:19000".parse().unwrap(),
            protocol_id: protocol.into(),
            enabled: true,
            retain_history: false,
            protection: None,
            tls: None,
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
                        additional_service_ids: vec![],
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
            base: crate::transport::RelayBase::new(
                service.clone(),
                client,
                telemetry.clone(),
                decoder.clone(),
            ),
            flow: &flow,
            idle: Duration::from_secs(5),
            last_activity: Mutex::new(tokio::time::Instant::now()),
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
        forwarding: None,
        id: "text".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:9000".parse().unwrap(),
        upstream_address: "127.0.0.1:19000".parse().unwrap(),
        protocol_id: "utf8".into(),
        enabled: true,
        retain_history: false,
        protection: None,
        tls: None,
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
        base: crate::transport::RelayBase::new(
            service.clone(),
            client,
            telemetry.clone(),
            decoder.clone(),
        ),
        flow: &flow,
        idle: Duration::from_secs(1),
        last_activity: Mutex::new(tokio::time::Instant::now()),
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

#[tokio::test]
async fn service_caps_isolate_same_ip_clients_and_global_cap_counts_rejections() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = backend.local_addr().unwrap();
    let echo = tokio::spawn(async move {
        loop {
            let (mut socket, _) = backend.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    if socket.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let telemetry = Telemetry::new(2);
    let stop = CancellationToken::new();
    let tracker = TaskTracker::new();
    let mut tasks = Vec::new();
    let mut configs = Vec::new();
    let mut senders = Vec::new();
    for id in ["a", "b"] {
        let listener = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
        let service = Service {
            forwarding: None,
            id: id.into(),
            transport: hydra_domain::Transport::Tcp,
            listen_address: listener.local_addr().unwrap(),
            upstream_address: upstream,
            protocol_id: "raw".into(),
            enabled: true,
            retain_history: false,
            protection: Some(hydra_domain::ServiceProtection {
                max_active_flows: 1,
                tcp_connections_per_second: 0,
                udp_datagrams_per_second: 0,
                bytes_per_second: 1024,
                idle_timeout_ms: 5000,
            }),
            tls: None,
        };
        let (sender, receiver) = watch::channel(Some(service.clone()));
        senders.push(sender);
        tasks.push(tokio::spawn(listen(
            listener,
            receiver,
            Limits::default(),
            telemetry.clone(),
            stop.clone(),
            tracker.clone(),
        )));
        configs.push(service);
    }
    async fn binary_roundtrip(address: std::net::SocketAddr) -> TcpStream {
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(&[0, 255, 128, 42]).await.unwrap();
        let mut data = [0; 4];
        tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut data))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(data, [0, 255, 128, 42]);
        client
    }
    async fn rejected(address: std::net::SocketAddr) {
        let mut client = TcpStream::connect(address).await.unwrap();
        let mut byte = [0];
        let outcome = tokio::time::timeout(Duration::from_secs(2), client.read(&mut byte))
            .await
            .unwrap();
        assert!(matches!(outcome, Ok(0) | Err(_)));
    }
    let mut a = binary_roundtrip(configs[0].listen_address).await;
    rejected(configs[0].listen_address).await; // service cap with global slot still free
    let b = binary_roundtrip(configs[1].listen_address).await;
    rejected(configs[1].listen_address).await; // shared global cap
    assert_eq!(telemetry.rejected.load(Ordering::Relaxed), 2);
    assert_eq!(telemetry.budget(&configs[0]).metrics().active_flows, 1);
    assert_eq!(telemetry.budget(&configs[1]).metrics().accepted_flows, 1);
    // An oversized read closes the whole connection before forwarding its contents.
    a.write_all(&[7; 2048]).await.unwrap();
    let mut byte = [0];
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), a.read(&mut byte))
            .await
            .unwrap(),
        Ok(0) | Err(_)
    ));
    assert_eq!(
        telemetry
            .budget(&configs[0])
            .metrics()
            .bytes_client_to_server,
        4
    );
    assert_eq!(telemetry.budget(&configs[0]).metrics().rate_limited, 1);
    drop((a, b, senders));
    stop.cancel();
    for task in tasks {
        task.await.unwrap();
    }
    tracker.close();
    tracker.wait().await;
    echo.abort();
}

#[tokio::test]
async fn one_way_upload_keeps_connection_alive_and_true_idle_releases_slot() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
    let address = listener.local_addr().unwrap();
    let config = Service {
        forwarding: None,
        id: "one-way".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: address,
        upstream_address: backend.local_addr().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: false,
        protection: Some(hydra_domain::ServiceProtection {
            max_active_flows: 1,
            tcp_connections_per_second: 0,
            udp_datagrams_per_second: 0,
            bytes_per_second: 0,
            idle_timeout_ms: 400,
        }),
        tls: None,
    };
    let telemetry = Telemetry::new(1);
    let (received_tx, mut received_rx) = mpsc::channel(16);
    let backend_task = tokio::spawn(async move {
        // The backend deliberately never sends a response.
        let (mut socket, _) = backend.accept().await.unwrap();
        let mut byte = [0];
        while socket.read_exact(&mut byte).await.is_ok() {
            received_tx.send(byte[0]).await.unwrap();
        }
    });
    let (_sender, receiver) = watch::channel(Some(config.clone()));
    let stop = CancellationToken::new();
    let tracker = TaskTracker::new();
    let relay = tokio::spawn(listen(
        listener,
        receiver,
        Limits::default(),
        telemetry.clone(),
        stop.clone(),
        tracker.clone(),
    ));
    let mut client = TcpStream::connect(address).await.unwrap();
    for value in 0..12u8 {
        client.write_all(&[value]).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), received_rx.recv())
                .await
                .unwrap(),
            Some(value)
        );
        assert_eq!(telemetry.budget(&config).metrics().active_flows, 1);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // More than one idle interval elapsed with only client -> upstream activity.
    assert_eq!(
        telemetry.budget(&config).metrics().bytes_client_to_server,
        12
    );
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), client.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while telemetry.budget(&config).metrics().active_flows != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(telemetry.permits.available_permits(), 1);
    stop.cancel();
    relay.await.unwrap();
    tracker.close();
    tracker.wait().await;
    backend_task.await.unwrap();
}

/// A decoder (including a hot-loaded WASM plugin) must not be able to rewrite the
/// host's own view of the connection: rules keyed on `network.*` stay trustworthy.
#[tokio::test]
async fn decoder_fields_cannot_shadow_host_network_metadata() {
    use hydra_protocol_sdk::{Direction, ParsedMessage, Value};
    use std::collections::BTreeMap;

    let telemetry = Telemetry::new(1);
    let service = Service {
        forwarding: None,
        id: "shadow".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:9010".parse().unwrap(),
        upstream_address: "127.0.0.1:19010".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: false,
        protection: None,
        tls: None,
    };
    let client = "203.0.113.7:40010".parse().unwrap();
    let flow = telemetry.open(
        &service,
        client,
        telemetry.permits.clone().acquire_owned().await.unwrap(),
    );
    let decoder = Arc::new(Mutex::new(None));
    let context = Context {
        base: crate::transport::RelayBase::new(service.clone(), client, telemetry.clone(), decoder),
        flow: &flow,
        idle: Duration::from_secs(1),
        last_activity: Mutex::new(tokio::time::Instant::now()),
    };
    let message = ParsedMessage {
        protocol: "wasm".into(),
        message_type: "spoof".into(),
        direction: Direction::ClientToServer,
        fields: BTreeMap::from([
            ("network.src_ip".into(), Value::String("10.0.0.1".into())),
            ("network.dst_port".into(), Value::Unsigned(1)),
            ("packet.size".into(), Value::Unsigned(1)),
        ]),
    };
    let fields = merged_fields(&context, "client_to_server", &message, 99, b"GET /x HTTP/1.1");
    assert_eq!(
        fields["network.src_ip"],
        hydra_domain::FieldValue::String("203.0.113.7".into())
    );
    assert_eq!(
        fields["network.dst_port"],
        hydra_domain::FieldValue::Uint(19010)
    );
    assert_eq!(fields["packet.size"], hydra_domain::FieldValue::Uint(99));
    assert_eq!(
        fields["stream.text"],
        hydra_domain::FieldValue::String("GET /x HTTP/1.1".into())
    );
}

#[tokio::test]
async fn decoder_cannot_shadow_stream_text() {
    use hydra_protocol_sdk::{Direction, ParsedMessage, Value};
    use std::collections::BTreeMap;

    let telemetry = Telemetry::new(1);
    let service = Service {
        forwarding: None,
        id: "stream".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:9011".parse().unwrap(),
        upstream_address: "127.0.0.1:19011".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: false,
        protection: None,
        tls: None,
    };
    let client = "203.0.113.8:40011".parse().unwrap();
    let flow = telemetry.open(
        &service,
        client,
        telemetry.permits.clone().acquire_owned().await.unwrap(),
    );
    let decoder = Arc::new(Mutex::new(None));
    let context = Context {
        base: crate::transport::RelayBase::new(service.clone(), client, telemetry.clone(), decoder),
        flow: &flow,
        idle: Duration::from_secs(1),
        last_activity: Mutex::new(tokio::time::Instant::now()),
    };
    let message = ParsedMessage {
        protocol: "wasm".into(),
        message_type: "spoof".into(),
        direction: Direction::ClientToServer,
        fields: BTreeMap::from([("stream.text".into(), Value::String("clean".into()))]),
    };
    let fields = merged_fields(&context, "client_to_server", &message, 8, b"../etc/passwd");
    assert_eq!(
        fields["stream.text"],
        hydra_domain::FieldValue::String("../etc/passwd".into())
    );
}

#[tokio::test]
async fn stream_text_survives_invalid_utf8() {
    use hydra_protocol_sdk::{Direction, ParsedMessage};
    use std::collections::BTreeMap;

    let telemetry = Telemetry::new(1);
    let service = Service {
        forwarding: None,
        id: "bytes".into(),
        transport: hydra_domain::Transport::Tcp,
        listen_address: "127.0.0.1:9012".parse().unwrap(),
        upstream_address: "127.0.0.1:19012".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: false,
        protection: None,
        tls: None,
    };
    let client = "203.0.113.9:40012".parse().unwrap();
    let flow = telemetry.open(
        &service,
        client,
        telemetry.permits.clone().acquire_owned().await.unwrap(),
    );
    let decoder = Arc::new(Mutex::new(None));
    let context = Context {
        base: crate::transport::RelayBase::new(service.clone(), client, telemetry.clone(), decoder),
        flow: &flow,
        idle: Duration::from_secs(1),
        last_activity: Mutex::new(tokio::time::Instant::now()),
    };
    let message = ParsedMessage {
        protocol: "raw".into(),
        message_type: "payload".into(),
        direction: Direction::ClientToServer,
        fields: BTreeMap::new(),
    };
    let fields = merged_fields(&context, "client_to_server", &message, 5, b"\xff\xfe/../");
    match &fields["stream.text"] {
        hydra_domain::FieldValue::String(text) => {
            assert!(text.contains("/../"), "{text}");
            assert!(text.contains('\u{fffd}'), "{text}");
        }
        other => panic!("unexpected {other:?}"),
    }
}
