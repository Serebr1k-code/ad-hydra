pub mod critical;
use hydra_domain::{Service, Transport};
use hydra_wire::v1::{self, event::Payload};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};

pub fn timestamp() -> prost_types::Timestamp {
    SystemTime::now().into()
}

pub struct Telemetry {
    pub forwarding: std::sync::RwLock<Option<Arc<crate::forwarding::Store>>>,
    pub budgets: std::sync::RwLock<
        std::collections::HashMap<String, Arc<crate::transport::protection::Budget>>,
    >,
    pub samples: std::sync::RwLock<Option<Arc<crate::samples::SampleRing>>>,
    pub history: std::sync::RwLock<Option<Arc<crate::history::HistoryStore>>>,
    pub flows: std::sync::Mutex<std::collections::BTreeSet<String>>,
    pub scanner: std::sync::RwLock<Option<Arc<crate::inspection::flags::Scanner>>>,
    pub critical: std::sync::RwLock<Option<Arc<critical::CriticalSpool>>>,
    pub protocols: std::sync::RwLock<Option<Arc<crate::inspection::protocols::Protocols>>>,
    pub hit_counts: std::sync::Mutex<std::collections::BTreeMap<String, u64>>,
    pub rules: std::sync::RwLock<Arc<hydra_filter_engine::CompiledSnapshot>>,
    pub epoch: String,
    pub started_at: prost_types::Timestamp,
    pub active: AtomicU64,
    pub dropped: AtomicU64,
    pub rejected: AtomicU64,
    pub udp_dropped: AtomicU64,
    pub events: broadcast::Sender<v1::Event>,
    pub permits: Arc<Semaphore>,
    pub subscribers: Arc<Semaphore>,
}
impl Telemetry {
    pub fn new(max_flows: usize) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            forwarding: Default::default(),
            budgets: Default::default(),
            samples: std::sync::RwLock::new(None),
            history: std::sync::RwLock::new(None),
            flows: std::sync::Mutex::new(Default::default()),
            scanner: std::sync::RwLock::new(None),
            critical: std::sync::RwLock::new(None),
            protocols: std::sync::RwLock::new(None),
            hit_counts: std::sync::Mutex::new(Default::default()),
            rules: std::sync::RwLock::new(Arc::new(
                hydra_filter_engine::CompiledSnapshot::compile(
                    hydra_domain::RuleSnapshot::default(),
                    &[],
                )
                .expect("empty rules"),
            )),
            epoch: uuid::Uuid::new_v4().to_string(),
            started_at: timestamp(),
            active: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            udp_dropped: AtomicU64::new(0),
            events,
            permits: Arc::new(Semaphore::new(max_flows)),
            subscribers: Arc::new(Semaphore::new(4)),
        })
    }
    pub fn drop_udp(&self, budget: &crate::transport::protection::Budget) {
        budget.dropped_datagram();
        self.udp_dropped.fetch_add(1, Ordering::Relaxed);
    }
    pub fn configure_protection(&self, services: &[Service]) {
        let mut budgets = self.budgets.write().unwrap_or_else(|e| e.into_inner());
        budgets.retain(|id, b| services.iter().any(|s| s.id == *id) || Arc::strong_count(b) > 1);
        for service in services {
            budgets
                .entry(service.id.clone())
                .or_insert_with(|| crate::transport::protection::Budget::new(&service.id))
                .configure(service.protection.clone());
        }
    }
    pub fn budget(&self, service: &Service) -> Arc<crate::transport::protection::Budget> {
        if let Some(b) = self
            .budgets
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&service.id)
        {
            return b.clone();
        }
        let mut budgets = self.budgets.write().unwrap_or_else(|e| e.into_inner());
        budgets
            .entry(service.id.clone())
            .or_insert_with(|| {
                let b = crate::transport::protection::Budget::new(&service.id);
                b.configure(service.protection.clone());
                b
            })
            .clone()
    }
    pub fn apply_history_services(&self, services: &[Service]) {
        if let Some(history) = self
            .history
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            history.apply_services(services);
        }
    }
    pub fn decoder_error(&self, service: &str, flow: &str, reason: &str) {
        self.emit(
            service,
            flow,
            Payload::Loss(v1::EventLoss {
                count: 1,
                reason: reason.into(),
            }),
        );
    }
    pub fn replace_rules(&self, compiled: hydra_filter_engine::CompiledSnapshot) {
        let ids: std::collections::BTreeSet<_> = compiled.rule_ids().map(str::to_owned).collect();
        let mut counts = self.hit_counts.lock().unwrap_or_else(|e| e.into_inner());
        counts.retain(|id, _| ids.contains(id));
        for id in ids {
            counts.entry(id).or_insert(0);
        }
        drop(counts);
        *self.rules.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(compiled);
    }
    pub fn evaluate(
        &self,
        service: &Service,
        flow_id: &str,
        fields: &std::collections::BTreeMap<String, hydra_domain::FieldValue>,
        safe_http: bool,
        message_id: &str,
    ) -> Option<hydra_domain::Action> {
        let rules = self.rules.read().unwrap_or_else(|e| e.into_inner()).clone();
        let result = rules.evaluate(&hydra_domain::EvaluationContext {
            service_id: &service.id,
            transport: service.transport,
            fields,
            now_ms: crate::state::now().saturating_mul(1000),
            safe_http,
        });
        self.record_evaluation(service, flow_id, message_id, result)
    }
    pub fn evaluate_stream_sizes(
        &self,
        service: &Service,
        flow_id: &str,
        fields: &std::collections::BTreeMap<String, hydra_domain::FieldValue>,
    ) -> Option<hydra_domain::Action> {
        let rules = self.rules.read().unwrap_or_else(|e| e.into_inner()).clone();
        let result = rules.evaluate_stream_sizes(&hydra_domain::EvaluationContext {
            service_id: &service.id,
            transport: service.transport,
            fields,
            now_ms: crate::state::now().saturating_mul(1000),
            safe_http: false,
        });
        self.record_evaluation(service, flow_id, "", result)
    }
    fn record_evaluation(
        &self,
        service: &Service,
        flow_id: &str,
        message_id: &str,
        result: hydra_domain::Evaluation,
    ) -> Option<hydra_domain::Action> {
        if result.state_exhausted {
            self.decoder_error(&service.id, flow_id, "rate_limit_state_exhausted");
        }
        for hit in result.hits {
            if let Some(count) = self
                .hit_counts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(&hit.rule_id)
            {
                *count = count.saturating_add(1);
            }
            self.emit(
                &service.id,
                flow_id,
                Payload::RuleHit(v1::RuleHit {
                    blocked: hit.blocked,
                    rule_id: hit.rule_id,
                    action: hydra_wire::rules::action_code(&hit.action),
                    mode: if hit.mode == hydra_domain::Mode::Shadow {
                        1
                    } else {
                        2
                    },
                    message_id: message_id.into(),
                }),
            );
        }
        result.action
    }
    pub fn message(
        &self,
        service: &str,
        flow: &str,
        message: hydra_protocol_sdk::ParsedMessage,
        size: usize,
    ) -> String {
        let message_id = uuid::Uuid::new_v4().to_string();
        let direction = message.direction;
        let fields = message
            .fields
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    hydra_wire::rules::value_to_wire(&crate::inspection::domain_value(v)),
                )
            })
            .collect();
        if let Some(scanner) = self
            .scanner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let values = message
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), crate::inspection::domain_value(v)))
                .collect();
            self.findings(
                service,
                flow,
                &message_id,
                direction,
                scanner.scan_fields(&values),
            );
        }
        self.emit(
            service,
            flow,
            Payload::Message(v1::ParsedMessage {
                message_id: message_id.clone(),
                protocol_id: message.protocol,
                message_type: message.message_type,
                direction: match message.direction {
                    hydra_protocol_sdk::Direction::ClientToServer => 1,
                    hydra_protocol_sdk::Direction::ServerToClient => 2,
                },
                fields: Some(v1::FieldObject { fields }),
                original_size: size.min(u32::MAX as usize) as u32,
                sample_id: String::new(),
                truncated: false,
            }),
        );
        message_id
    }
    pub fn findings(
        &self,
        service: &str,
        flow: &str,
        message: &str,
        direction: hydra_protocol_sdk::Direction,
        values: Vec<String>,
    ) {
        if values.len() == 256 {
            self.decoder_error(service, flow, "flag_findings_limit");
        }
        for _value in values {
            self.emit(
                service,
                flow,
                Payload::Flag(v1::FlagFinding {
                    finding_id: uuid::Uuid::new_v4().to_string(),
                    flag_value: String::new(),
                    source_kind: "own_exposed".into(),
                    message_id: message.into(),
                    direction: if direction == hydra_protocol_sdk::Direction::ClientToServer {
                        1
                    } else {
                        2
                    },
                }),
            );
        }
    }
    pub fn emit(&self, service: &str, flow: &str, payload: Payload) {
        let event = v1::Event {
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at: Some(timestamp()),
            service_id: service.into(),
            flow_id: flow.into(),
            payload: Some(payload),
        };
        if matches!(event.payload, Some(Payload::Flag(_))) {
            if let Some(spool) = self
                .critical
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                spool.try_emit(event);
            } else {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        if self.events.send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn open(
        self: &Arc<Self>,
        service: &Service,
        client: SocketAddr,
        permit: OwnedSemaphorePermit,
    ) -> Flow {
        let id = uuid::Uuid::new_v4().to_string();
        let history = if service.retain_history {
            self.history
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(|store| {
                    if service.transport == Transport::Udp {
                        store.begin_udp(id.clone(), service.id.clone(), service.protocol_id.clone())
                    } else {
                        store.begin(id.clone(), service.id.clone(), service.protocol_id.clone())
                    }
                })
        } else {
            None
        };
        let history_truncated = service.retain_history && history.is_none();
        let flow = Flow {
            protection: None,
            scanner: std::sync::Mutex::new(
                self.scanner
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .map(|s| s.session()),
            ),
            transport: service.transport,
            id,
            service: service.id.clone(),
            telemetry: self.clone(),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            body_sizes: std::sync::Mutex::new(crate::inspection::body_size::BodySizes::default()),
            reported_message: std::sync::atomic::AtomicBool::new(false),
            reason: "closed".into(),
            history: std::sync::Mutex::new(history),
            history_truncated: std::sync::atomic::AtomicBool::new(history_truncated),
            _permit: permit,
        };
        self.flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(flow.id.clone());
        self.active.fetch_add(1, Ordering::Relaxed);
        self.emit(
            &flow.service,
            &flow.id,
            Payload::FlowOpened(v1::FlowOpened {
                transport: match service.transport {
                    Transport::Tcp => 1,
                    Transport::Udp => 2,
                },
                client: Some(endpoint(client)),
                upstream: Some(endpoint(service.effective_upstream())),
            }),
        );
        flow
    }
}
fn endpoint(addr: SocketAddr) -> v1::Endpoint {
    v1::Endpoint {
        ip: addr.ip().to_string(),
        port: addr.port().into(),
    }
}
pub struct Flow {
    pub protection: Option<crate::transport::protection::Lease>,
    scanner: std::sync::Mutex<Option<crate::inspection::flags::FlagSession>>,
    transport: Transport,
    pub id: String,
    service: String,
    telemetry: Arc<Telemetry>,
    pub up: AtomicU64,
    pub down: AtomicU64,
    pub body_sizes: std::sync::Mutex<crate::inspection::body_size::BodySizes>,
    pub reported_message: std::sync::atomic::AtomicBool,
    pub reason: String,
    history: std::sync::Mutex<Option<crate::history::HistoryWriter>>,
    history_truncated: std::sync::atomic::AtomicBool,
    _permit: OwnedSemaphorePermit,
}
impl Flow {
    pub fn forwarded(&self, bytes: usize, up: bool) {
        if up {
            self.up.fetch_add(bytes as u64, Ordering::Relaxed);
        } else {
            self.down.fetch_add(bytes as u64, Ordering::Relaxed);
        }
        if let Some(lease) = &self.protection {
            lease.0.forwarded(bytes, up);
        }
    }

    pub fn record_history(&self, direction: hydra_protocol_sdk::Direction, bytes: &[u8]) {
        if let Some(history) = self
            .history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            history.record(direction, bytes);
        }
    }
    pub fn scan(&self, direction: hydra_protocol_sdk::Direction, bytes: &[u8]) {
        if let Some(samples) = self
            .telemetry
            .samples
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            samples.try_capture(&self.id, bytes);
        }
        let values = if self.transport == Transport::Udp {
            self.telemetry
                .scanner
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(|s| s.scan(bytes))
                .unwrap_or_default()
        } else {
            self.scanner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
                .map(|s| s.feed(direction, bytes))
                .unwrap_or_default()
        };
        self.telemetry
            .findings(&self.service, &self.id, "", direction, values);
    }
}
impl Drop for Flow {
    fn drop(&mut self) {
        if !self.reported_message.load(Ordering::Relaxed) {
            let sizes = self.body_sizes.get_mut().unwrap_or_else(|e| e.into_inner());
            for direction in [
                hydra_protocol_sdk::Direction::ClientToServer,
                hydra_protocol_sdk::Direction::ServerToClient,
            ] {
                if let Some((message, size)) = sizes.summary(
                    direction,
                    if self.transport == Transport::Udp {
                        "datagram"
                    } else {
                        "stream"
                    },
                ) {
                    self.telemetry
                        .message(&self.service, &self.id, message, size);
                }
            }
        }
        let history_truncated = self
            .history
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .is_some_and(|history| {
                history.finish(
                    SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        .min(i64::MAX as u128) as i64,
                    self.up.load(Ordering::Relaxed),
                    self.down.load(Ordering::Relaxed),
                )
            })
            || self.history_truncated.load(Ordering::Relaxed);
        self.telemetry
            .flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.telemetry.active.fetch_sub(1, Ordering::Relaxed);
        self.telemetry.emit(
            &self.service,
            &self.id,
            Payload::FlowClosed(v1::FlowClosed {
                bytes_client_to_server: self.up.load(Ordering::Relaxed),
                bytes_server_to_client: self.down.load(Ordering::Relaxed),
                reason: self.reason.clone(),
                history_truncated,
            }),
        );
    }
}
