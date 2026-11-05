use super::{Store, namespace};
use anyhow::{Context, ensure};
use hydra_domain::{ForwardFraming, Service};
use hydra_rewrite::{Format, Framer};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    net::TcpStream,
};
use tokio_util::sync::CancellationToken;
/// Dropping the adapter cancels both directions, including an incomplete frame.
pub struct Connection {
    stream: DuplexStream,
    stop: CancellationToken,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl AsyncRead for Connection {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for Connection {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
pub enum Upstream {
    Direct(TcpStream),
    Forwarded(Connection),
}
impl AsyncRead for Upstream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Direct(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Self::Forwarded(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}
impl AsyncWrite for Upstream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match &mut *self {
            Self::Direct(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Self::Forwarded(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Direct(s) => std::pin::Pin::new(s).poll_flush(cx),
            Self::Forwarded(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Direct(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Self::Forwarded(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}
pub async fn connect(service: &Service, store: Option<Arc<Store>>) -> std::io::Result<Upstream> {
    let stream = TcpStream::connect(service.effective_upstream()).await?;
    let Some(policy) = &service.forwarding else {
        return Ok(Upstream::Direct(stream));
    };
    let store = store.ok_or_else(|| std::io::Error::other("forwarding store unavailable"))?;
    let format = store.format(service).map_err(std::io::Error::other)?;
    let framing = policy.framing.clone();
    let ns = namespace(service);
    let (front, back) = tokio::io::duplex(16384);
    let stop = CancellationToken::new();
    let cancel = stop.clone();
    let id = service.id.clone();
    tokio::spawn(async move {
        let (ar, aw) = tokio::io::split(back);
        let (br, bw) = stream.into_split();
        let methods = Arc::new(Mutex::new(VecDeque::new()));
        let context = Pump {
            store,
            format,
            namespace: ns,
            methods,
            framing,
        };
        tokio::select! {_=cancel.cancelled()=>{}, result=async{tokio::try_join!(context.run(ar,bw,false),context.run(br,aw,true))}=>if result.is_err(){context.store.errors.fetch_add(1,std::sync::atomic::Ordering::Relaxed);tracing::warn!(service_id=%id,"forwarding failed: invalid framing, mapping capacity or storage unavailable");}}
    });
    Ok(Upstream::Forwarded(Connection {
        stream: front,
        stop,
    }))
}
struct Pump {
    store: Arc<Store>,
    format: Arc<Format>,
    namespace: String,
    methods: Arc<Mutex<VecDeque<String>>>,
    framing: ForwardFraming,
}
impl Pump {
    async fn run(
        &self,
        mut read: impl AsyncRead + Unpin,
        mut write: impl AsyncWrite + Unpin,
        reverse: bool,
    ) -> anyhow::Result<()> {
        let mut framer = Framer::new(self.framing.clone(), reverse)?;
        let mut bytes = [0; 8192];
        let mut since = None;
        loop {
            let n = if let Some(start) = since {
                tokio::time::timeout_at(start + Duration::from_secs(5), read.read(&mut bytes))
                    .await
                    .context("forwarding incomplete frame deadline")??
            } else {
                read.read(&mut bytes).await?
            };
            let received_at = tokio::time::Instant::now();
            let mut completed = false;
            let mut incoming = &bytes[..n];
            loop {
                let method = self
                    .methods
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .front()
                    .cloned();
                let frames = framer.feed(incoming, n == 0, method.as_deref())?;
                incoming = &[];
                if frames.is_empty() {
                    break;
                }
                completed = true;
                for mut frame in frames {
                    if !reverse && matches!(self.framing, ForwardFraming::Http1) {
                        let mut methods = self.methods.lock().unwrap_or_else(|e| e.into_inner());
                        ensure!(methods.len() < 128, "HTTP pipeline limit");
                        methods.push_back(
                            frame
                                .method
                                .clone()
                                .context("missing HTTP request method")?,
                        );
                    }
                    if reverse && matches!(self.framing, ForwardFraming::Http1) {
                        ensure!(method.is_some(), "unsolicited HTTP response");
                        if !frame.informational {
                            self.methods
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .pop_front();
                        }
                    }
                    self.store
                        .rewrite(&self.namespace, &self.format, &mut frame, reverse)
                        .await?;
                    write.write_all(&frame.wire).await?;
                }
                if !framer.pending() {
                    break;
                }
            }
            if n == 0 {
                write.shutdown().await?;
                return Ok(());
            }
            if framer.pending() {
                if completed || since.is_none() {
                    since = Some(received_at);
                }
            } else {
                since = None;
            }
        }
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn new_partial_frame_gets_a_new_deadline() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("hydra-framing-deadline-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(dir.join("journal")).await.unwrap();
        let context = Pump {
            store,
            format: Arc::new(Format::compile("FLAG[A-Z0-9]{8}").unwrap()),
            namespace: "ns".into(),
            methods: Arc::new(Mutex::new(VecDeque::new())),
            framing: ForwardFraming::Delimited {
                delimiter: "\n".into(),
            },
        };
        let (mut input, reader) = tokio::io::duplex(1024);
        let (writer, mut output) = tokio::io::duplex(1024);
        let task = tokio::spawn(async move { context.run(reader, writer, false).await });
        input.write_all(b"fir").await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(4)).await;
        input.write_all(b"st\nsec").await.unwrap();
        let mut first = [0; 6];
        output.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"first\n");
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::task::yield_now().await;
        input.write_all(b"ond\n").await.unwrap();
        input.shutdown().await.unwrap();
        let mut second = Vec::new();
        output.read_to_end(&mut second).await.unwrap();
        assert_eq!(second, b"second\n");
        task.await.unwrap().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
