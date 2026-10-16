use super::Rpc;
use hydra_wire::v1::{self, event::Payload, proxy_telemetry_server::ProxyTelemetry};
use std::sync::atomic::Ordering;
use tokio::sync::{broadcast::error::RecvError, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

#[tonic::async_trait]
impl ProxyTelemetry for Rpc {
    type StreamTelemetryStream = ReceiverStream<Result<v1::EventBatch, Status>>;
    async fn stream_telemetry(
        &self,
        request: Request<v1::StreamTelemetryRequest>,
    ) -> Result<Response<Self::StreamTelemetryStream>, Status> {
        if request.into_inner().max_events_per_batch > 128 {
            return Err(Status::invalid_argument("batch cap is 128"));
        }
        let t = self.state.telemetry.clone();
        let permit = t
            .subscribers
            .clone()
            .try_acquire_owned()
            .map_err(|_| Status::resource_exhausted("at most four telemetry subscribers"))?;
        let mut events = t.events.subscribe();
        let (sender, receiver) = mpsc::channel(16);
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let _permit = permit;
            loop {
                let next = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = sender.closed() => break,
                    event = events.recv() => event,
                };
                let event = match next {
                    Ok(event) => event,
                    Err(RecvError::Lagged(n)) => {
                        t.dropped.fetch_add(n, Ordering::Relaxed);
                        v1::Event {
                            event_id: uuid::Uuid::new_v4().to_string(),
                            occurred_at: Some(crate::telemetry::timestamp()),
                            payload: Some(Payload::Loss(v1::EventLoss {
                                count: n,
                                reason: "subscriber_lag".into(),
                            })),
                            ..Default::default()
                        }
                    }
                    Err(RecvError::Closed) => break,
                };
                let batch = v1::EventBatch {
                    proxy_epoch_id: t.epoch.clone(),
                    events: vec![event],
                    ..Default::default()
                };
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    result = sender.send(Ok(batch)) => if result.is_err() { break; },
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }
    type StreamCriticalStream = ReceiverStream<Result<v1::EventBatch, Status>>;
    async fn stream_critical(
        &self,
        request: Request<v1::StreamCriticalRequest>,
    ) -> Result<Response<Self::StreamCriticalStream>, Status> {
        let request = request.into_inner();
        if request.proxy_epoch_id.len() > 64 {
            return Err(Status::invalid_argument("invalid epoch"));
        }
        let t = self.state.telemetry.clone();
        let permit = t
            .subscribers
            .clone()
            .try_acquire_owned()
            .map_err(|_| Status::resource_exhausted("subscriber limit"))?;
        let spool = t
            .critical
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| Status::unavailable("critical spool unavailable"))?;
        let (sender, receiver) = mpsc::channel(16);
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut epoch = request.proxy_epoch_id;
            let mut after = request.after_sequence;
            loop {
                let batches = match spool.replay(&epoch, after, 16).await {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = sender.send(Err(Status::unavailable(e.to_string()))).await;
                        break;
                    }
                };
                for batch in batches {
                    epoch = batch.proxy_epoch_id.clone();
                    after = batch.first_sequence + batch.events.len() as u64 - 1;
                    tokio::select! {_=shutdown.cancelled()=>return,result=sender.send(Ok(batch))=>if result.is_err(){return}}
                }
                tokio::select! {_=shutdown.cancelled()=>break,_=sender.closed()=>break,_=tokio::time::sleep(std::time::Duration::from_millis(100))=>{}}
            }
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }
    async fn ack_critical(
        &self,
        request: Request<v1::AckCriticalRequest>,
    ) -> Result<Response<v1::AckCriticalReply>, Status> {
        let request = request.into_inner();
        let spool = self
            .state
            .telemetry
            .critical
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| Status::unavailable("critical spool unavailable"))?;
        let acknowledged_through = spool
            .ack(&request.proxy_epoch_id, request.through_sequence)
            .await
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        Ok(Response::new(v1::AckCriticalReply {
            acknowledged_through,
        }))
    }
}
