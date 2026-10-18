pub mod critical;
use anyhow::{Context, ensure};
use hydra_domain::ServiceSnapshot;
use hydra_wire::v1::{
    self, proxy_control_client::ProxyControlClient, proxy_telemetry_client::ProxyTelemetryClient,
};
use hyper_util::rt::TokioIo;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::net::UnixStream;
use tonic::{
    Request,
    transport::{Channel, Endpoint},
};
use tower::service_fn;

pub async fn connect(socket: &Path) -> anyhow::Result<Channel> {
    let path = socket.to_path_buf();
    Endpoint::from_static("http://[::]:50051")
        .connect_timeout(Duration::from_secs(2))
        .connect_with_connector(service_fn(move |_| {
            let path = path.clone();
            async move { UnixStream::connect(path).await.map(TokioIo::new) }
        }))
        .await
        .context("connect to Proxy UDS")
}
fn request<T>(value: T) -> Request<T> {
    let mut request = Request::new(value);
    request.set_timeout(Duration::from_secs(3));
    request
}
pub async fn status(channel: Channel) -> anyhow::Result<v1::ProxyStatus> {
    Ok(ProxyControlClient::new(channel)
        .get_status(request(v1::GetStatusRequest {}))
        .await?
        .into_inner())
}
pub async fn apply(
    channel: Channel,
    snapshot: &ServiceSnapshot,
    actor: v1::Actor,
    request_id: String,
) -> anyhow::Result<v1::ApplyResult> {
    let mut r = v1::ApplyServiceSnapshotRequest::from(snapshot);
    r.request_id = request_id;
    r.actor = Some(actor);
    Ok(ProxyControlClient::new(channel)
        .apply_service_snapshot(request(r))
        .await?
        .into_inner())
}
pub async fn events(channel: Channel) -> anyhow::Result<tonic::Streaming<v1::EventBatch>> {
    Ok(ProxyTelemetryClient::new(channel)
        .stream_telemetry(v1::StreamTelemetryRequest {
            max_events_per_batch: 64,
        })
        .await?
        .into_inner())
}
pub async fn load_snapshot(path: &Path) -> anyhow::Result<ServiceSnapshot> {
    ensure!(
        tokio::fs::metadata(path).await?.len() <= 256 * 1024,
        "snapshot exceeds 256 KiB"
    );
    let snapshot: ServiceSnapshot = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    snapshot.validate().map_err(anyhow::Error::msg)?;
    Ok(snapshot)
}
pub async fn reconcile(
    socket: PathBuf,
    mut history_policy: tokio::sync::watch::Receiver<u64>,
    view: crate::api::SharedView,
    shutdown: tokio_util::sync::CancellationToken,
    control: crate::modules::services::Control,
    traffic: crate::modules::traffic::Traffic,
    flags: crate::modules::flags::Flags,
) {
    loop {
        let result = session(
            &socket,
            &view,
            &shutdown,
            &control,
            &traffic,
            &flags,
            &mut history_policy,
        )
        .await;
        view.write().await.connected = false;
        if let Err(error) = result {
            tracing::warn!(%error,"proxy disconnected; retrying");
        }
        tokio::select! {_=shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(1))=>{}}
    }
}
async fn sync(channel: Channel, control: &crate::modules::services::Control) -> anyhow::Result<()> {
    let desired = control.desired().await?;
    let result = apply_config(channel, &desired.services, &desired.rules).await?;
    let error = if result.applied {
        None
    } else {
        Some(result.error_message)
    };
    control
        .mark_services(desired.services.revision, error.clone())
        .await?;
    control.mark_rules(desired.rules.revision, error).await?;
    Ok(())
}
async fn session(
    socket: &Path,
    view: &crate::api::SharedView,
    shutdown: &tokio_util::sync::CancellationToken,
    control: &crate::modules::services::Control,
    traffic: &crate::modules::traffic::Traffic,
    flags: &crate::modules::flags::Flags,
    history_policy: &mut tokio::sync::watch::Receiver<u64>,
) -> anyhow::Result<()> {
    let channel = connect(socket).await?;
    let history_bytes = *history_policy.borrow_and_update();
    configure_history(channel.clone(), history_bytes).await?;
    sync(channel.clone(), control).await?;
    let mut stream = events(channel.clone()).await?;
    let mut critical =
        hydra_wire::v1::proxy_telemetry_client::ProxyTelemetryClient::new(channel.clone())
            .stream_critical(v1::StreamCriticalRequest::default())
            .await?
            .into_inner();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _=shutdown.cancelled()=>return Ok(()),
            changed=history_policy.changed()=>{changed.context("history policy closed")?;let bytes=*history_policy.borrow_and_update();configure_history(channel.clone(),bytes).await?;},
            _=control.changed.notified()=>sync(channel.clone(),control).await?,
            _=tick.tick()=>{
                let status=status(channel.clone()).await?;
                let history_bytes=*history_policy.borrow();
                if status.history_per_service_bytes!=history_bytes { configure_history(channel.clone(),history_bytes).await?; }
                control.observe_counters(status.proxy_epoch_id.clone(),status.rule_hit_counts.clone()).await?;
                let desired=control.desired().await?;
                if status.applied_services_revision!=desired.services.revision || status.applied_rules_revision!=desired.rules.revision {
                    sync(channel.clone(),control).await?;
                }
                let mut current=view.write().await;let events=current.events_received;let indexing_dropped=current.indexing_dropped;
                *current=crate::api::ProxyView::from_status(status);current.events_received=events;current.indexing_dropped=indexing_dropped;
            },
            next=critical.message()=>{
                let batch=next?.context("critical stream closed")?;
                critical::commit(flags,traffic,&batch).await?;
                let through_sequence=batch.first_sequence+batch.events.len() as u64-1;
                hydra_wire::v1::proxy_telemetry_client::ProxyTelemetryClient::new(channel.clone()).ack_critical(v1::AckCriticalRequest{proxy_epoch_id:batch.proxy_epoch_id,through_sequence}).await?;
            },
            next=stream.message()=>{
                let batch=next?.context("telemetry stream closed")?;
                view.write().await.events_received+=batch.events.len() as u64;
                if let Err(error)=traffic.ingest(batch.clone()).await {
                    let mut v=view.write().await;let old=v.indexing_dropped;v.indexing_dropped=v.indexing_dropped.saturating_add(batch.events.len() as u64);
                    if old==0 || old/1000!=v.indexing_dropped/1000 {tracing::warn!(%error,dropped=v.indexing_dropped,"traffic indexing failed; normal telemetry lost");}
                }

            },
        }
    }
}
pub fn system_actor() -> v1::Actor {
    v1::Actor {
        r#type: 2,
        username: "hydra-core".into(),
        ..Default::default()
    }
}

pub async fn apply_config(
    channel: Channel,
    services: &ServiceSnapshot,
    rules: &hydra_domain::RuleSnapshot,
) -> anyhow::Result<v1::ApplyResult> {
    let id = uuid::Uuid::new_v4().to_string();
    let mut services = v1::ApplyServiceSnapshotRequest::from(services);
    let mut rules = v1::ApplyRuleSnapshotRequest::from(rules);
    services.request_id = id.clone();
    services.actor = Some(system_actor());
    rules.request_id = id;
    rules.actor = services.actor.clone();
    Ok(ProxyControlClient::new(channel)
        .apply_config_snapshot(request(v1::ApplyConfigSnapshotRequest {
            services: Some(services),
            rules: Some(rules),
        }))
        .await?
        .into_inner())
}

async fn configure_history(channel: Channel, bytes: u64) -> anyhow::Result<()> {
    let response = ProxyControlClient::new(channel)
        .configure_history(request(v1::ConfigureHistoryRequest {
            per_service_bytes: bytes,
            actor: Some(system_actor()),
            request_id: uuid::Uuid::new_v4().to_string(),
        }))
        .await?
        .into_inner();
    ensure!(
        response.per_service_bytes == bytes,
        "proxy history policy was not applied"
    );
    Ok(())
}
