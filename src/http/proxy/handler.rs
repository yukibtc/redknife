use std::sync::Arc;

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use http_body_util::{BodyExt, Full, Limited};
use tokio::sync::mpsc;

use super::state::ProxyState;
use super::{HttpProxyError, PendingRequest, remove_hop_by_hop};
use crate::http::body::HttpBody;
use crate::http::compression::unpack_request;
use crate::http::constant::MAX_BODY_SIZE;
use crate::http::request::normalize_body_headers;

#[derive(Debug)]
pub(super) enum ProxyHandleDropReason {
    // /// The request forward has been explicitly denied
    // ForwardDenied,
    /// Channel reservation failed
    ChannelReservation,
    /// The main channel has been closed
    ChannelClosed,
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

#[derive(Clone)]
pub(super) struct ProxyHandler {
    sender: mpsc::Sender<PendingRequest>,
    state: Arc<ProxyState>,
}

impl ProxyHandler {
    #[inline]
    pub(super) fn new(sender: mpsc::Sender<PendingRequest>) -> Self {
        Self {
            sender,
            state: Arc::new(ProxyState::default()),
        }
    }

    pub(super) async fn handle(
        &self,
        original_uri: Uri,
        req: Request<HttpBody>,
    ) -> Result<ProxyHandleOutcome, HttpProxyError> {
        let Ok(permit) = self.sender.try_reserve() else {
            return Ok(ProxyHandleOutcome::channel_reservation());
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

        // let (reply, receiver) = oneshot::channel();

        let pending: PendingRequest = PendingRequest::new(id, timestamp, original_uri, buffered)?;

        let request = pending.original.clone();

        // Send the request in the mpsc channel
        permit.send(pending);

        // // Wait for decision
        // let decision: InnerDecision = match receiver.await {
        //     Ok(Some(request)) => InnerDecision::Continue(Box::new(request)),
        //     Ok(None) => InnerDecision::Drop(ProxyHandleDropReason::ForwardDenied),
        //     Err(_) => InnerDecision::Drop(ProxyHandleDropReason::OneShotChannelClosed),
        // };

        // // Prepare the forward, or map the drop outcome
        // let result = match decision {
        //     InnerDecision::Continue(request) => {
        //         prepare_forward(*request, &mut record).await
        //     }
        //     InnerDecision::Drop(reason) => Ok(ProxyHandleOutcome::Drop(reason)),
        // };
        //
        // result

        prepare_forward(request).await
    }
}

#[inline]
async fn prepare_forward(
    mut request: Request<Bytes>,
) -> Result<ProxyHandleOutcome, HttpProxyError> {
    // Unpack the compressed request
    unpack_request(&mut request).await?;

    remove_hop_by_hop(request.headers_mut());
    normalize_body_headers(&mut request);

    let request: Request<HttpBody> =
        request.map(|body| Full::new(body).map_err(|never| match never {}).boxed());

    Ok(ProxyHandleOutcome::Process(Box::new(request)))
}
