use hydra_domain::{Service, Transport};
use hydra_filter_engine::*;
use std::collections::BTreeMap;
fn service() -> Service {
    Service {
        forwarding: None,
        id: "s".into(),
        transport: Transport::Tcp,
        listen_address: "127.0.0.1:1".parse().unwrap(),
        upstream_address: "127.0.0.1:2".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
        retain_history: true,
        protection: None,
        tls: None,
    }
}
fn predicate(op: Comparison, expected: FieldValue) -> Expression {
    Expression::Predicate {
        field: "protocol.x".into(),
        op,
        value: expected,
    }
}
fn rule(id: &str, action: Action) -> Rule {
    Rule {
        id: id.into(),
        service_id: "s".into(),
        additional_service_ids: vec![],
        name: id.into(),
        description: String::new(),
        enabled: true,
        mode: Mode::Enforce,
        priority: 1,
        expression: predicate(Comparison::Eq, FieldValue::String("yes".into())),
        action,
        backend: "userspace".into(),
        expires_at_ms: None,
    }
}
fn context(fields: &BTreeMap<String, FieldValue>) -> EvaluationContext<'_> {
    EvaluationContext {
        service_id: "s",
        transport: Transport::Tcp,
        fields,
        now_ms: 100,
        safe_http: false,
    }
}
fn compile(rules: Vec<Rule>) -> Result<CompiledSnapshot, CompileError> {
    CompiledSnapshot::compile(RuleSnapshot { revision: 1, rules }, &[service()])
}
#[test]
fn one_rule_applies_to_each_selected_service_and_validates_every_transport() {
    let mut udp = service();
    udp.id = "udp".into();
    udp.transport = Transport::Udp;
    let services = [service(), udp];
    let mut shared = rule("shared", Action::Log);
    shared.additional_service_ids.push("udp".into());
    let compile_shared = |rule| {
        CompiledSnapshot::compile(
            RuleSnapshot {
                revision: 1,
                rules: vec![rule],
            },
            &services,
        )
    };
    let compiled = compile_shared(shared.clone()).unwrap();
    let fields = BTreeMap::from([("protocol.x".into(), FieldValue::String("yes".into()))]);
    for service in &services {
        let result = compiled.evaluate(&EvaluationContext {
            service_id: &service.id,
            transport: service.transport,
            ..context(&fields)
        });
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].rule_id, "shared");
    }
    assert!(
        compiled
            .evaluate(&EvaluationContext {
                service_id: "other",
                ..context(&fields)
            })
            .hits
            .is_empty()
    );
    let mut wrong_transport = shared.clone();
    wrong_transport.action = Action::DropConnection;
    assert!(compile_shared(wrong_transport).is_err());
    shared.additional_service_ids.push("s".into());
    assert!(compile_shared(shared).is_err());
}
#[test]
fn missing_field_stays_unknown_under_not() {
    let mut r = rule("a", Action::DropConnection);
    r.expression = Expression::Not {
        not: Box::new(r.expression),
    };
    assert!(
        compile(vec![r])
            .unwrap()
            .evaluate(&context(&BTreeMap::new()))
            .hits
            .is_empty()
    );
}
#[test]
fn priority_tie_shadow_ttl_and_terminal() {
    let mut a = rule("a", Action::DropConnection);
    a.mode = Mode::Shadow;
    let b = rule("b", Action::Allow);
    let mut c = rule("c", Action::DropConnection);
    c.expires_at_ms = Some(100);
    let fields = BTreeMap::from([("protocol.x".into(), FieldValue::String("yes".into()))]);
    let result = compile(vec![c, b, a]).unwrap().evaluate(&context(&fields));
    assert_eq!(
        result
            .hits
            .iter()
            .map(|h| h.rule_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(result.action, Some(Action::Allow));
}
#[test]
fn validates_transport_and_limits() {
    assert!(compile(vec![rule("a", Action::DropDatagram)]).is_err());
    assert!(compile(vec![rule("a", Action::Block { http_status: 403 })]).is_err());
    let mut r = rule("a", Action::Allow);
    for _ in 0..16 {
        r.expression = Expression::Not {
            not: Box::new(r.expression),
        };
    }
    assert!(compile(vec![r]).is_err());
    let mut r = rule("a", Action::Allow);
    r.expression = predicate(Comparison::Regex, FieldValue::String("x".repeat(1025)));
    assert!(compile(vec![r]).is_err());
}
#[test]
fn regex_input_is_not_truncated_into_match() {
    let mut r = rule("a", Action::Allow);
    r.expression = predicate(Comparison::Regex, FieldValue::String("^a".into()));
    let fields = BTreeMap::from([(
        "protocol.x".into(),
        FieldValue::String("a".repeat(MAX_EVALUATED_FIELD_BYTES + 1)),
    )]);
    assert!(
        compile(vec![r])
            .unwrap()
            .evaluate(&context(&fields))
            .hits
            .is_empty()
    );
}

#[test]
fn http_body_and_packet_size_fields_are_available_to_visual_rules() {
    let mut body = rule("body", Action::Alert);
    body.expression = Expression::Not {
        not: Box::new(Expression::Predicate {
            field: "http.body_text".into(),
            op: Comparison::Regex,
            value: FieldValue::String("^Z(?:I[^U]|[^I]).*$".into()),
        }),
    };
    let mut size = rule("size", Action::Log);
    size.expression = Expression::Predicate {
        field: "packet.size".into(),
        op: Comparison::Gt,
        value: FieldValue::Uint(512_000),
    };
    let fields = BTreeMap::from([
        (
            "http.body_text".into(),
            FieldValue::String("ordinary body".into()),
        ),
        ("packet.size".into(), FieldValue::Uint(600_000)),
    ]);
    let result = compile(vec![body, size])
        .unwrap()
        .evaluate(&context(&fields));
    assert_eq!(result.hits.len(), 2);
}

#[test]
fn streaming_size_evaluation_does_not_run_unrelated_rules_and_respects_allow_priority() {
    let mut limit = rule("limit", Action::DropConnection);
    limit.expression = Expression::Predicate {
        field: "body.size".into(),
        op: Comparison::Gt,
        value: FieldValue::Uint(512),
    };
    let normal = rule("unrelated", Action::Alert);
    let fields = BTreeMap::from([
        ("body.size".into(), FieldValue::Uint(513)),
        ("protocol.x".into(), FieldValue::String("yes".into())),
    ]);
    let compiled = compile(vec![normal, limit.clone()]).unwrap();
    let result = compiled.evaluate_stream_sizes(&context(&fields));
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.action, Some(Action::DropConnection));
    let mut allow = rule("allow", Action::Allow);
    allow.priority = 20;
    assert_eq!(
        compile(vec![allow, limit])
            .unwrap()
            .evaluate_stream_sizes(&context(&fields))
            .action,
        Some(Action::Allow)
    );
}
#[test]
fn typed_comparisons_and_membership() {
    for (op, expected) in [
        (Comparison::Gt, FieldValue::Int(3)),
        (Comparison::In, FieldValue::List(vec![FieldValue::Int(4)])),
        (
            Comparison::NotIn,
            FieldValue::List(vec![FieldValue::Int(5)]),
        ),
    ] {
        let mut r = rule("a", Action::Allow);
        r.expression = predicate(op, expected);
        let fields = BTreeMap::from([("protocol.x".into(), FieldValue::Int(4))]);
        assert_eq!(
            compile(vec![r]).unwrap().evaluate(&context(&fields)).action,
            Some(Action::Allow)
        );
    }
}
#[test]
fn operators_and_nested_fields() {
    let fields = BTreeMap::from([(
        "http".into(),
        FieldValue::Object(BTreeMap::from([(
            "path".into(),
            FieldValue::String("/admin/test".into()),
        )])),
    )]);
    for (op, value) in [
        (Comparison::Eq, "/admin/test"),
        (Comparison::Ne, "/other"),
        (Comparison::Contains, "admin"),
        (Comparison::Prefix, "/admin"),
        (Comparison::Regex, "^/admin/"),
    ] {
        let mut r = rule("a", Action::Log);
        r.expression = Expression::Predicate {
            field: "http.path".into(),
            op,
            value: FieldValue::String(value.into()),
        };
        assert_eq!(
            compile(vec![r])
                .unwrap()
                .evaluate(&context(&fields))
                .hits
                .len(),
            1
        );
    }
}
#[test]
fn disabled_rules_and_missing_negative_comparisons_do_not_match() {
    for op in [Comparison::Ne, Comparison::NotIn] {
        let mut r = rule("a", Action::Allow);
        r.expression = predicate(
            op,
            if op == Comparison::NotIn {
                FieldValue::List(vec![FieldValue::String("protocol.x".into())])
            } else {
                FieldValue::String("protocol.x".into())
            },
        );
        assert!(
            compile(vec![r])
                .unwrap()
                .evaluate(&context(&BTreeMap::new()))
                .hits
                .is_empty()
        );
    }
    let mut r = rule("a", Action::Allow);
    r.enabled = false;
    let fields = BTreeMap::from([("protocol.x".into(), FieldValue::String("yes".into()))]);
    assert!(
        compile(vec![r])
            .unwrap()
            .evaluate(&context(&fields))
            .hits
            .is_empty()
    );
}
#[test]
fn rule_node_limits_and_bad_regex_reject_snapshot() {
    assert!(
        compile(
            (0..257)
                .map(|i| rule(&i.to_string(), Action::Allow))
                .collect()
        )
        .is_err()
    );
    let mut r = rule("a", Action::Allow);
    r.expression = Expression::All {
        all: vec![r.expression; 256],
    };
    assert!(compile(vec![r]).is_err());
    let mut r = rule("a", Action::Allow);
    r.expression = predicate(Comparison::Regex, FieldValue::String("[".into()));
    assert!(compile(vec![r]).is_err());
}
#[test]
fn http_block_only_when_caller_guarantees_buffered_http() {
    let mut service = service();
    service.protocol_id = "http".into();
    let snapshot = CompiledSnapshot::compile(
        RuleSnapshot {
            revision: 1,
            rules: vec![rule("a", Action::Block { http_status: 403 })],
        },
        &[service],
    )
    .unwrap();
    let fields = BTreeMap::from([("protocol.x".into(), FieldValue::String("yes".into()))]);
    assert!(snapshot.evaluate(&context(&fields)).hits.is_empty());
    let mut c = context(&fields);
    c.safe_http = true;
    assert_eq!(
        snapshot.evaluate(&c).action,
        Some(Action::Block { http_status: 403 })
    );
}
#[test]
fn missing_any_and_all_follow_three_valued_logic() {
    let yes = predicate(Comparison::Eq, FieldValue::String("yes".into()));
    let missing = Expression::Predicate {
        field: "protocol.missing".into(),
        op: Comparison::Eq,
        value: FieldValue::Bool(true),
    };
    let fields = BTreeMap::from([("protocol.x".into(), FieldValue::String("yes".into()))]);
    let mut r = rule("a", Action::Allow);
    r.expression = Expression::Any {
        any: vec![missing.clone(), yes.clone()],
    };
    assert_eq!(
        compile(vec![r.clone()])
            .unwrap()
            .evaluate(&context(&fields))
            .hits
            .len(),
        1
    );
    r.expression = Expression::Not {
        not: Box::new(Expression::All {
            all: vec![missing, yes],
        }),
    };
    assert!(
        compile(vec![r])
            .unwrap()
            .evaluate(&context(&fields))
            .hits
            .is_empty()
    );
}

#[test]
fn decoder_fields_compile_and_match_only_when_present() {
    for field in [
        "utf8.text",
        "http.status_code",
        "crypter.username",
        "grpc.method",
        "payload",
        "custom_field",
    ] {
        let mut r = rule("a", Action::DropConnection);
        r.expression = Expression::Predicate {
            field: field.into(),
            op: Comparison::Eq,
            value: FieldValue::String("needle".into()),
        };
        let snapshot = compile(vec![r]).unwrap();
        let fields = BTreeMap::from([(field.into(), FieldValue::String("needle".into()))]);
        assert_eq!(
            snapshot.evaluate(&context(&fields)).hits.len(),
            1,
            "{field}"
        );
        assert!(
            snapshot
                .evaluate(&context(&BTreeMap::new()))
                .hits
                .is_empty(),
            "missing {field}"
        );
    }
}

#[test]
fn malformed_field_paths_are_rejected() {
    for field in ["", ".text", "utf8.", "custom..text", &"x".repeat(257)] {
        let mut r = rule("a", Action::Allow);
        r.expression = Expression::Predicate {
            field: field.into(),
            op: Comparison::Eq,
            value: FieldValue::Bool(true),
        };
        assert!(compile(vec![r]).is_err());
    }
}

#[test]
fn aggregate_snapshot_budget_is_checked_before_regex_compilation() {
    let rules = (0..17)
        .map(|i| {
            let mut r = rule(&i.to_string(), Action::Log);
            // Invalid first regex proves budget checking precedes any compilation.
            r.expression = predicate(Comparison::Regex, FieldValue::String("[".into()));
            r
        })
        .collect();
    assert_eq!(
        compile(rules).unwrap_err().0,
        "snapshot regex count exceeds 16"
    );
}
#[test]
fn aggregate_snapshot_nodes_and_expected_bytes_are_bounded() {
    let mut rules = Vec::new();
    for i in 0..3 {
        let mut r = rule(&i.to_string(), Action::Log);
        r.expression = Expression::All {
            all: vec![r.expression; 200],
        };
        rules.push(r);
    }
    assert_eq!(
        compile(rules).unwrap_err().0,
        "snapshot node count exceeds 512"
    );
    let rules = (0..18)
        .map(|i| {
            let mut r = rule(&i.to_string(), Action::Log);
            r.expression = predicate(Comparison::Eq, FieldValue::String("x".repeat(16000)));
            r
        })
        .collect();
    assert_eq!(
        compile(rules).unwrap_err().0,
        "snapshot expected value budget exceeds 256 KiB"
    );
}

#[test]
fn bounded_rate_windows_separate_keys_and_missing_fields() {
    let action = Action::RateLimit {
        key: "network.src_ip".into(),
        requests: 2,
        window_ms: 1000,
        on_exceed: Box::new(Action::DropConnection),
    };
    let snapshot = compile(vec![rule("rate", action)]).unwrap();
    let mut fields = BTreeMap::from([
        ("protocol.x".into(), FieldValue::String("yes".into())),
        (
            "network.src_ip".into(),
            FieldValue::String("10.0.0.1".into()),
        ),
    ]);
    let first = snapshot.evaluate(&context(&fields));
    assert!(first.action.is_none());
    assert!(!first.hits[0].blocked);
    assert!(snapshot.evaluate(&context(&fields)).action.is_none());
    let exceeded = snapshot.evaluate(&context(&fields));
    assert_eq!(exceeded.action, Some(Action::DropConnection));
    assert!(exceeded.hits[0].blocked);
    fields.insert(
        "network.src_ip".into(),
        FieldValue::String("10.0.0.2".into()),
    );
    assert!(snapshot.evaluate(&context(&fields)).action.is_none());
    fields.remove("network.src_ip");
    assert!(snapshot.evaluate(&context(&fields)).hits.is_empty());
    fields.insert(
        "network.src_ip".into(),
        FieldValue::String("10.0.0.1".into()),
    );
    let mut later = context(&fields);
    later.now_ms = 1100;
    assert!(snapshot.evaluate(&later).action.is_none());
}

#[test]
fn condition_direction_scope_stays_nonmatching_on_other_side_even_under_negation() {
    let scoped:Expression=serde_json::from_value(serde_json::json!({"direction":"client_to_server","scoped":{"field":"protocol.x","op":"eq","value":{"type":"string","value":"yes"}}})).unwrap();
    let mut r = rule("scoped", Action::DropConnection);
    r.expression = Expression::Not {
        not: Box::new(scoped),
    };
    let compiled = compile(vec![r]).unwrap();
    for side in [None, Some("server_to_client")] {
        let mut fields = BTreeMap::from([("protocol.x".into(), FieldValue::String("no".into()))]);
        if let Some(side) = side {
            fields.insert("direction".into(), FieldValue::String(side.into()));
        }
        assert!(compiled.evaluate(&context(&fields)).hits.is_empty());
    }
    let fields = BTreeMap::from([
        (
            "direction".into(),
            FieldValue::String("client_to_server".into()),
        ),
        ("protocol.x".into(), FieldValue::String("no".into())),
    ]);
    assert_eq!(compiled.evaluate(&context(&fields)).hits.len(), 1);
}
#[test]
fn condition_direction_scopes_support_mixed_or_and_do_not_correlate_opposite_sides() {
    let make = |side: &str, value: &str| {
        serde_json::from_value::<Expression>(serde_json::json!({"direction":side,"scoped":{"field":"protocol.x","op":"eq","value":{"type":"string","value":value}}})).unwrap()
    };
    for (all, expected) in [(false, 1), (true, 0)] {
        let mut r = rule("scoped", Action::DropConnection);
        let children = vec![
            make("client_to_server", "request"),
            make("server_to_client", "reply"),
        ];
        r.expression = if all {
            Expression::All { all: children }
        } else {
            Expression::Any { any: children }
        };
        let compiled = compile(vec![r]).unwrap();
        for (side, value) in [
            ("client_to_server", "request"),
            ("server_to_client", "reply"),
        ] {
            let fields = BTreeMap::from([
                ("direction".into(), FieldValue::String(side.into())),
                ("protocol.x".into(), FieldValue::String(value.into())),
            ]);
            assert_eq!(compiled.evaluate(&context(&fields)).hits.len(), expected);
        }
    }
}

/// A 16 KiB read plus the raw overlap tail exceeds the rule-input cap. Live rules
/// must still match on the window instead of silently becoming Unknown.
#[test]
fn live_rules_match_fields_larger_than_the_rule_input_cap() {
    let mut r = rule("large", Action::DropConnection);
    r.expression = predicate(Comparison::Contains, FieldValue::Bytes(vec![0xde, 0xad]));
    let compiled = compile(vec![r]).unwrap();
    // 17_407 is the largest TCP raw window (16 KiB read plus the 1023-byte tail).
    for size in [15_000usize, 17_407, 65_559] {
        let mut bytes = vec![b'a'; size];
        bytes[0] = 0xde;
        bytes[1] = 0xad;
        let fields = BTreeMap::from([("protocol.x".into(), FieldValue::Bytes(bytes))]);
        assert_eq!(
            compiled.evaluate(&context(&fields)).hits.len(),
            1,
            "size {size}"
        );
    }
    let mut text = rule("large-text", Action::DropConnection);
    text.expression = predicate(Comparison::Contains, FieldValue::String("cmd=".into()));
    let compiled = compile(vec![text]).unwrap();
    let fields = BTreeMap::from([(
        "protocol.x".into(),
        FieldValue::String(format!("cmd={}", "a".repeat(64 * 1024))),
    )]);
    assert_eq!(compiled.evaluate(&context(&fields)).hits.len(), 1);
}

#[test]
fn stream_text_pattern_blocks_http_profile_payloads() {
    let mut pattern = rule("pattern", Action::DropConnection);
    pattern.name = "traversal".into();
    pattern.expression = Expression::Predicate {
        field: "stream.text".into(),
        op: Comparison::Regex,
        value: FieldValue::String(r"\.\./".into()),
    };
    let compiled = compile(vec![pattern]).unwrap();
    let hit = BTreeMap::from([(
        "stream.text".to_string(),
        FieldValue::String("GET /static/../../etc/passwd HTTP/1.1".into()),
    )]);
    assert_eq!(
        compiled.evaluate(&context(&hit)).action,
        Some(Action::DropConnection)
    );
    let clean = BTreeMap::from([(
        "stream.text".to_string(),
        FieldValue::String("GET /static/app.js HTTP/1.1".into()),
    )]);
    assert_eq!(compiled.evaluate(&context(&clean)).action, None);
    let absent = BTreeMap::new();
    assert_eq!(compiled.evaluate(&context(&absent)).action, None);
}

#[test]
fn stream_text_pattern_respects_case_and_direction_scoping() {
    let mut pattern = rule("case", Action::DropConnection);
    pattern.expression = Expression::Scoped {
        direction: hydra_domain::RuleDirection::ServerToClient,
        scoped: Box::new(Expression::Predicate {
            field: "stream.text".into(),
            op: Comparison::Regex,
            value: FieldValue::String("X-Debug".into()),
        }),
    };
    let compiled = compile(vec![pattern]).unwrap();
    let upper = BTreeMap::from([
        (
            "stream.text".to_string(),
            FieldValue::String("X-Debug: 1".into()),
        ),
        (
            "direction".to_string(),
            FieldValue::String("server_to_client".into()),
        ),
    ]);
    assert_eq!(
        compiled.evaluate(&context(&upper)).action,
        Some(Action::DropConnection)
    );
    let lower = BTreeMap::from([
        (
            "stream.text".to_string(),
            FieldValue::String("x-debug: 1".into()),
        ),
        (
            "direction".to_string(),
            FieldValue::String("server_to_client".into()),
        ),
    ]);
    assert_eq!(compiled.evaluate(&context(&lower)).action, None);
}
