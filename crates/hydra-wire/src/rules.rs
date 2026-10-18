use crate::v1 as p;
use hydra_domain::{Action, Comparison, Expression, FieldValue, Mode, Rule, RuleSnapshot};
use std::collections::BTreeMap;
impl From<&RuleSnapshot> for p::ApplyRuleSnapshotRequest {
    fn from(s: &RuleSnapshot) -> Self {
        Self {
            revision: s.revision,
            rules: s.rules.iter().map(rule_to_wire).collect(),
            ..Default::default()
        }
    }
}
pub fn action_code(action: &Action) -> i32 {
    match action {
        Action::Allow => 1,
        Action::Alert => 2,
        Action::Block { .. } => 3,
        Action::DropConnection => 4,
        Action::DropDatagram => 5,
        Action::RateLimit { .. } => 6,
        Action::Tag { .. } => 7,
        Action::Log => 8,
    }
}
pub fn rule_to_wire(r: &Rule) -> p::Rule {
    let (action, config) = match &r.action {
        Action::Allow => (1, None),
        Action::Alert => (2, None),
        Action::Block { http_status } => (
            3,
            Some(p::RuleActionConfig {
                http_status: Some((*http_status).into()),
                ..Default::default()
            }),
        ),
        Action::DropConnection => (4, None),
        Action::DropDatagram => (5, None),
        Action::RateLimit {
            key,
            requests,
            window_ms,
            on_exceed,
        } => (
            6,
            Some(p::RuleActionConfig {
                rate_limit: Some(p::RateLimitOptions {
                    key: key.clone(),
                    requests: *requests,
                    window_ms: *window_ms,
                    on_exceed: action_code(on_exceed),
                }),
                http_status: if let Action::Block { http_status } = on_exceed.as_ref() {
                    Some((*http_status).into())
                } else {
                    None
                },
                ..Default::default()
            }),
        ),
        Action::Tag { tag } => (
            7,
            Some(p::RuleActionConfig {
                tag: tag.clone(),
                ..Default::default()
            }),
        ),
        Action::Log => (8, None),
    };
    p::Rule {
        id: r.id.clone(),
        service_id: r.service_id.clone(),
        name: r.name.clone(),
        enabled: r.enabled,
        mode: if r.mode == Mode::Shadow { 1 } else { 2 },
        priority: r.priority,
        expression: Some(expression_to_wire(&r.expression)),
        action,
        backend: r.backend.clone(),
        description: r.description.clone(),
        action_config: config,
        expires_at: r.expires_at_ms.map(|ms| prost_types::Timestamp {
            seconds: (ms / 1000) as i64,
            nanos: ((ms % 1000) * 1_000_000) as i32,
        }),
    }
}
pub fn value_to_wire(value: &FieldValue) -> p::FieldValue {
    use p::field_value::Value as V;
    let value = match value {
        FieldValue::String(v) => V::StringValue(v.clone()),
        FieldValue::Int(v) => V::IntValue(*v),
        FieldValue::Uint(v) => V::UintValue(*v),
        FieldValue::Double(v) => V::DoubleValue(*v),
        FieldValue::Bool(v) => V::BoolValue(*v),
        FieldValue::Bytes(v) => V::BytesValue(v.clone()),
        FieldValue::Object(v) => V::ObjectValue(p::FieldObject {
            fields: v
                .iter()
                .map(|(k, v)| (k.clone(), value_to_wire(v)))
                .collect(),
        }),
        FieldValue::List(v) => V::ListValue(p::FieldList {
            values: v.iter().map(value_to_wire).collect(),
        }),
    };
    p::FieldValue { value: Some(value) }
}
fn expression_to_wire(e: &Expression) -> p::RuleExpression {
    use p::rule_expression::Node as N;
    let group = |v: &[Expression]| p::RuleGroup {
        children: v.iter().map(expression_to_wire).collect(),
    };
    let node = match e {
        Expression::All { all } => N::All(group(all)),
        Expression::Any { any } => N::Any(group(any)),
        Expression::Not { not } => N::Negated(Box::new(expression_to_wire(not))),
        Expression::Predicate { field, op, value } => N::Predicate(p::Predicate {
            field_path: field.clone(),
            comparison: match op {
                Comparison::Eq => 1,
                Comparison::Ne => 2,
                Comparison::Contains => 3,
                Comparison::Regex => 4,
                Comparison::Prefix => 5,
                Comparison::Gt => 6,
                Comparison::Lt => 7,
                Comparison::In => 8,
                Comparison::NotIn => 9,
            },
            expected: Some(value_to_wire(value)),
        }),
    };
    p::RuleExpression { node: Some(node) }
}
impl TryFrom<&p::ApplyRuleSnapshotRequest> for RuleSnapshot {
    type Error = String;
    fn try_from(s: &p::ApplyRuleSnapshotRequest) -> Result<Self, String> {
        if s.rules.len() > 256 {
            return Err("rule limit".into());
        }
        let rules = s
            .rules
            .iter()
            .map(|r| {
                let config = r.action_config.clone().unwrap_or_default();
                if config.rate_limit.is_some() && r.action != 6 {
                    return Err("rate_limit options require RATE_LIMIT action".into());
                }
                Ok(Rule {
                    id: r.id.clone(),
                    service_id: r.service_id.clone(),
                    name: r.name.clone(),
                    description: r.description.clone(),
                    enabled: r.enabled,
                    mode: match r.mode {
                        1 => Mode::Shadow,
                        2 => Mode::Enforce,
                        _ => return Err("invalid rule mode".into()),
                    },
                    priority: r.priority,
                    backend: r.backend.clone(),
                    expression: expression_from_wire(
                        r.expression.as_ref().ok_or("missing expression")?,
                        0,
                        &mut 0,
                    )?,
                    action: match r.action {
                        1 => Action::Allow,
                        2 => Action::Alert,
                        3 => Action::Block {
                            http_status: config
                                .http_status
                                .unwrap_or(403)
                                .try_into()
                                .map_err(|_| "HTTP status overflow")?,
                        },
                        4 => Action::DropConnection,
                        5 => Action::DropDatagram,
                        6 => {
                            let rate = config.rate_limit.ok_or("missing rate_limit options")?;
                            Action::RateLimit {
                                key: rate.key,
                                requests: rate.requests,
                                window_ms: rate.window_ms,
                                on_exceed: Box::new(match rate.on_exceed {
                                    3 => Action::Block {
                                        http_status: config
                                            .http_status
                                            .unwrap_or(429)
                                            .try_into()
                                            .map_err(|_| "HTTP status overflow")?,
                                    },
                                    4 => Action::DropConnection,
                                    5 => Action::DropDatagram,
                                    _ => return Err("invalid rate_limit on_exceed".into()),
                                }),
                            }
                        }
                        7 => Action::Tag { tag: config.tag },
                        8 => Action::Log,
                        _ => return Err("invalid or unsupported action".into()),
                    },
                    expires_at_ms: match &r.expires_at {
                        None => None,
                        Some(t) => {
                            if t.seconds < 0 || !(0..1_000_000_000).contains(&t.nanos) {
                                return Err("invalid expiry".into());
                            }
                            Some(
                                (t.seconds as u64)
                                    .checked_mul(1000)
                                    .and_then(|v| v.checked_add(t.nanos as u64 / 1_000_000))
                                    .ok_or("expiry overflow")?,
                            )
                        }
                    },
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            revision: s.revision,
            rules,
        })
    }
}
fn expression_from_wire(
    e: &p::RuleExpression,
    depth: usize,
    nodes: &mut usize,
) -> Result<Expression, String> {
    if depth > 16 || *nodes >= 256 {
        return Err("expression limit".into());
    }
    *nodes += 1;
    use p::rule_expression::Node as N;
    Ok(match e.node.as_ref().ok_or("missing node")? {
        N::All(v) => Expression::All {
            all: v
                .children
                .iter()
                .map(|e| expression_from_wire(e, depth + 1, nodes))
                .collect::<Result<_, _>>()?,
        },
        N::Any(v) => Expression::Any {
            any: v
                .children
                .iter()
                .map(|e| expression_from_wire(e, depth + 1, nodes))
                .collect::<Result<_, _>>()?,
        },
        N::Negated(v) => Expression::Not {
            not: Box::new(expression_from_wire(v, depth + 1, nodes)?),
        },
        N::Predicate(v) => Expression::Predicate {
            field: v.field_path.clone(),
            op: match v.comparison {
                1 => Comparison::Eq,
                2 => Comparison::Ne,
                3 => Comparison::Contains,
                4 => Comparison::Regex,
                5 => Comparison::Prefix,
                6 => Comparison::Gt,
                7 => Comparison::Lt,
                8 => Comparison::In,
                9 => Comparison::NotIn,
                _ => return Err("invalid comparison".into()),
            },
            value: value_from_wire(v.expected.as_ref().ok_or("missing expected")?, 0)?,
        },
    })
}
pub fn value_from_wire(v: &p::FieldValue, depth: usize) -> Result<FieldValue, String> {
    if depth > 16 {
        return Err("value nesting limit".into());
    }
    use p::field_value::Value as V;
    Ok(match v.value.as_ref().ok_or("missing value")? {
        V::StringValue(v) => FieldValue::String(v.clone()),
        V::IntValue(v) => FieldValue::Int(*v),
        V::UintValue(v) => FieldValue::Uint(*v),
        V::DoubleValue(v) if v.is_finite() => FieldValue::Double(*v),
        V::DoubleValue(_) => return Err("nonfinite value".into()),
        V::BoolValue(v) => FieldValue::Bool(*v),
        V::BytesValue(v) => FieldValue::Bytes(v.clone()),
        V::ObjectValue(v) => FieldValue::Object(
            v.fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), value_from_wire(v, depth + 1)?)))
                .collect::<Result<BTreeMap<_, _>, String>>()?,
        ),
        V::ListValue(v) => FieldValue::List(
            v.values
                .iter()
                .map(|v| value_from_wire(v, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_rule_round_trip_preserves_u64_and_nested_expression() {
        let snapshot = RuleSnapshot {
            revision: 42,
            rules: vec![Rule {
                id: "r".into(),
                service_id: "s".into(),
                name: "rule".into(),
                description: String::new(),
                enabled: true,
                mode: Mode::Shadow,
                priority: 7,
                expression: Expression::Not {
                    not: Box::new(Expression::Predicate {
                        field: "protocol.counter".into(),
                        op: Comparison::Eq,
                        value: FieldValue::Uint(u64::MAX),
                    }),
                },
                action: Action::Log,
                backend: "userspace".into(),
                expires_at_ms: Some(123456),
            }],
        };
        assert_eq!(
            RuleSnapshot::try_from(&p::ApplyRuleSnapshotRequest::from(&snapshot)).unwrap(),
            snapshot
        );
    }
}
