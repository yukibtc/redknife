use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::Request;
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

#[async_trait::async_trait]
impl HttpForwardProxyServerRequestHandler for InterceptHandler {
    async fn handle(&self, request: Request<ProxyBody>) -> Option<Request<ProxyBody>> {
        // Reserve capacity before reading a potentially large or slow body.
        let permit: Permit<PendingRequest> = self.sender.try_reserve().ok()?;

        let timestamp: DateTime<Local> = Local::now();

        let (parts, body) = request.into_parts();

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
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use bytes::Bytes;
    use http::Request;
    use http_body_util::{BodyExt, Full};
    use hyper::body::{Body, Frame};
    use tokio::sync::{mpsc, oneshot};
    use tokio::time::timeout;

    use super::{InterceptHandler, MAX_BODY_SIZE};
    use crate::command::http::intercept::messages::InterceptReply;
    use crate::proxy::http_forward_server::{HttpForwardProxyServerRequestHandler, ProxyBody};

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn buffered_request(body: Bytes) -> Request<ProxyBody> {
        Request::builder()
            .method("POST")
            .uri("http://example.com/original")
            .header("content-length", body.len())
            .body(Full::new(body).map_err(|never| match never {}).boxed())
            .expect("test request must be valid")
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

    #[tokio::test]
    async fn forward_returns_the_edited_request_with_updated_body_headers() {
        let (queue_sender, mut queue_receiver) = mpsc::channel(1);

        let handler = InterceptHandler::new(queue_sender);

        let original = buffered_request(Bytes::from_static(b"ping"));

        let handler_task = tokio::spawn(async move { handler.handle(original).await });

        let pending = timeout(TEST_TIMEOUT, queue_receiver.recv())
            .await
            .expect("handler must enqueue the request before the deadline")
            .expect("request queue must remain open");

        assert_eq!(pending.request.body(), b"ping".as_slice());

        // The editor may leave stale framing headers; the handler must rebuild them.
        let edited_request = Request::builder()
            .method("PATCH")
            .uri("http://elsewhere.example/edited")
            .header("content-length", 4)
            .header("transfer-encoding", "chunked")
            .header("trailer", "x-checksum")
            .body(Bytes::from_static(b"edited\x00\xff"))
            .expect("edited test request must be valid");

        let reply_sent = pending.reply.send(InterceptReply::Forward(edited_request));

        assert!(
            reply_sent.is_ok(),
            "handler must be waiting for the decision"
        );

        let decision = timeout(TEST_TIMEOUT, handler_task)
            .await
            .expect("handler must finish after Forward")
            .expect("handler task must not panic");
        let forwarded = decision.expect("Forward must return a request");

        assert_eq!(forwarded.method(), "PATCH");
        assert_eq!(forwarded.uri(), "http://elsewhere.example/edited");
        assert_eq!(forwarded.headers()["content-length"], "8");
        assert!(!forwarded.headers().contains_key("transfer-encoding"));
        assert!(!forwarded.headers().contains_key("trailer"));

        let forwarded_body = timeout(TEST_TIMEOUT, forwarded.into_body().collect())
            .await
            .expect("forwarded body must be readable before the deadline")
            .expect("reading the forwarded body must succeed")
            .to_bytes();

        assert_eq!(forwarded_body, b"edited\x00\xff".as_slice());
    }

    #[tokio::test]
    async fn drop_decision_returns_no_request() {
        let (queue_sender, mut queue_receiver) = mpsc::channel(1);

        let handler = InterceptHandler::new(queue_sender);

        let original = buffered_request(Bytes::from_static(b"ping"));

        let handler_task = tokio::spawn(async move { handler.handle(original).await });

        let pending = timeout(TEST_TIMEOUT, queue_receiver.recv())
            .await
            .expect("handler must enqueue the request before the deadline")
            .expect("request queue must remain open");

        let reply_sent = pending.reply.send(InterceptReply::Drop);

        assert!(
            reply_sent.is_ok(),
            "handler must be waiting for the decision"
        );

        let decision = timeout(TEST_TIMEOUT, handler_task)
            .await
            .expect("handler must finish after Drop")
            .expect("handler task must not panic");

        assert!(decision.is_none(), "Drop must not return a request");
    }

    #[tokio::test]
    async fn canceled_reply_returns_no_request() {
        let (queue_sender, mut queue_receiver) = mpsc::channel(1);

        let handler = InterceptHandler::new(queue_sender);

        let original = buffered_request(Bytes::from_static(b"ping"));

        let handler_task = tokio::spawn(async move { handler.handle(original).await });

        let pending = timeout(TEST_TIMEOUT, queue_receiver.recv())
            .await
            .expect("handler must enqueue the request before the deadline")
            .expect("request queue must remain open");

        drop(pending.reply);

        let decision = timeout(TEST_TIMEOUT, handler_task)
            .await
            .expect("handler must finish when the reply sender is dropped")
            .expect("handler task must not panic");

        assert!(decision.is_none(), "a canceled reply must drop the request");
    }

    #[tokio::test]
    async fn full_queue_drops_the_request_without_reading_its_body() {
        let (queue_sender, _queue_receiver) = mpsc::channel(1);

        let handler = InterceptHandler::new(queue_sender.clone());

        // Holding the only slot makes this queue full before the handler starts.
        let _occupied_slot = queue_sender
            .try_reserve()
            .expect("test setup must occupy the only queue slot");
        let (notify_when_polled, mut body_polled) = oneshot::channel();
        let original = Request::new(
            PendingBody {
                notify_when_polled: Some(notify_when_polled),
            }
            .boxed(),
        );

        let decision = timeout(TEST_TIMEOUT, handler.handle(original))
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

        let (notify_when_polled, mut body_polled) = oneshot::channel();

        let original = Request::new(
            PendingBody {
                notify_when_polled: Some(notify_when_polled),
            }
            .boxed(),
        );

        drop(queue_receiver);

        let decision = timeout(TEST_TIMEOUT, handler.handle(original))
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

        let decision = timeout(TEST_TIMEOUT, handler.handle(original))
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

        let (notify_when_polled, body_polled) = oneshot::channel();

        let original = Request::new(
            PendingBody {
                notify_when_polled: Some(notify_when_polled),
            }
            .boxed(),
        );

        let handler_task = tokio::spawn(async move { handler.handle(original).await });

        timeout(TEST_TIMEOUT, body_polled)
            .await
            .expect("handler must start reading the body before the deadline")
            .expect("body must notify the test when reading starts");

        drop(queue_receiver);

        let decision = timeout(TEST_TIMEOUT, handler_task)
            .await
            .expect("closing the UI must interrupt the pending body read")
            .expect("handler task must not panic");

        assert!(decision.is_none(), "closing the UI must drop the request");
    }
}
