//! Bound management connections for their entire lifetime, including TLS setup.
use axum_server::accept::Accept;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{Instant, Sleep},
};
#[derive(Clone)]
pub struct Limited<A> {
    inner: A,
    permits: Arc<Semaphore>,
    idle: Duration,
}
impl<A> Limited<A> {
    pub fn new(inner: A) -> Self {
        Self {
            inner,
            permits: Arc::new(Semaphore::new(64)),
            idle: Duration::from_secs(60),
        }
    }
}
pub struct Connection<I> {
    inner: I,
    _permit: OwnedSemaphorePermit,
    timer: Pin<Box<Sleep>>,
    idle: Duration,
}
type Accepted<I, S> = Pin<Box<dyn Future<Output = io::Result<(Connection<I>, S)>> + Send>>;
impl<I, S, A> Accept<I, S> for Limited<A>
where
    I: Send + 'static,
    S: Send + 'static,
    A: Accept<I, S> + Clone + Send + 'static,
    A::Stream: Send + 'static,
    A::Service: Send + 'static,
    A::Future: Send,
{
    type Stream = Connection<A::Stream>;
    type Service = A::Service;
    type Future = Accepted<A::Stream, A::Service>;
    fn accept(&self, stream: I, service: S) -> Self::Future {
        let permit = self.permits.clone().try_acquire_owned();
        let inner = self.inner.clone();
        let idle = self.idle;
        Box::pin(async move {
            let permit =
                permit.map_err(|_| io::Error::other("management connection capacity exhausted"))?;
            let (stream, service) =
                tokio::time::timeout(Duration::from_secs(5), inner.accept(stream, service))
                    .await??;
            Ok((
                Connection {
                    inner: stream,
                    _permit: permit,
                    timer: Box::pin(tokio::time::sleep(idle)),
                    idle,
                },
                service,
            ))
        })
    }
}
impl<I: AsyncRead + Unpin> AsyncRead for Connection<I> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if matches!(&result, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            this.timer.as_mut().reset(Instant::now() + this.idle)
        }
        result
    }
}
impl<I: AsyncWrite + Unpin> AsyncWrite for Connection<I> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        let result = Pin::new(&mut this.inner).poll_write(cx, buf);
        if matches!(&result,Poll::Ready(Ok(n))if *n>0) {
            this.timer.as_mut().reset(Instant::now() + this.idle)
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum_server::accept::DefaultAcceptor;
    use tokio::io::AsyncReadExt;
    #[tokio::test]
    async fn connection_permit_is_held_until_stream_drop() {
        let acceptor = Limited {
            inner: DefaultAcceptor::new(),
            permits: Arc::new(Semaphore::new(1)),
            idle: Duration::from_secs(60),
        };
        let (a, _peer) = tokio::io::duplex(16);
        let (first, _) = acceptor.accept(a, ()).await.unwrap();
        let (b, _peer2) = tokio::io::duplex(16);
        assert!(acceptor.accept(b, ()).await.is_err());
        drop(first);
        let (c, _peer3) = tokio::io::duplex(16);
        assert!(acceptor.accept(c, ()).await.is_ok());
    }
    #[tokio::test]
    async fn idle_management_connection_expires() {
        let acceptor = Limited {
            inner: DefaultAcceptor::new(),
            permits: Arc::new(Semaphore::new(1)),
            idle: Duration::from_millis(10),
        };
        let (a, _peer) = tokio::io::duplex(16);
        let (mut stream, _) = acceptor.accept(a, ()).await.unwrap();
        assert_eq!(
            stream.read(&mut [0u8]).await.unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
