//! Bound TCP/TLS connections before handshake work, including idle HTTP sockets.
use axum_server::accept::Accept;
use std::{
    future::{Ready, ready},
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    sync::{OwnedSemaphorePermit, Semaphore},
};
#[derive(Clone)]
pub struct ConnectionLimit(Arc<Semaphore>);
impl ConnectionLimit {
    pub fn new() -> Self {
        Self(Arc::new(Semaphore::new(96)))
    }
}
pub struct LimitedStream {
    inner: TcpStream,
    _permit: OwnedSemaphorePermit,
}
impl<S> Accept<TcpStream, S> for ConnectionLimit {
    type Stream = LimitedStream;
    type Service = S;
    type Future = Ready<io::Result<(LimitedStream, S)>>;
    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        ready(
            self.0
                .clone()
                .try_acquire_owned()
                .map_err(|_| io::Error::other("connection limit"))
                .and_then(|permit| {
                    stream.set_nodelay(true)?;
                    Ok((
                        LimitedStream {
                            inner: stream,
                            _permit: permit,
                        },
                        service,
                    ))
                }),
        )
    }
}
impl AsyncRead for LimitedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
impl AsyncWrite for LimitedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
