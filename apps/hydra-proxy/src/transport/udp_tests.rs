use super::*;
use hydra_domain::{Comparison, Expression, FieldValue, Mode, Rule, RuleSnapshot, Transport};

#[tokio::test]
async fn byte_limits_work_before_udp_decoding_and_keep_directions_separate() {
    for field in ["body.size", "transfer.bytes"] {
        let service = Service {
            id: "udp".into(),
            transport: Transport::Udp,
            listen_address: "127.0.0.1:9000".parse().unwrap(),
            upstream_address: "127.0.0.1:19000".parse().unwrap(),
            protocol_id: "raw".into(),
            enabled: true,
            retain_history: false,
        };
        let telemetry = Telemetry::new(1);
        telemetry.replace_rules(
            hydra_filter_engine::CompiledSnapshot::compile(
                RuleSnapshot {
                    revision: 1,
                    rules: vec![Rule {
                        id: "limit".into(),
                        service_id: "udp".into(),
                        name: "byte limit".into(),
                        description: String::new(),
                        enabled: true,
                        mode: Mode::Enforce,
                        priority: 1,
                        expression: Expression::Predicate {
                            field: field.into(),
                            op: Comparison::Gt,
                            value: FieldValue::Uint(512),
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
        if field == "body.size" {
            assert!(!context.inspect(Direction::ClientToServer, &[0xff; 1]));
            assert!(context.inspect(Direction::ClientToServer, &[0xff; 513]));
        } else {
            assert!(context.inspect(Direction::ClientToServer, &[0xff; 1]));
        }
        assert_eq!(context.telemetry.udp_dropped.load(Ordering::Relaxed), 1);
    }
}
