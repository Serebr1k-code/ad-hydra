use crate::{
    config::Limits,
    inspection::domain_value,
    telemetry::{Flow, Telemetry},
    transport::{RelayBase, RelayTransport},
};
use hydra_domain::{Action, Service};
use hydra_protocol_sdk::Direction;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{net::UdpSocket, sync::watch, task::JoinSet};
use tokio_util::sync::CancellationToken;
#[cfg(test)]
#[path = "udp_tests.rs"]
mod tests;
struct Session {
    socket: Arc<UdpSocket>,
    context: Arc<Context>,
    task: tokio::task::AbortHandle,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Context {
    base: RelayBase,
    flow: Flow,
    last: AtomicU64,
    start: Instant,
}
impl RelayTransport for Context {
    fn base(&self) -> &RelayBase {
        &self.base
    }
}
impl std::ops::Deref for Context {
    type Target = RelayBase;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}
impl Context {
    fn activity(&self) {
        self.last
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    fn inspect(&self, direction: Direction, bytes: &[u8]) -> bool {
        // Retain observed bytes before enforcement, matching TCP history.
        self.flow.record_history(direction, bytes);
        let size_fields = self
            .flow
            .body_sizes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .datagram(direction, bytes.len());
        let mut live_fields = self.network_fields(if direction == Direction::ClientToServer {
            "client_to_server"
        } else {
            "server_to_client"
        });
        live_fields.extend(size_fields);
        if matches!(
            self.telemetry
                .evaluate_stream_sizes(&self.config, &self.flow.id, &live_fields),
            Some(Action::DropDatagram)
        ) {
            self.telemetry.udp_dropped.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        self.flow.scan(direction, bytes);
        let mut decoder = self.decoder.lock().unwrap_or_else(|e| e.into_inner());
        let result = match decoder.as_mut() {
            Some(d) if d.is_stream_decoder() => Some(d.feed_datagram(direction, bytes)),
            _ => None,
        };
        if matches!(result, Some(Err(_))) {
            *decoder = None;
            self.telemetry.decoder_error(
                &self.config.id,
                &self.flow.id,
                "protocol_decoder_disabled",
            );
        }
        drop(decoder);
        let mut evaluated = false;
        if let Some(Ok(messages)) = result {
            for mut message in messages {
                self.flow
                    .body_sizes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .annotate(&mut message, "datagram");
                evaluated = true;
                let mut fields = self.network_fields(if direction == Direction::ClientToServer {
                    "client_to_server"
                } else {
                    "server_to_client"
                });
                fields.insert(
                    "packet.size".into(),
                    hydra_domain::FieldValue::Uint(bytes.len() as u64),
                );
                fields.extend(
                    message
                        .fields
                        .iter()
                        .map(|(k, v)| (k.clone(), domain_value(v))),
                );
                let raw_payload = message.protocol == "raw";
                let message_id = if raw_payload {
                    String::new()
                } else {
                    self.telemetry
                        .message(&self.config.id, &self.flow.id, message, bytes.len())
                };
                if !raw_payload {
                    self.flow.reported_message.store(true, Ordering::Relaxed);
                }
                let action = self.telemetry.evaluate(
                    &self.config,
                    &self.flow.id,
                    &fields,
                    false,
                    &message_id,
                );
                if matches!(action, Some(Action::DropDatagram)) {
                    self.telemetry.udp_dropped.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            }
        }
        if !evaluated
            && matches!(
                self.telemetry.evaluate(
                    &self.config,
                    &self.flow.id,
                    &{
                        // The fallback still has complete datagram byte metadata,
                        // even when a stateful decoder emits no parsed message.
                        live_fields.insert(
                            "packet.size".into(),
                            hydra_domain::FieldValue::Uint(bytes.len() as u64),
                        );
                        live_fields
                    },
                    false,
                    ""
                ),
                Some(Action::DropDatagram)
            )
        {
            self.telemetry.udp_dropped.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
    }
}
pub async fn listen(
    listener: Arc<UdpSocket>,
    mut service: watch::Receiver<Option<Service>>,
    limits: Limits,
    telemetry: Arc<Telemetry>,
    shutdown: CancellationToken,
) {
    if listener.writable().await.is_err() {
        return;
    }
    let start = Instant::now();
    let mut sessions: HashMap<SocketAddr, Session> = HashMap::new();
    let mut replies = JoinSet::new();
    // One extra byte detects oversized datagrams instead of forwarding a truncated prefix.
    let mut buf = vec![0u8; 65536];
    let mut tick = tokio::time::interval(limits.udp_idle().min(std::time::Duration::from_secs(1)));
    loop {
        tokio::select! {
         biased;
         _=shutdown.cancelled()=>break,
         changed=service.changed()=>{sessions.clear();if changed.is_err()||service.borrow().is_none(){break}},
         _=tick.tick()=>{let now=start.elapsed().as_millis() as u64;sessions.retain(|_,s|!s.task.is_finished()&&now.saturating_sub(s.context.last.load(Ordering::Relaxed))<limits.udp_idle_ms);},
         _=replies.join_next(),if !replies.is_empty()=>{},
         received=listener.recv_from(&mut buf)=>{
          let(n,client)=match received{Ok(v)=>v,Err(error)=>{tracing::warn!(%error,"UDP receive failed");continue}};
          if n>65535{telemetry.udp_dropped.fetch_add(1,Ordering::Relaxed);continue}
          if let std::collections::hash_map::Entry::Vacant(entry)=sessions.entry(client){
           let Some(config)=service.borrow().clone()else{break};
           let Ok(permit)=telemetry.permits.clone().try_acquire_owned()else{telemetry.rejected.fetch_add(1,Ordering::Relaxed);telemetry.udp_dropped.fetch_add(1,Ordering::Relaxed);continue};
           let bind=if config.upstream_address.is_ipv4(){"0.0.0.0:0"}else{"[::]:0"};
           let socket=match UdpSocket::bind(bind).await{Ok(s)=>Arc::new(s),Err(_)=>{telemetry.udp_dropped.fetch_add(1,Ordering::Relaxed);continue}};
           if socket.connect(config.upstream_address).await.is_err()||socket.writable().await.is_err(){telemetry.udp_dropped.fetch_add(1,Ordering::Relaxed);continue}
           let flow=telemetry.open(&config,client,permit);
           let decoder=telemetry.protocols.read().unwrap_or_else(|e|e.into_inner()).as_ref().and_then(|p|match p.session(&config.id,&config.protocol_id){Ok(s)=>Some(s),Err(_)=>{telemetry.decoder_error(&config.id,&flow.id,"decoder_initialization_failed");None}});
           let context=Arc::new(Context{base:RelayBase::new(config,client,telemetry.clone(),Arc::new(Mutex::new(decoder))),flow,last:AtomicU64::new(start.elapsed().as_millis() as u64),start});
           let task=replies.spawn(reply(socket.clone(),listener.clone(),context.clone()));
           entry.insert(Session{socket,context,task});
          }
          if let Some(s)=sessions.get(&client){
           s.context.activity();if s.context.inspect(Direction::ClientToServer,&buf[..n]){continue}
           match s.socket.try_send(&buf[..n]){Ok(sent)if sent==n=>{s.context.flow.up.fetch_add(n as u64,Ordering::Relaxed);},_=>{telemetry.udp_dropped.fetch_add(1,Ordering::Relaxed);}}
          }
         }
        }
    }
    sessions.clear();
    replies.shutdown().await;
}
async fn reply(socket: Arc<UdpSocket>, listener: Arc<UdpSocket>, context: Arc<Context>) {
    let mut buf = vec![0u8; 65536];
    while let Ok(n) = socket.recv(&mut buf).await {
        context.activity();
        if n > 65535 {
            context
                .telemetry
                .udp_dropped
                .fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if context.inspect(Direction::ServerToClient, &buf[..n]) {
            continue;
        }
        match listener.try_send_to(&buf[..n], context.client) {
            Ok(sent) if sent == n => {
                context.flow.down.fetch_add(n as u64, Ordering::Relaxed);
            }
            _ => {
                context
                    .telemetry
                    .udp_dropped
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
