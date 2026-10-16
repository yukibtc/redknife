use bytes::Bytes;
use chrono::{DateTime, Local};
use http::Uri;

use crate::http::target::{HttpTarget, HttpTargetError};

#[derive(Clone)]
pub(crate) struct RequestRecord {
    pub(crate) id: u64,
    pub(crate) timestamp: DateTime<Local>,
    pub(crate) original_uri: Uri,
    pub(crate) raw: Bytes,
    pub(crate) status: RequestStatus,
}

impl RequestRecord {
    #[inline]
    pub(crate) fn target(&self) -> Result<HttpTarget, HttpTargetError> {
        HttpTarget::from_uri(&self.original_uri)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestStatus {
    Pending,
    Forwarded,
    Dropped,
}

impl RequestStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Forwarded => "Forwarded",
            Self::Dropped => "Dropped",
        }
    }
}
