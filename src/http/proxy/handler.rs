use std::sync::Arc;

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use http_body_util::{BodyExt, Full, Limited};
use tokio::sync::mpsc::Permit;
use tokio::sync::{mpsc, oneshot};

use super::{
    HttpProxyError, PendingRequest, ProxyState, RequestRecord, RequestStatus, remove_hop_by_hop,
};
use crate::http::body::HttpBody;
use crate::http::compression::unpack_request;
use crate::http::constant::MAX_BODY_SIZE;
use crate::http::proxy::state::DispatchOutcome;
use crate::http::request::{format_request, normalize_body_headers};

#[derive(Debug)]
pub(super) enum ProxyHandleDropReason {
    /// The request forward has been explicitly denied
    ForwardDenied,
    /// Channel reservation failed
    ChannelReservation,
    /// The main channel has been closed
    ChannelClosed,
    /// The oneshot channel has been closed
    OneShotChannelClosed,
}

pub(super) enum ProxyHandleOutcome {
    /// Go ahead and process the request
    Process(Box<Request<HttpBody>>),
    /// Drop the request
    Drop(ProxyHandleDropReason),
}

impl ProxyHandleOutcome {
    #[inline]
    fn channel_reservation() -> Self {
        Self::Drop(ProxyHandleDropReason::ChannelReservation)
    }

    #[inline]
    fn channel_closed() -> Self {
        Self::Drop(ProxyHandleDropReason::ChannelClosed)
    }
}

enum InnerDecision {
    Continue(Box<Request<Bytes>>),
    Drop(ProxyHandleDropReason),
}

#[derive(Clone)]
pub(super) struct ProxyHandler {
    sender: mpsc::Sender<PendingRequest>,
    state: Arc<ProxyState>,
}

impl ProxyHandler {
    #[inline]
    pub(super) fn new(sender: mpsc::Sender<PendingRequest>, state: Arc<ProxyState>) -> Self {
        Self { sender, state }
    }

    pub(super) async fn handle(
        &self,
        original_uri: Uri,
        req: Request<HttpBody>,
    ) -> Result<ProxyHandleOutcome, HttpProxyError> {
        // OFF traffic never consumes pending capacity.
        let permit: Option<Permit<'_, PendingRequest>> = if self.state.is_intercepting() {
            let Ok(permit) = self.sender.try_reserve() else {
                return Ok(ProxyHandleOutcome::channel_reservation());
            };
            Some(permit)
        } else {
            None
        };

        if self.sender.is_closed() {
            return Ok(ProxyHandleOutcome::channel_closed());
        }

        let timestamp: DateTime<Local> = Local::now();

        let id: u64 = self.state.next_id();

        let (parts, body) = req.into_parts();

        let collected: Bytes = tokio::select! {
            result = Limited::new(body, MAX_BODY_SIZE).collect() => result.map_err(HttpProxyError::LimitedCollector)?.to_bytes(),
            _ = self.sender.closed() => return Ok(ProxyHandleOutcome::channel_closed()),
        };

        let mut buffered: Request<Bytes> = Request::from_parts(parts, collected);

        tokio::select! {
            result = unpack_request(&mut buffered) => { result?; },
            _ = self.sender.closed() => return Ok(ProxyHandleOutcome::channel_closed()),
        }

        normalize_body_headers(&mut buffered);

        let (reply, receiver) = oneshot::channel();

        let pending: PendingRequest =
            PendingRequest::new(id, timestamp, original_uri, buffered, reply)?;

        // Preserve the original for history before moving pending into the queue.
        let mut history_record: RequestRecord = pending.record().clone();

        // Mark it as dropped. In case it will be forwarded, it will be updated later.
        history_record.status = RequestStatus::Dropped;

        // Wait for decision
        let decision: InnerDecision = match self.state.dispatch(pending, &self.sender, permit) {
            DispatchOutcome::Continue(pending) => {
                InnerDecision::Continue(Box::new(pending.original()))
            }
            DispatchOutcome::WaitForInterceptorDecision => match receiver.await {
                Ok(Some(request)) => InnerDecision::Continue(Box::new(request)),
                Ok(None) => InnerDecision::Drop(ProxyHandleDropReason::ForwardDenied),
                Err(_) => InnerDecision::Drop(ProxyHandleDropReason::OneShotChannelClosed),
            },
            DispatchOutcome::ChannelClosed => {
                InnerDecision::Drop(ProxyHandleDropReason::ChannelClosed)
            }
        };

        // Prepare the forward, or map the drop outcome
        let result = match decision {
            InnerDecision::Continue(request) => {
                prepare_forward(*request, &mut history_record).await
            }
            InnerDecision::Drop(reason) => Ok(ProxyHandleOutcome::Drop(reason)),
        };

        // Save the history record
        self.state.record(history_record);

        result
    }
}

#[inline]
async fn prepare_forward(
    mut request: Request<Bytes>,
    record: &mut RequestRecord,
) -> Result<ProxyHandleOutcome, HttpProxyError> {
    // Unpack the compressed request
    unpack_request(&mut request).await?;

    remove_hop_by_hop(request.headers_mut());
    normalize_body_headers(&mut request);

    record.raw = format_request(&request)?.into();
    record.status = RequestStatus::Forwarded;

    let request: Request<HttpBody> =
        request.map(|body| Full::new(body).map_err(|never| match never {}).boxed());

    Ok(ProxyHandleOutcome::Process(Box::new(request)))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::stream;
    use http_body_util::StreamBody;
    use hyper::body::Frame;
    use tokio::time::timeout;

    use super::super::RequestQueue;
    use super::*;
    use crate::http::body::HttpBodyError;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn intercepting_handler(sender: mpsc::Sender<PendingRequest>) -> ProxyHandler {
        let state: Arc<ProxyState> = Arc::default();
        state.toggle_intercepting();
        ProxyHandler::new(sender, state)
    }

    fn buffered_request(body: Bytes) -> Request<HttpBody> {
        Request::builder()
            .method("POST")
            .uri("/original")
            .body(Full::new(body).map_err(|never| match never {}).boxed())
            .unwrap()
    }

    fn original_uri() -> Uri {
        "http://example.com/original".parse().unwrap()
    }

    // Reading pauses until the test changes the interceptor state or closes the UI.
    fn gated_request() -> (
        Request<HttpBody>,
        oneshot::Receiver<()>,
        oneshot::Sender<()>,
    ) {
        let (started, body_started) = oneshot::channel();
        let (release, body_release) = oneshot::channel();
        let frames = stream::once(async move {
            started.send(()).unwrap();
            body_release.await.unwrap();
            Ok::<_, HttpBodyError>(Frame::data(Bytes::from_static(b"slow body")))
        });
        let request: Request<HttpBody> = Request::new(StreamBody::new(frames).boxed());
        (request, body_started, release)
    }

    #[tokio::test]
    async fn test_invalid_compressed_draft_records_the_original() {
        let (sender, mut receiver) = mpsc::channel(1);

        let handler: ProxyHandler = intercepting_handler(sender);
        let request: Request<HttpBody> = buffered_request(Bytes::from_static(b"original"));
        let draft: &[u8] = b"POST /edited HTTP/1.1\r\nContent-Encoding: gzip\r\n\r\ninvalid gzip";

        let edit = async {
            let pending: PendingRequest = receiver.recv().await.unwrap();
            let original: Bytes = pending.record().raw.clone();
            let id: u64 = pending.record().id;
            let mut queue: RequestQueue = RequestQueue::default();
            queue.push(pending);
            queue.set_draft(id, draft.to_vec());
            queue.resolve(0, true).unwrap();
            original
        };

        let (result, original) = timeout(TEST_TIMEOUT, async {
            tokio::join!(handler.handle(original_uri(), request), edit)
        })
        .await
        .unwrap();

        let history: Vec<Arc<RequestRecord>> = handler.state.history_since(0).unwrap();

        assert!(matches!(result, Err(HttpProxyError::Unpack(_))));
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, RequestStatus::Dropped);
        assert_eq!(history[0].raw, original);
    }

    #[tokio::test]
    async fn test_full_queue_drops_without_waiting_for_the_body() {
        let (sender, _receiver) = mpsc::channel(1);

        let handler: ProxyHandler = intercepting_handler(sender.clone());
        let _occupied_slot: Permit<'_, PendingRequest> = sender.try_reserve().unwrap();
        let frames = stream::pending::<Result<Frame<Bytes>, HttpBodyError>>();
        let request: Request<HttpBody> = Request::new(StreamBody::new(frames).boxed());

        let result: Result<ProxyHandleOutcome, HttpProxyError> =
            timeout(TEST_TIMEOUT, handler.handle(original_uri(), request))
                .await
                .expect("a full queue must reject before reading the body");

        assert!(matches!(
            result,
            Ok(ProxyHandleOutcome::Drop(
                ProxyHandleDropReason::ChannelReservation
            ))
        ));
    }

    #[tokio::test]
    async fn test_oversized_body_is_not_queued() {
        let (sender, mut receiver) = mpsc::channel(1);

        let handler: ProxyHandler = intercepting_handler(sender);
        let request: Request<HttpBody> =
            buffered_request(Bytes::from(vec![b'x'; MAX_BODY_SIZE + 1]));

        let result: Result<ProxyHandleOutcome, HttpProxyError> =
            handler.handle(original_uri(), request).await;

        assert!(matches!(result, Err(HttpProxyError::LimitedCollector(_))));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn test_closing_ui_interrupts_a_body_read() {
        let (sender, receiver) = mpsc::channel(1);

        let handler: ProxyHandler = intercepting_handler(sender);
        let (request, started, _release) = gated_request();

        let close_ui = async {
            started.await.unwrap();
            drop(receiver);
        };

        let (result, ()) = timeout(TEST_TIMEOUT, async {
            tokio::join!(handler.handle(original_uri(), request), close_ui)
        })
        .await
        .unwrap();

        assert!(matches!(
            result,
            Ok(ProxyHandleOutcome::Drop(
                ProxyHandleDropReason::ChannelClosed
            ))
        ));
    }

    #[tokio::test]
    async fn test_disabling_during_a_body_read_forwards_without_queueing() {
        let (sender, receiver) = mpsc::channel(1);

        let handler: ProxyHandler = intercepting_handler(sender);
        let (request, started, release) = gated_request();

        let disable_intercept = async {
            started.await.unwrap();
            assert!(!handler.state.toggle_intercepting());
            release.send(()).unwrap();
        };

        let (result, ()) = timeout(TEST_TIMEOUT, async {
            tokio::join!(handler.handle(original_uri(), request), disable_intercept)
        })
        .await
        .unwrap();

        assert!(matches!(result, Ok(ProxyHandleOutcome::Process(_))));
        assert!(receiver.is_empty());
    }
}
