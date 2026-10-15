use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use tokio::sync::oneshot;

mod handler;
mod queue;

pub(crate) use self::handler::HttpInterceptHandler;
pub(crate) use self::queue::{QueuedRequest, RequestQueue};

pub(crate) enum HttpInterceptReply {
    Forward(Box<Request<Bytes>>),
    Drop,
}

pub(crate) struct HttpInterceptPendingRequest {
    pub id: u64,
    pub timestamp: DateTime<Local>,
    /// The intercepted URL, retained independently of the editable request.
    pub original_uri: Uri,
    pub request: Request<Bytes>,
    pub reply: oneshot::Sender<HttpInterceptReply>,
}
