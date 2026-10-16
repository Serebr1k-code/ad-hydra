use hydra_domain::{Service, Transport};
use hydra_filter_engine::*;
use std::collections::BTreeMap;
fn service() -> Service {
    Service {
        id: "s".into(),
        transport: Transport::Tcp,
        listen_address: "127.0.0.1:1".parse().unwrap(),
        upstream_address: "127.0.0.1:2".parse().unwrap(),
        protocol_id: "raw".into(),
        enabled: true,
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
    let fields = BTreeMap::from([("protocol.x".into(), FieldValue::String("a".repeat(16385)))]);
    assert!(
        compile(vec![r])
            .unwrap()
            .evaluate(&context(&fields))
            .hits
            .is_empty()
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
fn rejects_unknown_http_and_metadata_fields() {
    for field in [
        "http.cookie",
        "network.anything",
        "anything",
        "transport.value",
    ] {
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
    assert!(snapshot.evaluate(&context(&fields)).action.is_none());
    assert!(snapshot.evaluate(&context(&fields)).action.is_none());
    assert_eq!(
        snapshot.evaluate(&context(&fields)).action,
        Some(Action::DropConnection)
    );
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
