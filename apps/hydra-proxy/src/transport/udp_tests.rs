use super::*;
use hydra_domain::{
    Action, Comparison, Expression, FieldValue, Mode, Rule, RuleSnapshot, Transport,
};

#[tokio::test]
async fn raw_stream_text_rules_match_udp_datagrams_in_both_directions() {
    let service = Service {
        forwarding: None,
        id: "udp-pattern".into(),
        transport: Transport::Udp,
        listen_address: "127.0.0.1:9000".parse().unwrap(),
        upstream_address: "127.0.0.1:19000".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: false,
        protection: None,
        tls: None,
    };
    let telemetry = Telemetry::new(2);
    telemetry.replace_rules(
        hydra_filter_engine::CompiledSnapshot::compile(
            RuleSnapshot {
                revision: 1,
                rules: vec![Rule {
                    id: "flag-pattern".into(),
                    service_id: service.id.clone(),
                    additional_service_ids: vec![],
                    name: "flag pattern".into(),
                    description: String::new(),
                    enabled: true,
                    mode: Mode::Enforce,
                    priority: 1,
                    expression: Expression::Predicate {
                        field: "stream.text".into(),
                        op: Comparison::Regex,
                        value: FieldValue::String(r"FLAG\{[A-Z0-9]+\}".into()),
                    },
                    action: Action::DropDatagram,
                    backend: "userspace".into(),
                    expires_at_ms: None,
                }],
            },
            std::slice::from_ref(&service),
        )
        .unwrap(),
    );
    let protocols = crate::inspection::protocols::Protocols::open(
        std::env::temp_dir().join(format!("hydra-udp-pattern-{}", uuid::Uuid::new_v4())),
    )
    .await
    .unwrap();
    *telemetry.protocols.write().unwrap() = Some(protocols.clone());
    let client = "127.0.0.1:40000".parse().unwrap();
    let flow = telemetry.open(
        &service,
        client,
        telemetry.permits.clone().acquire_owned().await.unwrap(),
    );
    let context = Context {
        base: RelayBase::new(
            service.clone(),
            client,
            telemetry.clone(),
            Mutex::new(Some(protocols.session(&service.id, "raw").unwrap())).into(),
        ),
        flow,
        last: AtomicU64::new(0),
        start: Instant::now(),
    };

    assert!(context.inspect(Direction::ClientToServer, b"FLAG{TEAM1}"));
    assert!(context.inspect(Direction::ServerToClient, b"FLAG{TEAM2}"));
    assert_eq!(telemetry.udp_dropped.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn byte_limits_work_before_udp_decoding_and_keep_directions_separate() {
    for field in ["body.size", "transfer.bytes", "mixed"] {
        let service = Service {
            forwarding: None,
            id: "udp".into(),
            transport: Transport::Udp,
            listen_address: "127.0.0.1:9000".parse().unwrap(),
            upstream_address: "127.0.0.1:19000".parse().unwrap(),
            protocol_id: "raw".into(),
            enabled: true,
            retain_history: false,
            protection: None,
            tls: None,
        };
        let telemetry = Telemetry::new(1);
        telemetry.replace_rules(
            hydra_filter_engine::CompiledSnapshot::compile(
                RuleSnapshot {
                    revision: 1,
                    rules: vec![Rule {
                        id: "limit".into(),
                        service_id: "udp".into(),
                        additional_service_ids: vec![],
                        name: "byte limit".into(),
                        description: String::new(),
                        enabled: true,
                        mode: Mode::Enforce,
                        priority: 1,
                        expression: if field == "mixed" {
                            Expression::All {
                                all: ["body.size", "packet.size"]
                                    .into_iter()
                                    .map(|field| Expression::Predicate {
                                        field: field.into(),
                                        op: Comparison::Gt,
                                        value: FieldValue::Uint(512),
                                    })
                                    .collect(),
                            }
                        } else {
                            Expression::Predicate {
                                field: field.into(),
                                op: Comparison::Gt,
                                value: FieldValue::Uint(512),
                            }
                        },
                        action: Action::DropDatagram,
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
        let context = Context {
            base: crate::transport::RelayBase::new(
                service,
                client,
                telemetry.clone(),
                Mutex::new(None).into(),
            ),
            flow,
            last: AtomicU64::new(0),
            start: Instant::now(),
        };
        assert!(!context.inspect(Direction::ClientToServer, &[0xff; 512]));
        assert!(!context.inspect(Direction::ServerToClient, &[0xff; 512]));
        if field != "transfer.bytes" {
            assert!(!context.inspect(Direction::ClientToServer, &[0xff; 1]));
            assert!(context.inspect(Direction::ClientToServer, &[0xff; 513]));
        } else {
            assert!(context.inspect(Direction::ClientToServer, &[0xff; 1]));
        }
        assert_eq!(context.telemetry.udp_dropped.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test]
async fn saved_udp_sessions_keep_clients_binary_empty_datagrams_and_idle_closure() {
    use hydra_wire::v1::event::Payload;
    use std::time::Duration;
    let directory =
        std::env::temp_dir().join(format!("hydra-udp-history-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = crate::history::HistoryStore::open(&directory)
        .await
        .unwrap();
    let backend = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listener = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let address = listener.local_addr().unwrap();
    let service = Service {
        forwarding: None,
        id: "udp".into(),
        transport: Transport::Udp,
        listen_address: address,
        upstream_address: backend.local_addr().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: true,
        protection: Some(hydra_domain::ServiceProtection {
            max_active_flows: 4,
            tcp_connections_per_second: 0,
            udp_datagrams_per_second: 100,
            bytes_per_second: 1024,
            idle_timeout_ms: 80,
        }),
        tls: None,
    };
    let telemetry = Telemetry::new(4);
    let mut events = telemetry.events.subscribe();
    *telemetry.history.write().unwrap() = Some(store.clone());
    store.apply_services(std::slice::from_ref(&service));
    let (_sender, receiver) = watch::channel(Some(service));
    let stop = CancellationToken::new();
    let task = tokio::spawn(listen(
        listener,
        receiver,
        Limits {
            udp_idle_ms: 80,
            ..Default::default()
        },
        telemetry.clone(),
        stop.clone(),
    ));
    let echo = tokio::spawn(async move {
        let mut bytes = [0u8; 65536];
        for _ in 0..4 {
            let (n, peer) = backend.recv_from(&mut bytes).await.unwrap();
            backend.send_to(&bytes[..n], peer).await.unwrap();
        }
    });
    let mut clients = Vec::new();
    for marker in [1u8, 2] {
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.connect(address).await.unwrap();
        for payload in [vec![marker, 0, 255], vec![]] {
            client.send(&payload).await.unwrap();
            let mut bytes = [0u8; 8];
            let n = tokio::time::timeout(Duration::from_secs(2), client.recv(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&bytes[..n], payload.as_slice());
        }
        clients.push(client);
    }
    clients[0].send(&[7; 2048]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while telemetry.udp_dropped.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let metrics = telemetry
        .budgets
        .read()
        .unwrap()
        .get("udp")
        .unwrap()
        .metrics();
    assert_eq!(metrics.dropped_datagrams, 1);
    assert_eq!(metrics.rate_limited, 1);
    assert_eq!(metrics.bytes_client_to_server, 6);
    assert_eq!(metrics.bytes_server_to_client, 6);
    echo.await.unwrap();
    let mut opened = Vec::new();
    let mut closed = Vec::new();
    while closed.len() < 2 {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        match event.payload {
            Some(Payload::FlowOpened(_)) => opened.push(event.flow_id),
            Some(Payload::FlowClosed(close)) => {
                assert!(!close.history_truncated);
                closed.push(event.flow_id);
            }
            _ => (),
        }
    }
    assert_eq!(opened.len(), 2);
    assert_eq!(telemetry.active.load(Ordering::Relaxed), 0);
    for (index, flow_id) in opened.iter().enumerate() {
        let saved = store.get(flow_id).await.unwrap().unwrap();
        assert_eq!(saved.datagrams.len(), 4);
        assert_eq!(
            saved
                .datagrams
                .iter()
                .map(|d| d.direction)
                .collect::<Vec<_>>(),
            [1, 2, 1, 2]
        );
        assert_eq!(saved.datagrams[0].data, [index as u8 + 1, 0, 255]);
        assert_eq!(saved.datagrams[1].data, saved.datagrams[0].data);
        assert!(saved.datagrams[2].data.is_empty() && saved.datagrams[3].data.is_empty());
    }
    stop.cancel();
    task.await.unwrap();
    drop(clients);
    drop(telemetry);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}
