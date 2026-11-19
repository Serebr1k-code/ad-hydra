use crate::Transport;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FieldValue {
    String(String),
    Int(#[serde(with = "crate::decimal")] i64),
    Uint(#[serde(with = "crate::decimal")] u64),
    Double(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    Object(BTreeMap<String, FieldValue>),
    List(Vec<FieldValue>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Eq,
    Ne,
    Contains,
    Prefix,
    Regex,
    Gt,
    Lt,
    In,
    NotIn,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDirection {
    ClientToServer,
    ServerToClient,
}
impl RuleDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientToServer => "client_to_server",
            Self::ServerToClient => "server_to_client",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Expression {
    Scoped {
        direction: RuleDirection,
        scoped: Box<Expression>,
    },
    All {
        all: Vec<Expression>,
    },
    Any {
        any: Vec<Expression>,
    },
    Not {
        not: Box<Expression>,
    },
    Predicate {
        field: String,
        op: Comparison,
        value: FieldValue,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Shadow,
    Enforce,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Allow,
    Alert,
    Tag {
        tag: String,
    },
    Log,
    Block {
        http_status: u16,
    },
    DropConnection,
    DropDatagram,
    RateLimit {
        key: String,
        requests: u32,
        window_ms: u64,
        on_exceed: Box<Action>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub service_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_service_ids: Vec<String>,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub mode: Mode,
    pub priority: u32,
    pub expression: Expression,
    pub action: Action,
    pub backend: String,
    pub expires_at_ms: Option<u64>,
}
impl Rule {
    pub fn service_ids(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.service_id.as_str())
            .chain(self.additional_service_ids.iter().map(String::as_str))
    }

    pub fn applies_to(&self, service_id: &str) -> bool {
        self.service_ids().any(|id| id == service_id)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSnapshot {
    pub revision: u64,
    pub rules: Vec<Rule>,
}
pub struct EvaluationContext<'a> {
    pub service_id: &'a str,
    pub transport: Transport,
    pub fields: &'a BTreeMap<String, FieldValue>,
    pub now_ms: u64,
    /// Caller guarantees complete parsed HTTP message buffered before forwarding.
    pub safe_http: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleHit {
    /// Actual enforced block/drop; false for shadow and unexceeded rate limits.
    pub blocked: bool,
    pub rule_id: String,
    pub mode: Mode,
    pub action: Action,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evaluation {
    pub hits: Vec<RuleHit>,
    pub action: Option<Action>,
    pub state_exhausted: bool,
}
