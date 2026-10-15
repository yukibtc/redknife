use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use std::{io, mem};

use bytes::Bytes;
use http::Request;
use tokio::io::AsyncWrite;
use tokio::task::JoinHandle;

use crate::http::client::send_http;
use crate::http::target::HttpTarget;

const RESPONSE_LIMIT: usize = 10 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
enum ResponseStage {
    Bytes(Vec<u8>),
    Taken,
}

impl Default for ResponseStage {
    #[inline]
    fn default() -> Self {
        Self::Bytes(Vec::new())
    }
}

#[derive(Clone, Default)]
struct ResponseOutput {
    stage: Arc<Mutex<ResponseStage>>,
}

impl ResponseOutput {
    /// Acquires the Mutex lock and takes the inner value without cloning.
    #[inline]
    fn take(&self) -> Option<Vec<u8>> {
        let mut stage = self.stage.lock().unwrap_or_else(|error| error.into_inner());

        match mem::replace(&mut *stage, ResponseStage::Taken) {
            ResponseStage::Bytes(bytes) => Some(bytes),
            ResponseStage::Taken => None,
        }
    }
}

impl AsyncWrite for ResponseOutput {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut stage = self.stage.lock().unwrap_or_else(|error| error.into_inner());

        let ResponseStage::Bytes(bytes) = &mut *stage else {
            return Poll::Ready(Err(io::Error::other("Response has been taken")));
        };

        let count: usize = input.len().min(RESPONSE_LIMIT.saturating_sub(bytes.len()));

        if count == 0 && !input.is_empty() {
            return Poll::Ready(Err(io::Error::other(
                "Response exceeds the editor limit of 10 MiB",
            )));
        }

        bytes.extend_from_slice(&input[..count]);

        Poll::Ready(Ok(count))
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Outcome of a send, including bytes received before an error or cancellation.
pub(super) struct SendResult {
    pub(super) bytes: Vec<u8>,
    pub(super) error: Option<String>,
}

/// Owns the network task; dropping it always stops any remaining network I/O.
pub(super) struct PendingSend {
    handle: JoinHandle<Option<String>>,
    output: ResponseOutput,
}

impl PendingSend {
    /// Start sending without blocking the terminal event loop.
    pub(super) fn start(request: Request<Bytes>, target: HttpTarget) -> Self {
        let output: ResponseOutput = ResponseOutput::default();

        let mut writer: ResponseOutput = output.clone();

        let handle: JoinHandle<Option<String>> = tokio::spawn(async move {
            send_http(request, &target, TIMEOUT, &mut writer)
                .await
                .err()
                .map(|error| error.to_string())
        });

        Self { handle, output }
    }

    /// Whether the task has ended and its result can be collected without waiting.
    #[inline]
    pub(super) fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Stop network I/O and collect the response bytes received so far.
    #[inline]
    pub(super) async fn cancel(self) -> SendResult {
        self.handle.abort();
        self.finish().await
    }

    /// Wait for the task to end and collect its complete or partial response.
    pub(super) async fn finish(mut self) -> SendResult {
        // Borrow the handle so Drop still owns cancellation if this wait is interrupted.
        let error: Option<String> = match (&mut self.handle).await {
            Ok(error) => error,
            Err(error) if error.is_cancelled() => Some("Send cancelled".into()),
            Err(error) => Some(error.to_string()),
        };

        SendResult {
            bytes: self.output.take().unwrap_or_default(),
            error,
        }
    }
}

impl Drop for PendingSend {
    #[inline]
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::future;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::time::timeout;

    use super::*;
    use crate::http::request::parse_request;

    #[tokio::test]
    async fn test_response_output_writing() {
        let mut output = ResponseOutput::default();

        {
            let stage = output.stage.lock().unwrap();
            assert_eq!(&*stage, &ResponseStage::Bytes(Vec::new()))
        }

        output.write_all(b"hello, world").await.unwrap();

        {
            let stage = output.stage.lock().unwrap();
            assert_eq!(&*stage, &ResponseStage::Bytes(b"hello, world".to_vec()))
        }

        let bytes = output.take();
        assert_eq!(bytes, Some(b"hello, world".to_vec()));

        {
            let stage = output.stage.lock().unwrap();
            assert_eq!(&*stage, &ResponseStage::Taken)
        }

        assert!(output.take().is_none());

        let error: io::Error = output.write_all(b"more").await.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(error.to_string(), "Response has been taken");
    }

    /// A task that writes a partial response, signals readiness, then waits for cancellation.
    async fn waiting_send() -> (PendingSend, oneshot::Receiver<()>) {
        let output: ResponseOutput = ResponseOutput::default();
        let mut writer: ResponseOutput = output.clone();
        let (ready, started) = oneshot::channel();
        let (stop_sender, stopped) = oneshot::channel();

        let handle: JoinHandle<Option<String>> = tokio::spawn(async move {
            // Closing this channel tells the test that the task has actually been dropped.
            let _stop_sender = stop_sender;
            writer.write_all(b"partial").await.unwrap();
            ready.send(()).unwrap();
            future::pending().await
        });
        let send: PendingSend = PendingSend { handle, output };

        timeout(Duration::from_secs(2), started)
            .await
            .unwrap()
            .unwrap();

        (send, stopped)
    }

    #[tokio::test]
    async fn test_send_returns_the_response() {
        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target: HttpTarget = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server: JoinHandle<()> = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader: BufReader<_> = BufReader::new(stream);
            let mut head: String = String::new();

            while !head.ends_with("\r\n\r\n") {
                assert_ne!(reader.read_line(&mut head).await.unwrap(), 0);
            }

            reader
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        });
        let request: Request<Bytes> = parse_request(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let send: PendingSend = PendingSend::start(request, target);

        let result: SendResult = timeout(Duration::from_secs(2), send.finish())
            .await
            .unwrap();

        assert!(result.error.is_none());
        assert_eq!(
            result.bytes,
            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok"
        );
        timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn test_cancel_keeps_partial_bytes() {
        let (send, stopped) = waiting_send().await;
        let result: SendResult = timeout(Duration::from_secs(2), send.cancel())
            .await
            .unwrap();

        assert_eq!(result.bytes, b"partial");
        assert!(result.error.is_some());
        assert!(stopped.await.is_err());
    }

    #[tokio::test]
    async fn test_drop_stops_the_task() {
        let (send, stopped) = waiting_send().await;

        drop(send);

        assert!(
            timeout(Duration::from_secs(2), stopped)
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_response_limit_keeps_received_bytes() {
        let mut output: ResponseOutput = ResponseOutput::default();
        let bytes: Vec<u8> = vec![b'x'; RESPONSE_LIMIT];
        output.write_all(&bytes).await.unwrap();

        let error: io::Error = output.write_all(b"overflow").await.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(output.take().unwrap(), bytes);
    }
}
