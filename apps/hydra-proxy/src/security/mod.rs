use anyhow::{bail, ensure};
use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{UnixListener, UnixStream},
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tokio_stream::{Stream, StreamExt, wrappers::UnixListenerStream};
use tonic::transport::server::{Connected, UdsConnectInfo};

pub async fn bind(path: &Path, mode: u32) -> anyhow::Result<UnixListener> {
    ensure!(matches!(mode, 0o600 | 0o660), "unsafe socket mode");
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        ensure!(
            meta.file_type().is_socket() && meta.uid() == nix::unistd::geteuid().as_raw(),
            "refusing to replace non-socket or foreign-owned socket"
        );
        match UnixStream::connect(path).await {
            Ok(_) => bail!("proxy socket is already in use"),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                tokio::fs::remove_file(path).await?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    // Parent directory must be provisioned by the operator; never remove arbitrary paths.
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(listener)
}
pub struct LocalConnection {
    stream: UnixStream,
    _permit: OwnedSemaphorePermit,
}
impl Connected for LocalConnection {
    type ConnectInfo = UdsConnectInfo;
    fn connect_info(&self) -> Self::ConnectInfo {
        self.stream.connect_info()
    }
}
impl AsyncRead for LocalConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for LocalConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
pub fn incoming(
    listener: UnixListener,
    uid: u32,
) -> impl Stream<Item = io::Result<LocalConnection>> {
    let connections = Arc::new(Semaphore::new(32));
    UnixListenerStream::new(listener).filter_map(move |result| match result {
        Ok(stream) => {
            if stream.peer_cred().ok().map(|cred| cred.uid()) != Some(uid) {
                return None;
            }
            let permit = connections.clone().try_acquire_owned().ok()?;
            Some(Ok(LocalConnection {
                stream,
                _permit: permit,
            }))
        }
        Err(e) => Some(Err(e)),
    })
}
