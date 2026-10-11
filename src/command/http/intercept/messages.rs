use bytes::Bytes;
use chrono::{DateTime, Local};
use http::Request;
use tokio::sync::oneshot;

pub enum InterceptReply {
    Forward(Request<Bytes>),
    Drop,
}

pub struct PendingRequest {
    pub id: u64,
    pub timestamp: DateTime<Local>,
    pub request: Request<Bytes>,
    pub reply: oneshot::Sender<InterceptReply>,
}
