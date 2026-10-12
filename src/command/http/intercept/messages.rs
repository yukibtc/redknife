use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use tokio::sync::oneshot;

pub enum InterceptReply {
    Forward(Request<Bytes>),
    Drop,
}

pub struct PendingRequest {
    pub id: u64,
    pub timestamp: DateTime<Local>,
    /// The intercepted URL, retained independently of the editable request.
    pub original_uri: Uri,
    pub request: Request<Bytes>,
    pub reply: oneshot::Sender<InterceptReply>,
}
