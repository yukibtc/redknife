use std::convert::Infallible;
use std::io;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use thiserror::Error;

pub type HttpBody = BoxBody<Bytes, HttpBodyError>;

#[derive(Debug, Error)]
pub enum HttpBodyError {
    #[error("HTTP body transport error: {0}")]
    Hyper(#[from] hyper::Error),
    #[error("HTTP body I/O error: {0}")]
    IO(#[from] io::Error),
}

impl From<Infallible> for HttpBodyError {
    fn from(error: Infallible) -> Self {
        match error {}
    }
}
