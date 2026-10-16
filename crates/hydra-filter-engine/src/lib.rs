//! Pure bounded rule evaluation. Compile before publishing an immutable `Arc<CompiledSnapshot>`.
mod budget;
mod expression;
mod rate;
use expression::CompiledExpression;
pub use hydra_domain::{
    Action, Comparison, Evaluation, EvaluationContext, Expression, FieldValue, Mode, Rule, RuleHit,
    RuleSnapshot,
};
use hydra_domain::{Service, Transport};
use std::{collections::HashSet, fmt};
pub const MAX_RULES: usize = 256;
pub const MAX_DEPTH: usize = 16;
pub const MAX_NODES: usize = 256;
pub const MAX_FIELD_BYTES: usize = 16 * 1024;
pub const MAX_PATTERN_BYTES: usize = 1024;
pub const MAX_SNAPSHOT_NODES: usize = 512;
pub const MAX_SNAPSHOT_REGEXES: usize = 16;
pub const MAX_SNAPSHOT_EXPECTED_BYTES: usize = 256 * 1024;
pub const MAX_REGEX_COMPILED_BYTES: usize = 256 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError(pub String);
impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CompileError {}
fn invalid(message: &str) -> CompileError {
    CompileError(message.into())
}
#[derive(Debug)]
struct CompiledRule {
    rule: Rule,
    transport: Transport,
    expression: CompiledExpression,
}
#[derive(Debug)]
pub struct CompiledSnapshot {
    pub revision: u64,
    rules: Vec<CompiledRule>,
    windows: std::sync::Mutex<rate::Windows>,
}
impl CompiledSnapshot {
    pub fn rule_ids(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().map(|r| r.rule.id.as_str())
    }

    pub fn compile(snapshot: RuleSnapshot, services: &[Service]) -> Result<Self, CompileError> {
        if snapshot.rules.len() > MAX_RULES {
            return Err(invalid("rule count exceeds 256"));
        }
        budget::validate(&snapshot)?;
        let mut ids = HashSet::new();
        let mut rules = Vec::new();
        for rule in snapshot.rules {
            if rule.id.is_empty() || rule.id.len() > 128 || !ids.insert(rule.id.clone()) {
                return Err(invalid("invalid or duplicate rule id"));
            }
            if rule.backend != "userspace" || rule.name.len() > 256 || rule.description.len() > 4096
            {
                return Err(invalid("unsupported backend or oversized metadata"));
            }
            let service = services
                .iter()
                .find(|s| s.id == rule.service_id)
                .ok_or_else(|| invalid("unknown service"))?;
            if let Action::RateLimit {
                key,
                requests,
                window_ms,
                on_exceed,
            } = &rule.action
                && (key.is_empty()
                    || key.len() > 128
                    || !(1..=1_000_000).contains(requests)
                    || !(50..=3_600_000).contains(window_ms)
                    || !matches!(
                        on_exceed.as_ref(),
                        Action::Block { .. } | Action::DropConnection | Action::DropDatagram
                    ))
            {
                return Err(invalid("invalid rate limit configuration"));
            }
            let enforcement = match &rule.action {
                Action::RateLimit { on_exceed, .. } => on_exceed.as_ref(),
                other => other,
            };
            match enforcement {
                Action::DropConnection if service.transport != Transport::Tcp => {
                    return Err(invalid("drop_connection requires TCP"));
                }
                Action::DropDatagram if service.transport != Transport::Udp => {
                    return Err(invalid("drop_datagram requires UDP"));
                }
                Action::Block { http_status }
                    if service.transport != Transport::Tcp
                        || service.protocol_id != "http"
                        || !(400..=599).contains(http_status) =>
                {
                    return Err(invalid("block requires HTTP/TCP and status 400..599"));
                }
                Action::Tag { tag } if tag.is_empty() || tag.len() > 128 => {
                    return Err(invalid("tag must contain 1..128 bytes"));
                }
                _ => {}
            }
            let expression = CompiledExpression::compile(&rule.expression, 1, &mut 0)?;
            rules.push(CompiledRule {
                rule,
                transport: service.transport,
                expression,
            });
        }
        rules.sort_by(|a, b| {
            b.rule
                .priority
                .cmp(&a.rule.priority)
                .then(a.rule.id.cmp(&b.rule.id))
        });
        Ok(Self {
            revision: snapshot.revision,
            rules,
            windows: std::sync::Mutex::new(Default::default()),
        })
    }
    pub fn evaluate(&self, context: &EvaluationContext<'_>) -> Evaluation {
        let mut result = Evaluation::default();
        for compiled in &self.rules {
            let r = &compiled.rule;
            if !r.enabled
                || r.service_id != context.service_id
                || compiled.transport != context.transport
                || r.expires_at_ms.is_some_and(|t| context.now_ms >= t)
            {
                continue;
            }
            let enforcement = match &r.action {
                Action::RateLimit { on_exceed, .. } => on_exceed.as_ref(),
                other => other,
            };
            if matches!(enforcement, Action::Block { .. }) && !context.safe_http {
                continue;
            }
            if compiled.expression.matches(context.fields) {
                let action = if let Action::RateLimit {
                    key,
                    requests,
                    window_ms,
                    on_exceed,
                } = &r.action
                {
                    let key = match context.fields.get(key) {
                        Some(FieldValue::String(v)) if v.len() <= 256 => v.clone(),
                        Some(FieldValue::Int(v)) => v.to_string(),
                        Some(FieldValue::Uint(v)) => v.to_string(),
                        _ => continue,
                    };
                    let exceeded = self
                        .windows
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .exceeds(&r.id, key, *requests, *window_ms, context.now_ms);
                    if exceeded.is_none() {
                        result.state_exhausted = true;
                    }
                    if exceeded.unwrap_or(true) {
                        Some(on_exceed.as_ref().clone())
                    } else {
                        None
                    }
                } else {
                    Some(r.action.clone())
                };
                result.hits.push(RuleHit {
                    rule_id: r.id.clone(),
                    mode: r.mode,
                    action: r.action.clone(),
                });
                if r.mode == Mode::Enforce
                    && matches!(
                        action,
                        Some(
                            Action::Allow
                                | Action::Block { .. }
                                | Action::DropConnection
                                | Action::DropDatagram
                        )
                    )
                {
                    result.action = action;
                    break;
                }
            }
        }
        result
    }
}
/// Backend abstraction deliberately independent of sockets, storage and control transport.
pub trait NetworkFilterBackend: Send + Sync {
    fn evaluate(&self, snapshot: &CompiledSnapshot, context: &EvaluationContext<'_>) -> Evaluation;
}
#[derive(Default)]
pub struct UserspaceBackend;
impl NetworkFilterBackend for UserspaceBackend {
    fn evaluate(&self, snapshot: &CompiledSnapshot, context: &EvaluationContext<'_>) -> Evaluation {
        snapshot.evaluate(context)
    }
}
