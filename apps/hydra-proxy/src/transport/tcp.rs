use crate::{
    config::Limits,
    inspection::{domain_value, network_fields},
    telemetry::{Flow, Telemetry},
    transport::{RelayBase, RelayTransport},
};
use hydra_domain::{Action, Service};
use hydra_protocol_sdk::Direction;
use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, watch},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
#[cfg(test)]
#[path = "tcp_tests.rs"]
mod tests;

pub async fn listen(
    listener: Arc<TcpListener>,
    mut service: watch::Receiver<Option<Service>>,
    limits: Limits,
    telemetry: Arc<Telemetry>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
) {
    loop {
        tokio::select! {
            biased;
            _=shutdown.cancelled()=>break,
            changed=service.changed()=>if changed.is_err() || service.borrow().is_none(){break},
            accepted=listener.accept()=>{
                let (client,addr)=match accepted{Ok(v)=>v,Err(error)=>{tracing::warn!(%error,"TCP accept failed");tokio::time::sleep(Duration::from_millis(50)).await;continue}};
                let Some(config)=service.borrow().clone() else{break};
                let budget=telemetry.budget(&config);
                let Ok(permit)=telemetry.permits.clone().try_acquire_owned() else{budget.reject();telemetry.rejected.fetch_add(1,Ordering::Relaxed);continue};
                let Some(lease)=budget.admit(true) else{telemetry.rejected.fetch_add(1,Ordering::Relaxed);continue};
                let flow=telemetry.open(&config,addr,permit);
                let limits=limits.clone();let shutdown=shutdown.clone();let telemetry=telemetry.clone();
                tasks.spawn(async move{
                    let mut flow=flow;
                    flow.protection=Some(lease);
                    let mut session=telemetry.protocols.read().unwrap_or_else(|e|e.into_inner()).as_ref().and_then(|p|match p.session(&config.id,&config.protocol_id){Ok(s)=>Some(s),Err(_)=>{telemetry.decoder_error(&config.id,&flow.id,"decoder_initialization_failed");None}});
                    flow.body_sizes.lock().unwrap_or_else(|e|e.into_inner()).set_http(session.as_mut().is_some_and(|s|s.http_mut().is_some()));
                    let decoder=Arc::new(Mutex::new(session));
                    let work=async{
                        if matches!(telemetry.evaluate(&config,&flow.id,&network_fields(&config,addr,"client_to_server"),false,""),Some(Action::DropConnection)){return Err(std::io::ErrorKind::PermissionDenied.into())}
                        let forwarding_store=telemetry.forwarding.read().unwrap_or_else(|e|e.into_inner()).clone();
                        let upstream=tokio::time::timeout(Duration::from_millis(limits.connect_timeout_ms),crate::forwarding::connect(&config, forwarding_store)).await??;
                        let mut client=client;
                        let(cr,cw)=client.split();let(ur,uw)=tokio::io::split(upstream);
                        let idle=Duration::from_millis(budget.idle(limits.tcp_idle_ms));
                        let (block_tx,block_rx)=mpsc::channel(1);
                        let context=Context{base:RelayBase::new(config.clone(),addr,telemetry.clone(),decoder.clone()),flow:&flow,idle,last_activity:Mutex::new(tokio::time::Instant::now())};
                        tokio::select! {
                            result=async { tokio::try_join!(client_to_server(cr,uw,&context,block_tx),server_to_client(ur,cw,&context,block_rx)) } => { result?; },
                            _=context.expire_when_idle()=>return Err(std::io::ErrorKind::TimedOut.into()),
                        }
                        Ok::<_,std::io::Error>(())
                    };
                    flow.reason=tokio::select!{_=shutdown.cancelled()=>"shutdown".into(),result=work=>match result{Ok(())=>"eof".into(),Err(error)=>format!("io:{:?}",error.kind())}};
                });
            }
        }
    }
}
struct Context<'a> {
    base: RelayBase,
    flow: &'a Flow,
    idle: Duration,
    last_activity: Mutex<tokio::time::Instant>,
}
impl Context<'_> {
    fn activity(&self) {
        *self.last_activity.lock().unwrap_or_else(|e| e.into_inner()) = tokio::time::Instant::now();
    }
    async fn expire_when_idle(&self) {
        loop {
            let elapsed = self
                .last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .elapsed();
            let remaining = self.idle.saturating_sub(elapsed);
            if remaining.is_zero() {
                return;
            }
            tokio::time::sleep(remaining).await;
        }
    }
}
impl RelayTransport for Context<'_> {
    fn base(&self) -> &RelayBase {
        &self.base
    }
}
impl std::ops::Deref for Context<'_> {
    type Target = RelayBase;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}
async fn write_active(
    c: &Context<'_>,
    write: &mut (impl AsyncWrite + Unpin),
    mut bytes: &[u8],
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let n = write.write(bytes).await?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        c.activity();
        bytes = &bytes[n..];
    }
    Ok(())
}
async fn client_to_server(
    mut read: impl AsyncRead + Unpin,
    mut write: impl AsyncWrite + Unpin,
    c: &Context<'_>,
    block: mpsc::Sender<u16>,
) -> std::io::Result<()> {
    let mut buf = [0; 16384];
    let mut pending_since: Option<tokio::time::Instant> = None;
    loop {
        // HTTP buffering keeps its fixed parser deadline independently of connection activity.
        let read_result = if let Some(start) = pending_since {
            tokio::time::timeout_at(start + Duration::from_secs(5), read.read(&mut buf)).await
        } else {
            Ok(read.read(&mut buf).await)
        };
        let n = match read_result {
            Ok(result) => result?,
            Err(_) if pending_since.is_some() => {
                let pending = {
                    let mut decoder = c.decoder.lock().unwrap_or_else(|e| e.into_inner());
                    let pending = decoder
                        .as_mut()
                        .and_then(|d| d.http_mut())
                        .map(|h| h.take_pending())
                        .unwrap_or_default();
                    *decoder = None;
                    pending
                };
                c.telemetry
                    .decoder_error(&c.config.id, &c.flow.id, "http_buffer_deadline");
                if !c.telemetry.request_checks.framing_failure(&c.config) {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                write_active(c, &mut write, &pending).await?;
                c.activity();
                c.flow.forwarded(pending.len(), true);
                pending_since = None;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if n == 0 {
            // A partial HTTP message must remain byte-transparent; no decision is claimed on it.
            let pending = c
                .decoder
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
                .and_then(|d| d.http_mut())
                .map(|h| h.take_pending())
                .unwrap_or_default();
            if !pending.is_empty() {
                if !c.telemetry.request_checks.framing_failure(&c.config) {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                write_active(c, &mut write, &pending).await?;
                c.activity();
                c.flow.forwarded(pending.len(), true);
            }
            write.shutdown().await?;
            return Ok(());
        }
        c.activity();
        if !c
            .flow
            .protection
            .as_ref()
            .is_none_or(|lease| lease.0.transfer(n, false))
        {
            c.telemetry.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        c.flow.record_history(Direction::ClientToServer, &buf[..n]);
        c.flow.scan(Direction::ClientToServer, &buf[..n]);
        enum Inspected {
            Raw,
            Frames(Vec<hydra_protocol_runtime::HttpRequest>, Option<Vec<u8>>),
        }
        let inspected = {
            let mut decoder = c.decoder.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(http) = decoder.as_mut().and_then(|d| d.http_mut()) {
                if inspect_transfer(c, Direction::ClientToServer, &buf[..n]) {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                let batch = http.feed_available(Direction::ClientToServer, &buf[..n]);
                if http.has_pending() {
                    if pending_since.is_none() || !batch.requests.is_empty() {
                        pending_since = Some(tokio::time::Instant::now());
                    }
                } else {
                    pending_since = None;
                }
                let fallback = if batch.failure.is_some() {
                    Some(http.take_pending())
                } else {
                    None
                };
                if fallback.is_some() {
                    pending_since = None;
                    *decoder = None;
                }
                Inspected::Frames(batch.requests, fallback)
            } else {
                Inspected::Raw
            }
        };
        match inspected {
            Inspected::Frames(frames, fallback) => {
                for frame in frames {
                    if !c
                        .telemetry
                        .request_checks
                        .allow(&c.config, &frame.bytes)
                        .await
                    {
                        return Err(std::io::ErrorKind::PermissionDenied.into());
                    }
                    let action = inspect_message(c, frame.message, frame.bytes.len(), true);
                    match action {
                        Some(Action::Block { http_status }) => {
                            if c.flow.up.load(Ordering::Relaxed) > 0 {
                                return Err(std::io::ErrorKind::PermissionDenied.into());
                            }
                            let _ = block.send(http_status).await;
                            write.shutdown().await?;
                            return Ok(());
                        }
                        Some(Action::DropConnection) => {
                            return Err(std::io::ErrorKind::PermissionDenied.into());
                        }
                        _ => {
                            write_active(c, &mut write, &frame.bytes).await?;
                            c.flow.forwarded(frame.bytes.len(), true);
                        }
                    }
                }
                if let Some(bytes) = fallback {
                    c.telemetry
                        .decoder_error(&c.config.id, &c.flow.id, "http_decoder_disabled");
                    if !c.telemetry.request_checks.framing_failure(&c.config) {
                        return Err(std::io::ErrorKind::PermissionDenied.into());
                    }
                    write_active(c, &mut write, &bytes).await?;
                    c.flow.forwarded(bytes.len(), true);
                }
            }
            Inspected::Raw => {
                let allowed = if c.config.protocol_id == "http" {
                    c.telemetry.request_checks.framing_failure(&c.config)
                } else {
                    c.telemetry.request_checks.allow(&c.config, &buf[..n]).await
                };
                if !allowed || inspect_raw(c, Direction::ClientToServer, &buf[..n]) {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                write_active(c, &mut write, &buf[..n]).await?;
                c.flow.forwarded(n, true);
            }
        }
    }
}
async fn server_to_client(
    mut read: impl AsyncRead + Unpin,
    mut write: impl AsyncWrite + Unpin,
    c: &Context<'_>,
    mut block: mpsc::Receiver<u16>,
) -> std::io::Result<()> {
    let mut buf = [0; 16384];
    let mut block_open = true;
    loop {
        let n = tokio::select! {
            biased;
            status=block.recv(),if block_open=>{
                if let Some(status)=status{
                    let response=format!("HTTP/1.1 {status} Blocked\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    write_active(c, &mut write, response.as_bytes()).await?;write.shutdown().await?;return Ok(())
                }else{block_open=false;continue}
            },
            result=read.read(&mut buf)=>result?,
        };
        if n == 0 {
            let result = c
                .decoder
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
                .and_then(|d| d.http_mut())
                .map(|h| h.finish_response());
            if let Some(result) = result {
                match result {
                    Ok(frames) => {
                        for frame in frames {
                            inspect_message(c, frame.message, frame.bytes.len(), false);
                        }
                    }
                    Err(_) => c.telemetry.decoder_error(
                        &c.config.id,
                        &c.flow.id,
                        "http_response_decoder_disabled",
                    ),
                }
            }
            write.shutdown().await?;
            return Ok(());
        }
        c.activity();
        if !c
            .flow
            .protection
            .as_ref()
            .is_none_or(|lease| lease.0.transfer(n, false))
        {
            c.telemetry.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        c.flow.record_history(Direction::ServerToClient, &buf[..n]);
        c.flow.scan(Direction::ServerToClient, &buf[..n]);
        if inspect_raw(c, Direction::ServerToClient, &buf[..n]) {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        write_active(c, &mut write, &buf[..n]).await?;
        c.activity();
        c.flow.forwarded(n, false);
    }
}
fn inspect_message(
    c: &Context<'_>,
    mut message: hydra_protocol_sdk::ParsedMessage,
    size: usize,
    safe_http: bool,
) -> Option<Action> {
    c.flow
        .body_sizes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .annotate(&mut message, "stream");
    message.fields.insert(
        "packet.size".into(),
        hydra_protocol_sdk::Value::Unsigned(size as u64),
    );
    let direction = if message.direction == Direction::ClientToServer {
        "client_to_server"
    } else {
        "server_to_client"
    };
    let mut fields = c.network_fields(direction);
    fields.insert(
        "packet.size".into(),
        hydra_domain::FieldValue::Uint(size as u64),
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
        c.telemetry.message(&c.config.id, &c.flow.id, message, size)
    };
    if !raw_payload {
        c.flow.reported_message.store(true, Ordering::Relaxed);
    }
    c.telemetry
        .evaluate(&c.config, &c.flow.id, &fields, safe_http, &message_id)
}
fn inspect_raw(c: &Context<'_>, direction: Direction, bytes: &[u8]) -> bool {
    if inspect_transfer(c, direction, bytes) {
        return true;
    }
    let result = {
        let mut decoder = c.decoder.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = decoder.as_mut() {
            if direction == Direction::ServerToClient {
                if let Some(http) = d.http_mut() {
                    Some(
                        http.feed(direction, bytes)
                            .map_err(anyhow::Error::from)
                            .map(|frames| {
                                frames
                                    .into_iter()
                                    .map(|f| (f.message, f.bytes.len()))
                                    .collect()
                            }),
                    )
                } else if d.is_stream_decoder() {
                    Some(d.feed(direction, bytes).map(|messages| {
                        messages
                            .into_iter()
                            .map(|m| (m, bytes.len()))
                            .collect::<Vec<_>>()
                    }))
                } else {
                    None
                }
            } else if d.is_stream_decoder() {
                Some(d.feed(direction, bytes).map(|messages| {
                    messages
                        .into_iter()
                        .map(|m| (m, bytes.len()))
                        .collect::<Vec<_>>()
                }))
            } else {
                None
            }
        } else {
            None
        }
    };
    if let Some(result) = result {
        match result {
            Ok(messages) => {
                let emitted = !messages.is_empty();
                for (message, size) in messages {
                    if matches!(
                        inspect_message(c, message, size, false),
                        Some(Action::DropConnection)
                    ) {
                        return true;
                    }
                }
                if emitted {
                    return false;
                }
            }
            Err(_) => {
                let mut decoder = c.decoder.lock().unwrap_or_else(|e| e.into_inner());
                let native_http = decoder.as_mut().is_some_and(|d| d.http_mut().is_some());
                // A passive response failure must not discard buffered client requests.
                if !native_http {
                    *decoder = None;
                }
                c.telemetry.decoder_error(
                    &c.config.id,
                    &c.flow.id,
                    if native_http {
                        "http_response_decoder_disabled"
                    } else {
                        "protocol_decoder_disabled"
                    },
                );
            }
        }
    }
    let direction = if direction == Direction::ClientToServer {
        "client_to_server"
    } else {
        "server_to_client"
    };
    let mut fields = c.network_fields(direction);
    fields.insert(
        "packet.size".into(),
        hydra_domain::FieldValue::Uint(bytes.len() as u64),
    );
    matches!(
        c.telemetry
            .evaluate(&c.config, &c.flow.id, &fields, false, ""),
        Some(Action::DropConnection)
    )
}
fn inspect_transfer(c: &Context<'_>, direction: Direction, bytes: &[u8]) -> bool {
    let mut sizes = c.flow.body_sizes.lock().unwrap_or_else(|e| e.into_inner());
    sizes.feed(direction, bytes, |size_fields| {
        let mut fields = c.network_fields(if direction == Direction::ClientToServer {
            "client_to_server"
        } else {
            "server_to_client"
        });
        fields.extend(size_fields.clone());
        matches!(
            c.telemetry
                .evaluate_stream_sizes(&c.config, &c.flow.id, &fields),
            Some(Action::DropConnection)
        )
    })
}
