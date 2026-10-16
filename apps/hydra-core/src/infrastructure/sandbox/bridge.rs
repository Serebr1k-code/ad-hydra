use super::{Result, Runtime, SandboxError};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket, UnixListener, UnixStream},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Endpoint {
    pub id: String,
    pub address: SocketAddr,
    pub transport: String,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BridgeConfig {
    pub target: SocketAddr,
    pub endpoints: Vec<Endpoint>,
    pub entrypoint: String,
    pub runtime: Runtime,
}
/// Trusted helper, dispatched before Core loads secrets. Only fixed broker paths are exposed.
pub async fn bridge(config: BridgeConfig) -> Result<i32> {
    if config.endpoints.is_empty() || config.endpoints.len() > 256 {
        return Err(SandboxError::Invalid(
            "invalid bridge endpoint count".into(),
        ));
    }
    let status = tokio::process::Command::new("/hydra-ip")
        .args(["link", "set", "lo", "up"])
        .env_clear()
        .status()
        .await?;
    if !status.success() {
        return Err(SandboxError::Unavailable(
            "network namespace setup failed".into(),
        ));
    }
    let mut addresses = std::collections::HashSet::new();
    let mut listeners = JoinSet::new();
    for (index, endpoint) in config.endpoints.iter().enumerate() {
        super::validate_target(endpoint.address)?;
        if !["tcp", "udp"].contains(&endpoint.transport.as_str()) {
            return Err(SandboxError::Invalid("invalid bridge transport".into()));
        }
        if addresses.insert(endpoint.address.ip()) {
            let status = tokio::process::Command::new("/hydra-ip")
                .args([
                    "addr",
                    "add",
                    &format!("{}/32", endpoint.address.ip()),
                    "dev",
                    "lo",
                ])
                .env_clear()
                .status()
                .await?;
            if !status.success() {
                return Err(SandboxError::Unavailable(
                    "network namespace setup failed".into(),
                ));
            }
        }
        let path = format!("/broker/target-{index}.sock");
        if endpoint.transport == "tcp" {
            let listener = TcpListener::bind(endpoint.address).await?;
            listeners.spawn(async move { tcp_bridge(listener, path).await });
        } else {
            let socket = UdpSocket::bind(endpoint.address).await?;
            listeners.spawn(async move { udp_bridge(socket, path).await });
        }
    }
    let mut command = tokio::process::Command::from(super::command::inner_command(&config));
    let mut child = command
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    tokio::select! {
        status = child.wait() => { listeners.abort_all(); Ok(status?.code().unwrap_or(128)) },
        _ = listeners.join_next() => { let _ = child.kill().await; Err(SandboxError::Unavailable("sandbox bridge stopped".into())) }
    }
}
async fn tcp_bridge(listener: TcpListener, path: String) -> Result<()> {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept(), if connections.len() < 16 => {
                let (mut tcp, _) = accepted?;
                let path = path.clone();
                connections.spawn(async move {
                    if let Ok(mut unix) = UnixStream::connect(path).await {
                        let _ = tokio::time::timeout(Duration::from_secs(3600), tokio::io::copy_bidirectional(&mut tcp, &mut unix)).await;
                    }
                });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}
/// Each source address has one connected fixed-destination broker session. The bounded map
/// rejects new sources at capacity and expires idle sessions; packets preserve datagram framing.
async fn udp_bridge(socket: UdpSocket, path: String) -> Result<()> {
    let socket = std::sync::Arc::new(socket);
    let mut sessions =
        std::collections::HashMap::<SocketAddr, tokio::sync::mpsc::Sender<Vec<u8>>>::new();
    let mut workers = JoinSet::new();
    let mut bytes = vec![0u8; 65507];
    loop {
        tokio::select! {
            packet = socket.recv_from(&mut bytes) => {
                let (length, source) = packet?;
                sessions.retain(|_, sender| !sender.is_closed());
                if let Some(sender) = sessions.get(&source) {
                    if sender.try_send(bytes[..length].to_vec()).is_err() { eprintln!("[sandbox UDP datagram dropped: queue full]"); }
                } else if sessions.len() < 16 {
                    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
                    let _ = sender.try_send(bytes[..length].to_vec());
                    sessions.insert(source, sender);
                    let socket = socket.clone(); let path = path.clone();
                    workers.spawn(async move {
                        let Ok(unix) = UnixStream::connect(path).await else { return; };
                        let (mut read, mut write) = unix.into_split();
                        let receive = async {
                            loop {
                                let packet = read_frame(&mut read).await?;
                                socket.send_to(&packet, source).await?;
                            }
                            #[allow(unreachable_code)] Ok::<(),std::io::Error>(())
                        };
                        let send = async {
                            while let Ok(Some(packet)) = tokio::time::timeout(Duration::from_secs(30), receiver.recv()).await {
                                write_frame(&mut write, &packet).await?;
                            }
                            Ok::<(),std::io::Error>(())
                        };
                        tokio::select! { _ = receive => {}, _ = send => {} }
                    });
                } else { eprintln!("[sandbox UDP datagram dropped: session capacity]"); }
            }
            _ = workers.join_next(), if !workers.is_empty() => {}
        }
    }
}
async fn write_frame(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    packet: &[u8],
) -> std::io::Result<()> {
    writer.write_u16(packet.len() as u16).await?;
    writer.write_all(packet).await
}
async fn read_frame(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> std::io::Result<Vec<u8>> {
    let length = reader.read_u16().await? as usize;
    if length > 65507 {
        return Err(std::io::Error::other("oversized UDP frame"));
    }
    let mut packet = vec![0u8; length];
    reader.read_exact(&mut packet).await?;
    Ok(packet)
}
/// Parent gateway never accepts destination bytes from workers.
pub(super) async fn gateway(
    listener: UnixListener,
    endpoint: Endpoint,
    cancel: CancellationToken,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            accepted = listener.accept(), if connections.len() < 16 => {
                let Ok((mut unix, _)) = accepted else { break; };
                let Ok(permit) = slots.clone().try_acquire_owned() else { tracing::warn!("sandbox connection rejected: global gateway capacity"); continue; };
                let endpoint = endpoint.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    if endpoint.transport == "tcp" {
                        if let Ok(Ok(mut tcp)) = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(endpoint.address)).await {
                            let _ = tokio::time::timeout(Duration::from_secs(3600), tokio::io::copy_bidirectional(&mut unix, &mut tcp)).await;
                        }
                    } else if let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await {
                        if socket.connect(endpoint.address).await.is_err() { return; }
                        let (mut read, mut write) = unix.into_split();
                        let send = async {
                            loop { let packet = read_frame(&mut read).await?; socket.send(&packet).await?; }
                            #[allow(unreachable_code)] Ok::<(),std::io::Error>(())
                        };
                        let receive = async {
                            let mut bytes = vec![0u8; 65507];
                            loop { let length = socket.recv(&mut bytes).await?; write_frame(&mut write, &bytes[..length]).await?; }
                            #[allow(unreachable_code)] Ok::<(),std::io::Error>(())
                        };
                        tokio::select! { _ = send => {}, _ = receive => {} }
                    }
                });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn datagram_frames_preserve_empty_and_binary_packets() {
        let mut bytes = Vec::new();
        for packet in [&[][..], &[0, 255, 0, 13, 10][..], &[7; 65507][..]] {
            write_frame(&mut bytes, packet).await.unwrap();
        }
        let mut reader = bytes.as_slice();
        assert_eq!(read_frame(&mut reader).await.unwrap(), Vec::<u8>::new());
        assert_eq!(read_frame(&mut reader).await.unwrap(), [0, 255, 0, 13, 10]);
        assert_eq!(read_frame(&mut reader).await.unwrap(), vec![7; 65507]);
        assert!(reader.is_empty());
    }
}
