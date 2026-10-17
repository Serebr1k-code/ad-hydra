use super::*;
use hydra_wire::v1::event::Payload;

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
    }
    std::fs::remove_dir_all(directory).unwrap();
}
