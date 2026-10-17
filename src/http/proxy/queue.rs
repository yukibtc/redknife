use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};

use crate::http::request::{HttpWireRequestError, format_request};

pub(crate) struct PendingRequest {
    pub(crate) id: u64,
    pub(crate) timestamp: DateTime<Local>,
    pub(crate) original_uri: Uri,
    pub(crate) raw: Bytes,
    pub(crate) original: Request<Bytes>,
}

impl PendingRequest {
    pub(crate) fn new(
        id: u64,
        timestamp: DateTime<Local>,
        original_uri: Uri,
        original: Request<Bytes>,
    ) -> Result<Self, HttpWireRequestError> {
        let raw: Bytes = format_request(&original)?.into();

        Ok(Self {
            id,
            timestamp,
            original_uri,
            raw,
            original,
        })
    }
}
