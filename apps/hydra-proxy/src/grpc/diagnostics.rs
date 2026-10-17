//! Bounded, explicit diagnostics against configured service upstreams only.
use super::{Rpc, control::check_provenance};
use hydra_domain::Transport;
use hydra_wire::v1::{self, proxy_diagnostics_server::ProxyDiagnostics};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
};
use tonic::{Request, Response, Status};

fn history_status(error: crate::history::HistoryReadError) -> Status {
    match error {
        crate::history::HistoryReadError::Active => {
            Status::failed_precondition("flow history is not complete")
        }
        crate::history::HistoryReadError::Storage(_) => {
            Status::internal("flow history is unavailable")
        }
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn history_store_errors_map_to_bounded_rpc_statuses() {
        assert_eq!(
            history_status(crate::history::HistoryReadError::Active).code(),
            tonic::Code::FailedPrecondition
        );
        assert_eq!(
            history_status(crate::history::HistoryReadError::Storage(anyhow::anyhow!(
                "corrupt"
            )))
            .code(),
            tonic::Code::Internal
        );
    }
}

#[tonic::async_trait]
impl ProxyDiagnostics for Rpc {
    async fn test_protocol(
        &self,
        request: Request<v1::TestProtocolRequest>,
    ) -> Result<Response<v1::TestProtocolReply>, Status> {
        let r = request.into_inner();
        check_provenance(r.actor.as_ref(), &r.request_id)?;
        if r.protocol_id.len() > 64 || r.version.len() > 64 || r.sample_id.len() > 128 {
            return Err(Status::invalid_argument("invalid test identifiers"));
        }
        let _permit = self
            .diagnostics
            .clone()
            .try_acquire_owned()
            .map_err(|_| Status::resource_exhausted("diagnostics busy"))?;
        let direction = match r.direction {
            1 => hydra_protocol_sdk::Direction::ClientToServer,
            2 => hydra_protocol_sdk::Direction::ServerToClient,
            _ => return Err(Status::invalid_argument("invalid direction")),
        };
        let ring = self
            .state
            .telemetry
            .samples
            .read()
            .map_err(|_| Status::internal("samples unavailable"))?
            .clone()
            .ok_or_else(|| Status::unavailable("samples disabled"))?;
        let sample = ring
            .get(&r.sample_id)
            .await
            .map_err(|_| Status::internal("sample read failed"))?
            .ok_or_else(|| Status::not_found("sample expired or unavailable"))?;
        let messages = self
            .protocols
            .test(&r.protocol_id, &r.version, direction, &sample.data)
            .map_err(|_| Status::invalid_argument("decoder test failed"))?
            .into_iter()
            .map(|m| v1::ParsedMessage {
                message_id: uuid::Uuid::new_v4().to_string(),
                protocol_id: m.protocol,
                message_type: m.message_type,
                direction: r.direction,
                fields: Some(v1::FieldObject {
                    fields: m
                        .fields
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                hydra_wire::rules::value_to_wire(&crate::inspection::domain_value(
                                    v,
                                )),
                            )
                        })
                        .collect(),
                }),
                original_size: 0,
                ..Default::default()
            })
            .collect();
        Ok(Response::new(v1::TestProtocolReply { messages }))
    }
    async fn capture_sample(
        &self,
        request: Request<v1::CaptureSampleRequest>,
    ) -> Result<Response<v1::CaptureSampleReply>, Status> {
        let r = request.into_inner();
        check_provenance(r.actor.as_ref(), &r.request_id)?;
        if r.flow_id.is_empty() || r.flow_id.len() > 128 {
            return Err(Status::invalid_argument("invalid flow identifier"));
        }
        let t = &self.state.telemetry;
        if !t
            .flows
            .lock()
            .map_err(|_| Status::internal("flow registry unavailable"))?
            .contains(&r.flow_id)
        {
            return Err(Status::not_found("active flow not found"));
        }
        let ring = t
            .samples
            .read()
            .map_err(|_| Status::internal("sample storage unavailable"))?
            .clone()
            .ok_or_else(|| Status::failed_precondition("raw samples disabled"))?;
        let sample_id = ring
            .request_capture(r.flow_id)
            .map_err(|_| Status::resource_exhausted("capture admission unavailable"))?;
        Ok(Response::new(v1::CaptureSampleReply { sample_id }))
    }
    async fn get_sample(
        &self,
        request: Request<v1::GetSampleRequest>,
    ) -> Result<Response<v1::GetSampleReply>, Status> {
        let id = request.into_inner().sample_id;
        if id.is_empty() || id.len() > 128 {
            return Err(Status::invalid_argument("invalid sample identifier"));
        }
        let ring = self
            .state
            .telemetry
            .samples
            .read()
            .map_err(|_| Status::internal("sample storage unavailable"))?
            .clone()
            .ok_or_else(|| Status::failed_precondition("raw samples disabled"))?;
        let sample = ring
            .get(&id)
            .await
            .map_err(|_| Status::internal("sample read failed"))?
            .ok_or_else(|| Status::not_found("sample unavailable or expired"))?;
        Ok(Response::new(v1::GetSampleReply {
            data: sample.data,
            truncated: sample.truncated,
            expires_at: Some(prost_types::Timestamp {
                seconds: sample.expires_at,
                nanos: 0,
            }),
        }))
    }

    async fn get_flow_history(
        &self,
        request: Request<v1::GetFlowHistoryRequest>,
    ) -> Result<Response<v1::GetFlowHistoryReply>, Status> {
        let flow_id = request.into_inner().flow_id;
        if flow_id.len() > 128 || uuid::Uuid::parse_str(&flow_id).is_err() {
            return Err(Status::invalid_argument("invalid flow identifier"));
        }
        let _permit = self
            .diagnostics
            .clone()
            .try_acquire_owned()
            .map_err(|_| Status::resource_exhausted("diagnostics busy"))?;
        if self
            .state
            .telemetry
            .flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&flow_id)
        {
            return Err(Status::failed_precondition("flow is still active"));
        }
        let store = self
            .state
            .telemetry
            .history
            .read()
            .map_err(|_| Status::internal("history store unavailable"))?
            .clone()
            .ok_or_else(|| Status::unavailable("history store disabled"))?;
        let history = store
            .get(&flow_id)
            .await
            .map_err(history_status)?
            .ok_or_else(|| Status::not_found("flow history is unavailable or expired"))?;
        let total = history.client_to_server.len() + history.server_to_client.len();
        if total > crate::history::MAX_HISTORY_BYTES {
            return Err(Status::resource_exhausted("history exceeds response limit"));
        }
        Ok(Response::new(v1::GetFlowHistoryReply {
            flow_id: history.flow_id,
            service_id: history.service_id,
            protocol_id: history.protocol_id,
            opened_at_ms: history.opened_at_ms,
            closed_at_ms: history.closed_at_ms,
            bytes_client_to_server: history.bytes_client_to_server,
            bytes_server_to_client: history.bytes_server_to_client,
            segments: vec![
                v1::HistorySegment {
                    direction: 1,
                    data: history.client_to_server,
                },
                v1::HistorySegment {
                    direction: 2,
                    data: history.server_to_client,
                },
            ],
            truncated_client_to_server: history.truncated_client_to_server,
            truncated_server_to_client: history.truncated_server_to_client,
            history_truncated: history.history_truncated,
        }))
    }
    async fn probe_service(
        &self,
        request: Request<v1::ProbeServiceRequest>,
    ) -> Result<Response<v1::ProbeServiceReply>, Status> {
        let r = request.into_inner();
        check_provenance(r.actor.as_ref(), &r.request_id)?;
        if r.service_id.is_empty()
            || r.service_id.len() > 128
            || r.payload.len() > 4096
            || !(50..=5000).contains(&r.timeout_ms)
        {
            return Err(Status::invalid_argument("probe bounds exceeded"));
        }
        let _permit = self
            .diagnostics
            .clone()
            .try_acquire_owned()
            .map_err(|_| Status::resource_exhausted("diagnostic probe busy"))?;
        let service = self
            .state
            .get_service(&r.service_id)
            .await
            .ok_or_else(|| Status::not_found("configured service not found"))?;
        let transport = match r.transport {
            1 => Transport::Tcp,
            2 => Transport::Udp,
            _ => return Err(Status::invalid_argument("invalid probe transport")),
        };
        if !service.enabled || transport != service.transport {
            return Err(Status::failed_precondition(
                "service disabled or transport mismatch",
            ));
        }
        let started = Instant::now();
        let operation = probe(service.upstream_address, transport, &r.payload);
        let (response, truncated, error_code) = tokio::select! {
            _=self.shutdown.cancelled()=>(Vec::new(),false,"shutdown".into()),
            result=tokio::time::timeout(Duration::from_millis(r.timeout_ms.into()),operation)=>match result {Ok(Ok((bytes,truncated)))=>(bytes,truncated,String::new()),Ok(Err(_))=>(Vec::new(),false,"network_error".into()),Err(_)=>(Vec::new(),false,"timeout".into())}
        };
        Ok(Response::new(v1::ProbeServiceReply {
            response,
            truncated,
            elapsed_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
            error_code,
        }))
    }
}
async fn probe(
    address: SocketAddr,
    transport: Transport,
    payload: &[u8],
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut response = vec![0; 4097];
    let n = match transport {
        Transport::Tcp => {
            let mut stream = TcpStream::connect(address).await?;
            stream.write_all(payload).await?;
            stream.shutdown().await?;
            stream.read(&mut response).await?
        }
        Transport::Udp => {
            let socket = UdpSocket::bind(if address.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            })
            .await?;
            socket.connect(address).await?;
            socket.send(payload).await?;
            socket.recv(&mut response).await?
        }
    };
    response.truncate(n.min(4096));
    Ok((response, n > 4096))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn tcp_probe_preserves_bytes_and_bounds_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            s.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes, b"\0probe\xff");
            s.write_all(&vec![7; 8192]).await.unwrap();
        });
        let (bytes, truncated) = probe(address, Transport::Tcp, b"\0probe\xff")
            .await
            .unwrap();
        assert_eq!(bytes, vec![7; 4096]);
        assert!(truncated);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn udp_probe_preserves_bytes() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut bytes = [0; 64];
            let (n, from) = socket.recv_from(&mut bytes).await.unwrap();
            socket.send_to(&bytes[..n], from).await.unwrap();
        });
        let (bytes, truncated) = probe(address, Transport::Udp, b"\0udp\xff").await.unwrap();
        assert_eq!(bytes, b"\0udp\xff");
        assert!(!truncated);
        server.await.unwrap();
    }
}
