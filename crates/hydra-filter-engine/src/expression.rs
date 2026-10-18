use crate::*;
use regex::{Regex, RegexBuilder};
use std::{cmp::Ordering, collections::BTreeMap};
#[derive(Debug)]
pub(crate) enum CompiledExpression {
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
    Predicate {
        field: String,
        op: Comparison,
        value: FieldValue,
        regex: Option<Regex>,
    },
}
#[derive(Clone, Copy, PartialEq)]
enum Truth {
    Yes,
    No,
    Unknown,
}
impl CompiledExpression {
    pub fn compile(
        expr: &Expression,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<Self, CompileError> {
        *nodes += 1;
        if depth > MAX_DEPTH || *nodes > MAX_NODES {
            return Err(invalid("expression exceeds depth/node limit"));
        }
        Ok(match expr {
            Expression::All { all } | Expression::Any { any: all } => {
                if all.is_empty() {
                    return Err(invalid("empty expression group"));
                }
                let children = all
                    .iter()
                    .map(|e| Self::compile(e, depth + 1, nodes))
                    .collect::<Result<Vec<_>, _>>()?;
                if matches!(expr, Expression::All { .. }) {
                    Self::All(children)
                } else {
                    Self::Any(children)
                }
            }
            Expression::Not { not } => Self::Not(Box::new(Self::compile(not, depth + 1, nodes)?)),
            Expression::Predicate { field, op, value } => {
                if field.is_empty() || field.len() > 256 || field.split('.').any(str::is_empty) {
                    return Err(invalid("invalid field path"));
                }
                let registered = matches!(
                    field.as_str(),
                    "http.method"
                        | "http.path"
                        | "http.user_agent"
                        | "http.body_text"
                        | "http.body_length"
                        | "packet.size"
                        | "body.size"
                        | "body.size_scope"
                        | "transfer.bytes"
                        | "network.src_ip"
                        | "network.src_port"
                        | "network.dst_ip"
                        | "network.dst_port"
                        | "transport.kind"
                        | "direction"
                        | "raw.bytes"
                ) || field.starts_with("protocol.");
                if !registered {
                    return Err(invalid("unknown field path"));
                }
                if !bounded(value, 1, &mut 0) {
                    return Err(invalid("expected value exceeds limits"));
                }
                let regex = match op {
                    Comparison::Regex => {
                        let FieldValue::String(pattern) = value else {
                            return Err(invalid("regex requires string"));
                        };
                        if pattern.len() > MAX_PATTERN_BYTES {
                            return Err(invalid("regex pattern exceeds 1024 bytes"));
                        }
                        Some(
                            RegexBuilder::new(pattern)
                                .size_limit(MAX_REGEX_COMPILED_BYTES)
                                .dfa_size_limit(MAX_REGEX_COMPILED_BYTES)
                                .build()
                                .map_err(|_| invalid("invalid or oversized regex"))?,
                        )
                    }
                    Comparison::Contains | Comparison::Prefix
                        if !matches!(value, FieldValue::String(_) | FieldValue::Bytes(_)) =>
                    {
                        return Err(invalid("contains/prefix requires string or bytes"));
                    }
                    Comparison::Gt | Comparison::Lt
                        if !matches!(
                            value,
                            FieldValue::Int(_) | FieldValue::Uint(_) | FieldValue::Double(_)
                        ) =>
                    {
                        return Err(invalid("gt/lt requires number"));
                    }
                    Comparison::In | Comparison::NotIn if !matches!(value, FieldValue::List(_)) => {
                        return Err(invalid("in/not_in requires list"));
                    }
                    _ => None,
                };
                Self::Predicate {
                    field: field.clone(),
                    op: *op,
                    value: value.clone(),
                    regex,
                }
            }
        })
    }
    pub fn matches(&self, fields: &BTreeMap<String, FieldValue>) -> bool {
        self.evaluate(fields) == Truth::Yes
    }
    fn evaluate(&self, fields: &BTreeMap<String, FieldValue>) -> Truth {
        match self {
            Self::All(children) => {
                let mut unknown = false;
                for child in children {
                    match child.evaluate(fields) {
                        Truth::No => return Truth::No,
                        Truth::Unknown => unknown = true,
                        _ => {}
                    }
                }
                if unknown { Truth::Unknown } else { Truth::Yes }
            }
            Self::Any(children) => {
                let mut unknown = false;
                for child in children {
                    match child.evaluate(fields) {
                        Truth::Yes => return Truth::Yes,
                        Truth::Unknown => unknown = true,
                        _ => {}
                    }
                }
                if unknown { Truth::Unknown } else { Truth::No }
            }
            Self::Not(child) => match child.evaluate(fields) {
                Truth::Yes => Truth::No,
                Truth::No => Truth::Yes,
                Truth::Unknown => Truth::Unknown,
            },
            Self::Predicate {
                field,
                op,
                value,
                regex,
            } => {
                let Some(actual) = lookup(fields, field) else {
                    return Truth::Unknown;
                };
                if !bounded(actual, 1, &mut 0) {
                    return Truth::Unknown;
                }
                let result = match op {
                    Comparison::Eq | Comparison::Ne => {
                        if std::mem::discriminant(actual) != std::mem::discriminant(value) {
                            return Truth::Unknown;
                        }
                        let equal = actual == value;
                        if *op == Comparison::Eq { equal } else { !equal }
                    }
                    Comparison::Contains => match (actual, value) {
                        (FieldValue::String(a), FieldValue::String(b)) => a.contains(b),
                        (FieldValue::Bytes(a), FieldValue::Bytes(b)) => {
                            b.is_empty() || a.windows(b.len()).any(|v| v == b)
                        }
                        _ => return Truth::Unknown,
                    },
                    Comparison::Prefix => match (actual, value) {
                        (FieldValue::String(a), FieldValue::String(b)) => a.starts_with(b),
                        (FieldValue::Bytes(a), FieldValue::Bytes(b)) => a.starts_with(b),
                        _ => return Truth::Unknown,
                    },
                    Comparison::Regex => match actual {
                        FieldValue::String(a) => regex.as_ref().is_some_and(|r| r.is_match(a)),
                        _ => return Truth::Unknown,
                    },
                    Comparison::Gt | Comparison::Lt => {
                        let order = match (actual, value) {
                            (FieldValue::Int(a), FieldValue::Int(b)) => a.partial_cmp(b),
                            (FieldValue::Uint(a), FieldValue::Uint(b)) => a.partial_cmp(b),
                            (FieldValue::Double(a), FieldValue::Double(b)) => a.partial_cmp(b),
                            _ => None,
                        };
                        let Some(order) = order else {
                            return Truth::Unknown;
                        };
                        order
                            == if *op == Comparison::Gt {
                                Ordering::Greater
                            } else {
                                Ordering::Less
                            }
                    }
                    Comparison::In | Comparison::NotIn => {
                        let FieldValue::List(values) = value else {
                            return Truth::Unknown;
                        };
                        if !values
                            .iter()
                            .any(|v| std::mem::discriminant(v) == std::mem::discriminant(actual))
                        {
                            return Truth::Unknown;
                        }
                        let found = values.contains(actual);
                        if *op == Comparison::In { found } else { !found }
                    }
                };
                if result { Truth::Yes } else { Truth::No }
            }
        }
    }
}
fn lookup<'a>(fields: &'a BTreeMap<String, FieldValue>, path: &str) -> Option<&'a FieldValue> {
    // Parser events may expose flat canonical paths or nested typed objects.
    if let Some(value) = fields.get(path) {
        return Some(value);
    }
    let mut parts = path.split('.');
    let mut value = fields.get(parts.next()?)?;
    for part in parts {
        let FieldValue::Object(object) = value else {
            return None;
        };
        value = object.get(part)?;
    }
    Some(value)
}
pub(crate) fn bounded(value: &FieldValue, depth: usize, size: &mut usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    *size += 1;
    if *size > MAX_FIELD_BYTES {
        return false;
    }
    match value {
        FieldValue::String(v) => *size += v.len(),
        FieldValue::Bytes(v) => *size += v.len(),
        FieldValue::Double(v) if !v.is_finite() => return false,
        FieldValue::Object(v) => {
            for (key, value) in v {
                *size += key.len();
                if !bounded(value, depth + 1, size) {
                    return false;
                }
            }
        }
        FieldValue::List(v) => {
            for value in v {
                if !bounded(value, depth + 1, size) {
                    return false;
                }
            }
        }
        _ => {}
    }
    *size <= MAX_FIELD_BYTES
}
