//! The stream a terminal-initiated ZMODEM transfer borrows from a session.

use super::*;

pub(super) enum Control {
    Write(Vec<u8>),
    Resize(PtySize),
    /// Route raw session bytes through a terminal-initiated ZMODEM transfer.
    ZmodemBridge {
        to_transfer: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        from_transfer: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    },
}

pub(super) type ZmodemBridge = (
    tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
);

/// A ZMODEM endpoint wired into a live session channel: data arriving from the
/// remote host is read here, and frames written here are sent back to it.
pub struct BridgeStream {
    pub(super) rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    pub(super) tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    pub(super) buffer: Vec<u8>,
    pub(super) position: usize,
}

impl tokio::io::AsyncRead for BridgeStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        loop {
            if self.position < self.buffer.len() {
                let n = (self.buffer.len() - self.position).min(buf.remaining());
                buf.put_slice(&self.buffer[self.position..self.position + n]);
                self.position += n;
                if self.position == self.buffer.len() {
                    self.buffer.clear();
                    self.position = 0;
                }
                return std::task::Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                std::task::Poll::Ready(Some(chunk)) => {
                    self.buffer = chunk;
                    self.position = 0;
                }
                std::task::Poll::Ready(None) => return std::task::Poll::Ready(Ok(())),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }
}

impl tokio::io::AsyncWrite for BridgeStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self.tx.send(buf.to_vec()).is_err() {
            return std::task::Poll::Ready(Err(std::io::Error::from(
                std::io::ErrorKind::BrokenPipe,
            )));
        }
        std::task::Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}
