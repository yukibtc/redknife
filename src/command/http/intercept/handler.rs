use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use http_body_util::{BodyExt, Full, Limited};
use tokio::sync::mpsc::Permit;
use tokio::sync::{mpsc, oneshot};

use super::messages::{InterceptReply, PendingRequest};
use crate::http::request::normalize_body_headers;
use crate::proxy::http_forward_server::{HttpForwardProxyServerRequestHandler, ProxyBody};

const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;

pub struct InterceptHandler {
    sender: mpsc::Sender<PendingRequest>,
    ids: AtomicU64,
}

impl InterceptHandler {
    pub fn new(sender: mpsc::Sender<PendingRequest>) -> Self {
        Self {
            sender,
            ids: AtomicU64::new(1),
        }
    }
}

impl HttpForwardProxyServerRequestHandler for InterceptHandler {
    fn handle<'a>(
        &'a self,
        original_uri: Uri,
        req: Request<ProxyBody>,
    ) -> Pin<Box<dyn Future<Output = Option<Request<ProxyBody>>> + Send + 'a>> {
        Box::pin(async move {
            // Reserve capacity before reading a potentially large or slow body.
            let permit: Permit<PendingRequest> = self.sender.try_reserve().ok()?;

            let timestamp: DateTime<Local> = Local::now();

            let (parts, body) = req.into_parts();

            let collected: Bytes = tokio::select! {
                result = Limited::new(body, MAX_BODY_SIZE).collect() => result.ok()?.to_bytes(),
                _ = self.sender.closed() => return None,
            };

            let mut buffered: Request<Bytes> = Request::from_parts(parts, collected);

            normalize_body_headers(&mut buffered);

            let (reply, receiver) = oneshot::channel();

            let id: u64 = self.ids.fetch_add(1, Ordering::Relaxed);

            let pending = PendingRequest {
                id,
                timestamp,
                original_uri,
                request: buffered,
                reply,
            };

            permit.send(pending);

            // If the UI is closed or the oneshot is canceled, fail closed.
            match receiver.await.ok()? {
                InterceptReply::Forward(mut edited) => {
                    normalize_body_headers(&mut edited);
                    Some(edited.map(|body| Full::new(body).map_err(|never| match never {}).boxed()))
                }
                InterceptReply::Drop => None,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use bytes::Bytes;
    use http::{Request, Response, Uri};
    use http_body_util::{BodyExt, Full};
    use hyper::body::{Body, Frame, Incoming};
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{mpsc, oneshot};
    use tokio::task::{AbortHandle, JoinHandle};
    use tokio::time::timeout;

    use super::{InterceptHandler, MAX_BODY_SIZE};
    use crate::command::http::intercept::messages::{InterceptReply, PendingRequest};
    use crate::proxy::http_forward_server::{
        HttpForwardProxyServer, HttpForwardProxyServerRequestHandler, ProxyBody,
    };

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn buffered_request(body: Bytes) -> Request<ProxyBody> {
        Request::builder()
            .method("POST")
            .uri("/original")
            .header("content-length", body.len())
            .body(Full::new(body).map_err(|never| match never {}).boxed())
            .expect("test request must be valid")
    }

    fn original_uri() -> Uri {
        "http://example.com/original".parse().unwrap()
    }

    // Signals its first read, then stays pending so the test controls cancellation.
    struct PendingBody {
        notify_when_polled: Option<oneshot::Sender<()>>,
    }

    impl Body for PendingBody {
        type Data = Bytes;
        type Error = hyper::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
            if let Some(notification) = self.notify_when_polled.take() {
                let _ = notification.send(());
            }
            Poll::Pending
        }
    }

    // One proxy, one destination, and one client request per test.
    struct TestProxy {
        original_url: String,
        address: SocketAddr,
        pending_requests: mpsc::Receiver<PendingRequest>,
        received_requests: mpsc::Receiver<Request<Bytes>>,
        server_tasks: Vec<AbortHandle>,
        client_task: Option<JoinHandle<String>>,
    }

    impl TestProxy {
        async fn start() -> Self {
            let origin = TcpListener::bind("127.0.0.1:0").await.expect("bind origin");

            let original_url = format!("http://{}/inspect?value=1", origin.local_addr().unwrap());

            let (received_sender, received_requests) = mpsc::channel(1);

            let origin_task = tokio::spawn(async move {
                let (stream, _) = origin.accept().await.expect("accept origin connection");

                let service = service_fn(move |request: Request<Incoming>| {
                    let sender = received_sender.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        let body = body.collect().await?.to_bytes();
                        let _ = sender.send(Request::from_parts(parts, body)).await;
                        Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from_static(b"ok"))))
                    }
                });

                hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await
                    .expect("serve origin connection");
            });

            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
            let address = listener.local_addr().expect("proxy address");
            let (pending_sender, pending_requests) = mpsc::channel(1);
            let handler = Arc::new(InterceptHandler::new(pending_sender));
            let proxy_task = tokio::spawn(HttpForwardProxyServer::new(listener, handler).run());

            Self {
                original_url,
                address,
                pending_requests,
                received_requests,
                server_tasks: vec![origin_task.abort_handle(), proxy_task.abort_handle()],
                client_task: None,
            }
        }

        async fn intercept(&mut self, wire_request: String) -> PendingRequest {
            let address = self.address;
            self.client_task = Some(tokio::spawn(async move {
                timeout(TEST_TIMEOUT, async {
                    let mut stream = TcpStream::connect(address).await.expect("connect to proxy");
                    stream
                        .write_all(wire_request.as_bytes())
                        .await
                        .expect("send request");
                    let mut response = String::new();
                    stream
                        .read_to_string(&mut response)
                        .await
                        .expect("read response");
                    response
                })
                .await
                .expect("client request timed out")
            }));

            timeout(TEST_TIMEOUT, self.pending_requests.recv())
                .await
                .expect("request was not queued")
                .expect("interceptor queue closed")
        }

        async fn forward(
            &mut self,
            reply: oneshot::Sender<InterceptReply>,
            edited_request: Request<Bytes>,
        ) -> Request<Bytes> {
            reply
                .send(InterceptReply::Forward(Box::new(edited_request)))
                .unwrap_or_else(|_| panic!("handler stopped before Forward"));
            timeout(TEST_TIMEOUT, self.received_requests.recv())
                .await
                .expect("request did not reach the original destination")
                .expect("origin stopped before receiving the request")
        }

        async fn response(&mut self) -> String {
            self.client_task
                .take()
                .expect("client request must be started")
                .await
                .expect("client task failed")
        }
    }

    impl Drop for TestProxy {
        fn drop(&mut self) {
            for task in &self.server_tasks {
                task.abort();
            }
            if let Some(task) = &self.client_task {
                task.abort();
            }
        }
    }

    fn post_request(url: &str) -> String {
        format!(
            concat!(
                "POST {} HTTP/1.1\r\n",
                "Host: example.com\r\n",
                "Connection: close\r\n",
                "Content-Length: 4\r\n",
                "\r\n",
                "ping",
            ),
            url
        )
    }

    #[tokio::test]
    async fn queues_original_url_separately_from_origin_form_request() {
        let mut proxy = TestProxy::start().await;
        let pending = proxy.intercept(post_request(&proxy.original_url)).await;

        assert_eq!(
            pending.original_uri,
            proxy.original_url.parse::<Uri>().unwrap()
        );
        assert_eq!(pending.request.uri(), "/inspect?value=1");
        assert_eq!(pending.request.method(), "POST");
        assert_eq!(pending.request.headers()["host"], "example.com");
        assert_eq!(pending.request.body(), b"ping".as_slice());

        let received = proxy.forward(pending.reply, pending.request).await;
        let response = proxy.response().await;

        assert_eq!(received.uri(), "/inspect?value=1");
        assert_eq!(received.body(), b"ping".as_slice());
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("\r\n\r\nok"));
    }

    #[tokio::test]
    async fn path_edits_are_sent_to_the_original_destination() {
        let mut proxy = TestProxy::start().await;
        let pending = proxy.intercept(post_request(&proxy.original_url)).await;
        let mut edited_request = pending.request;
        *edited_request.uri_mut() = "/edited?value=2".parse().unwrap();

        let received = proxy.forward(pending.reply, edited_request).await;

        assert_eq!(received.uri(), "/edited?value=2");
        assert_eq!(received.headers()["host"], "example.com");
        assert_eq!(received.body(), b"ping".as_slice());
    }

    #[tokio::test]
    async fn host_edits_are_sent_to_the_original_destination() {
        let mut proxy = TestProxy::start().await;
        let pending = proxy.intercept(post_request(&proxy.original_url)).await;
        let mut edited_request = pending.request;
        edited_request
            .headers_mut()
            .insert("host", "elsewhere.example:1234".parse().unwrap());

        let received = proxy.forward(pending.reply, edited_request).await;

        assert_eq!(received.headers()["host"], "elsewhere.example:1234");
        assert_eq!(received.uri(), "/inspect?value=1");
    }

    #[tokio::test]
    async fn proxy_headers_are_hidden_from_the_editor() {
        let mut proxy = TestProxy::start().await;
        let wire_request = format!(
            concat!(
                "POST {} HTTP/1.1\r\n",
                "Host: example.com\r\n",
                "Connection: close\r\n",
                "Connection: X-Proxy-Only\r\n",
                "X-Proxy-Only: remove\r\n",
                "Proxy-Connection: keep-alive\r\n",
                "Proxy-Authorization: Basic test\r\n",
                "X-Application: preserve\r\n",
                "Content-Length: 4\r\n",
                "\r\n",
                "ping",
            ),
            proxy.original_url
        );

        let pending = proxy.intercept(wire_request).await;
        let headers = pending.request.headers();

        assert!(!headers.contains_key("connection"));
        assert!(!headers.contains_key("proxy-connection"));
        assert!(!headers.contains_key("proxy-authorization"));
        assert!(!headers.contains_key("x-proxy-only"));
        assert_eq!(headers["x-application"], "preserve");
        assert_eq!(pending.request.body(), b"ping".as_slice());
    }

    #[tokio::test]
    async fn proxy_headers_added_in_the_editor_are_removed_before_forwarding() {
        let mut proxy = TestProxy::start().await;
        let pending = proxy.intercept(post_request(&proxy.original_url)).await;
        let edited_request = Request::builder()
            .method("POST")
            .uri("/inspect?value=1")
            .header("host", "example.com")
            .header("proxy-connection", "keep-alive")
            .header("proxy-authorization", "Basic edited")
            .header("connection", "x-edited-hop")
            .header("x-edited-hop", "remove")
            .header("x-application", "preserve")
            .body(pending.request.into_body())
            .unwrap();

        let received = proxy.forward(pending.reply, edited_request).await;
        let headers = received.headers();

        assert!(!headers.contains_key("proxy-connection"));
        assert!(!headers.contains_key("proxy-authorization"));
        assert!(!headers.contains_key("x-edited-hop"));
        assert_eq!(headers["x-application"], "preserve");
        assert_eq!(received.body(), b"ping".as_slice());
    }

    #[tokio::test]
    async fn chunked_body_is_decoded_before_editing() {
        let mut proxy = TestProxy::start().await;
        let wire_request = format!(
            concat!(
                "POST {} HTTP/1.1\r\n",
                "Host: example.com\r\n",
                "Connection: close\r\n",
                "Transfer-Encoding: chunked\r\n",
                "Trailer: x-checksum\r\n",
                "\r\n",
                "4\r\n",
                "ping\r\n",
                "0\r\n",
                "x-checksum: test\r\n",
                "\r\n",
            ),
            proxy.original_url
        );

        let pending = proxy.intercept(wire_request).await;

        assert_eq!(pending.request.body(), b"ping".as_slice());
        assert_eq!(pending.request.headers()["content-length"], "4");
        assert!(!pending.request.headers().contains_key("transfer-encoding"));
        assert!(!pending.request.headers().contains_key("trailer"));

        let received = proxy.forward(pending.reply, pending.request).await;

        assert_eq!(received.body(), b"ping".as_slice());
        assert_eq!(received.headers()["content-length"], "4");
    }

    #[tokio::test]
    async fn edited_binary_body_is_forwarded_with_its_updated_length() {
        let mut proxy = TestProxy::start().await;
        let pending = proxy.intercept(post_request(&proxy.original_url)).await;
        let mut edited_request = pending.request;
        *edited_request.body_mut() = Bytes::from_static(b"edited\x00\xff");

        let received = proxy.forward(pending.reply, edited_request).await;

        assert_eq!(received.body(), b"edited\x00\xff".as_slice());
        assert_eq!(received.headers()["content-length"], "8");
    }

    async fn intercept_body(
        body: Bytes,
    ) -> (PendingRequest, JoinHandle<Option<Request<ProxyBody>>>) {
        let (sender, mut requests) = mpsc::channel(1);
        let handler = InterceptHandler::new(sender);
        let request = buffered_request(body);
        let task = tokio::spawn(async move { handler.handle(original_uri(), request).await });
        let pending = timeout(TEST_TIMEOUT, requests.recv())
            .await
            .expect("handler did not queue the request")
            .expect("handler queue closed");
        (pending, task)
    }

    async fn handler_result(
        task: JoinHandle<Option<Request<ProxyBody>>>,
    ) -> Option<Request<Bytes>> {
        let request = timeout(TEST_TIMEOUT, task)
            .await
            .expect("handler did not finish")
            .expect("handler task failed")?;
        let (parts, body) = request.into_parts();
        let body = timeout(TEST_TIMEOUT, body.collect())
            .await
            .expect("forwarded body could not be read")
            .expect("reading forwarded body failed")
            .to_bytes();
        Some(Request::from_parts(parts, body))
    }

    #[tokio::test]
    async fn forward_returns_the_edited_request_with_updated_body_headers() {
        let (pending, task) = intercept_body(Bytes::from_static(b"ping")).await;
        let edited_request = Request::builder()
            .method("PATCH")
            .uri("/edited")
            // These lengths describe the original body, not the edited body.
            .header("content-length", 4)
            .header("transfer-encoding", "chunked")
            .header("trailer", "x-checksum")
            .body(Bytes::from_static(b"edited\x00\xff"))
            .unwrap();

        assert!(
            pending
                .reply
                .send(InterceptReply::Forward(Box::new(edited_request)))
                .is_ok()
        );
        let forwarded = handler_result(task).await.expect("request was dropped");

        assert_eq!(forwarded.method(), "PATCH");
        assert_eq!(forwarded.uri(), "/edited");
        assert_eq!(forwarded.body(), b"edited\x00\xff".as_slice());
        assert_eq!(forwarded.headers()["content-length"], "8");
        assert!(!forwarded.headers().contains_key("transfer-encoding"));
        assert!(!forwarded.headers().contains_key("trailer"));
    }

    #[tokio::test]
    async fn buffered_request_keeps_the_original_url_as_separate_metadata() {
        let (pending, task) = intercept_body(Bytes::from_static(b"ping")).await;

        assert_eq!(pending.original_uri, original_uri());
        assert_eq!(pending.request.uri(), "/original");
        assert_eq!(pending.request.body(), b"ping".as_slice());

        assert!(pending.reply.send(InterceptReply::Drop).is_ok());
        assert!(handler_result(task).await.is_none());
    }

    #[tokio::test]
    async fn drop_decision_returns_no_request() {
        let (pending, task) = intercept_body(Bytes::from_static(b"ping")).await;

        assert!(pending.reply.send(InterceptReply::Drop).is_ok());
        let result = handler_result(task).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn canceled_reply_returns_no_request() {
        let (pending, task) = intercept_body(Bytes::from_static(b"ping")).await;

        drop(pending.reply);
        let result = handler_result(task).await;

        assert!(result.is_none());
    }

    fn pending_body() -> (Request<ProxyBody>, oneshot::Receiver<()>) {
        let (notification, receiver) = oneshot::channel();
        let body = PendingBody {
            notify_when_polled: Some(notification),
        }
        .boxed();
        (Request::new(body), receiver)
    }

    #[tokio::test]
    async fn full_queue_drops_the_request_without_reading_its_body() {
        let (queue_sender, _queue_receiver) = mpsc::channel(1);

        let handler = InterceptHandler::new(queue_sender.clone());

        // Holding the only slot makes this queue full before the handler starts.
        let _occupied_slot = queue_sender
            .try_reserve()
            .expect("test setup must occupy the only queue slot");
        let (original, mut body_polled) = pending_body();

        let decision = timeout(TEST_TIMEOUT, handler.handle(original_uri(), original))
            .await
            .expect("a full queue must reject the request without waiting for its body");

        assert!(decision.is_none(), "a full queue must drop the request");
        assert!(
            matches!(
                body_polled.try_recv(),
                Err(oneshot::error::TryRecvError::Closed)
            ),
            "body must be dropped without sending a read notification"
        );
    }

    #[tokio::test]
    async fn closed_queue_drops_the_request_without_reading_its_body() {
        let (queue_sender, queue_receiver) = mpsc::channel(1);

        let handler = InterceptHandler::new(queue_sender);

        let (original, mut body_polled) = pending_body();

        drop(queue_receiver);

        let decision = timeout(TEST_TIMEOUT, handler.handle(original_uri(), original))
            .await
            .expect("a closed queue must reject the request without waiting for its body");

        assert!(decision.is_none(), "a closed queue must drop the request");
        assert!(
            matches!(
                body_polled.try_recv(),
                Err(oneshot::error::TryRecvError::Closed)
            ),
            "body must be dropped without sending a read notification"
        );
    }

    #[tokio::test]
    async fn oversized_body_is_never_queued() {
        let (queue_sender, mut queue_receiver) = mpsc::channel(1);
        let handler = InterceptHandler::new(queue_sender);
        let original = buffered_request(Bytes::from(vec![b'x'; MAX_BODY_SIZE + 1]));

        let decision = timeout(TEST_TIMEOUT, handler.handle(original_uri(), original))
            .await
            .expect("handler must reject an oversized body before the deadline");

        assert!(
            decision.is_none(),
            "an oversized body must drop the request"
        );
        assert!(
            matches!(
                queue_receiver.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ),
            "an oversized request must not reach the UI queue"
        );
    }

    #[tokio::test]
    async fn closing_ui_interrupts_a_body_read() {
        let (queue_sender, queue_receiver) = mpsc::channel(1);

        let handler = InterceptHandler::new(queue_sender);

        let (original, body_polled) = pending_body();

        let handler_task =
            tokio::spawn(async move { handler.handle(original_uri(), original).await });

        timeout(TEST_TIMEOUT, body_polled)
            .await
            .expect("handler must start reading the body before the deadline")
            .expect("body must notify the test when reading starts");

        drop(queue_receiver);

        let decision = handler_result(handler_task).await;

        assert!(decision.is_none(), "closing the UI must drop the request");
    }
}
