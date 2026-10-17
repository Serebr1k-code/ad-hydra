use crate::{
    config::Limits,
    inspection::{domain_value, network_fields, protocols::ProtocolSession},
    telemetry::{Flow, Telemetry},
};
use hydra_domain::{Action, Service};
use hydra_protocol_sdk::Direction;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
type Decoder = Arc<Mutex<Option<ProtocolSession>>>;

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
                let Ok(permit)=telemetry.permits.clone().try_acquire_owned() else{telemetry.rejected.fetch_add(1,Ordering::Relaxed);continue};
                let flow=telemetry.open(&config,addr,permit);
                let limits=limits.clone();let shutdown=shutdown.clone();let telemetry=telemetry.clone();
                tasks.spawn(async move{
                    let mut flow=flow;
                    let session=telemetry.protocols.read().unwrap_or_else(|e|e.into_inner()).as_ref().and_then(|p|match p.session(&config.id,&config.protocol_id){Ok(s)=>Some(s),Err(_)=>{telemetry.decoder_error(&config.id,&flow.id,"decoder_initialization_failed");None}});
                    let decoder=Arc::new(Mutex::new(session));
                    let work=async{
                        if matches!(telemetry.evaluate(&config,&flow.id,&network_fields(&config,addr,"client_to_server"),false,""),Some(Action::DropConnection)){return Err(std::io::ErrorKind::PermissionDenied.into())}
                        let mut upstream=tokio::time::timeout(Duration::from_millis(limits.connect_timeout_ms),TcpStream::connect(config.upstream_address)).await??;
                        let mut client=client;
                        let(cr,cw)=client.split();let(ur,uw)=upstream.split();
                        let idle=Duration::from_millis(limits.tcp_idle_ms);
                        let (block_tx,block_rx)=mpsc::channel(1);
                        let context=Context{config:&config,client:addr,telemetry:&telemetry,flow:&flow,decoder:&decoder,idle};
                        tokio::try_join!(client_to_server(cr,uw,&context,block_tx),server_to_client(ur,cw,&context,block_rx))?;
                        Ok::<_,std::io::Error>(())
                    };
                    flow.reason=tokio::select!{_=shutdown.cancelled()=>"shutdown".into(),result=work=>match result{Ok(())=>"eof".into(),Err(error)=>format!("io:{:?}",error.kind())}};
                });
            }
        }
    }
}
struct Context<'a> {
    config: &'a Service,
    client: SocketAddr,
    telemetry: &'a Telemetry,
    flow: &'a Flow,
    decoder: &'a Decoder,
    idle: Duration,
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
        let wait = pending_since
            .map(|start| {
                Duration::from_secs(5)
                    .saturating_sub(start.elapsed())
                    .min(c.idle)
            })
            .unwrap_or(c.idle);
        let n = match tokio::time::timeout(wait, read.read(&mut buf)).await {
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
                tokio::time::timeout(c.idle, write.write_all(&pending)).await??;
                c.flow.up.fetch_add(pending.len() as u64, Ordering::Relaxed);
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
                tokio::time::timeout(c.idle, write.write_all(&pending)).await??;
                c.flow.up.fetch_add(pending.len() as u64, Ordering::Relaxed);
            }
            write.shutdown().await?;
            return Ok(());
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
                            tokio::time::timeout(c.idle, write.write_all(&frame.bytes)).await??;
                            c.flow
                                .up
                                .fetch_add(frame.bytes.len() as u64, Ordering::Relaxed);
                        }
                    }
                }
                if let Some(bytes) = fallback {
                    c.telemetry
                        .decoder_error(&c.config.id, &c.flow.id, "http_decoder_disabled");
                    tokio::time::timeout(c.idle, write.write_all(&bytes)).await??;
                    c.flow.up.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                }
            }
            Inspected::Raw => {
                if inspect_raw(c, Direction::ClientToServer, &buf[..n]) {
                    return Err(std::io::ErrorKind::PermissionDenied.into());
                }
                tokio::time::timeout(c.idle, write.write_all(&buf[..n])).await??;
                c.flow.up.fetch_add(n as u64, Ordering::Relaxed);
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
                    tokio::time::timeout(c.idle,write.write_all(response.as_bytes())).await??;write.shutdown().await?;return Ok(())
                }else{block_open=false;continue}
            },
            result=tokio::time::timeout(c.idle,read.read(&mut buf))=>result??,
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
        c.flow.record_history(Direction::ServerToClient, &buf[..n]);
        c.flow.scan(Direction::ServerToClient, &buf[..n]);
        if inspect_raw(c, Direction::ServerToClient, &buf[..n]) {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        tokio::time::timeout(c.idle, write.write_all(&buf[..n])).await??;
        c.flow.down.fetch_add(n as u64, Ordering::Relaxed);
    }
}
fn inspect_message(
    c: &Context<'_>,
    message: hydra_protocol_sdk::ParsedMessage,
    size: usize,
    safe_http: bool,
) -> Option<Action> {
    let direction = if message.direction == Direction::ClientToServer {
        "client_to_server"
    } else {
        "server_to_client"
    };
    let mut fields = network_fields(c.config, c.client, direction);
    fields.extend(
        message
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), domain_value(v))),
    );
    let message_id = c.telemetry.message(&c.config.id, &c.flow.id, message, size);
    c.telemetry
        .evaluate(c.config, &c.flow.id, &fields, safe_http, &message_id)
}
fn inspect_raw(c: &Context<'_>, direction: Direction, bytes: &[u8]) -> bool {
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
                    Some(
                        d.feed(direction, bytes).map(|messages| {
                            messages.into_iter().map(|m| (m, 0)).collect::<Vec<_>>()
                        }),
                    )
                } else {
                    None
                }
            } else if d.is_stream_decoder() {
                Some(
                    d.feed(direction, bytes)
                        .map(|messages| messages.into_iter().map(|m| (m, 0)).collect::<Vec<_>>()),
                )
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
    matches!(
        c.telemetry.evaluate(
            c.config,
            &c.flow.id,
            &network_fields(
                c.config,
                c.client,
                if direction == Direction::ClientToServer {
                    "client_to_server"
                } else {
                    "server_to_client"
                }
            ),
            false,
            ""
        ),
        Some(Action::DropConnection)
    )
}
