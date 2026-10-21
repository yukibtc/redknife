use thiserror::Error;

use crate::dns::name::{DomainNameError, LabelError};

#[derive(Debug, Error)]
pub enum DnsMessageError {
    #[error("message truncated")]
    Truncated,
    #[error("unexpected end of message")]
    UnexpectedEof,
    #[error("invalid compression pointer at offset: {0}")]
    InvalidPointer(usize),
    #[error("too many compression pointers")]
    TooManyPointers,
    #[error("invalid rdata: {0}")]
    InvalidRData(&'static str),
    #[error("invalid domain name: {0}")]
    InvalidDomainName(#[from] DomainNameError),
    #[error("invalid label: {0}")]
    InvalidLabel(#[from] LabelError),
}
