use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use tokio::sync::mpsc;

use crate::http::proxy::{HttpProxyServerHandler, HttpProxyServerHandlerDecision};
use crate::http::request::format_request;

pub(super) struct MiddlewareRequest {
    pub(super) id: u64,
    pub(super) timestamp: DateTime<Local>,
    pub(super) uri: Uri,
    pub(super) req: Request<Bytes>,
    pub(super) raw: Bytes,
}

pub(super) struct HttpProxyServerMiddleware {
    next_id: AtomicU64,
    tx: mpsc::UnboundedSender<MiddlewareRequest>,
}

impl HttpProxyServerMiddleware {
    #[inline]
    pub(super) fn new(tx: mpsc::UnboundedSender<MiddlewareRequest>) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            tx,
        }
    }
}

impl HttpProxyServerHandler for HttpProxyServerMiddleware {
    fn handle(
        &self,
        uri: Uri,
        req: Request<Bytes>,
    ) -> Pin<Box<dyn Future<Output = io::Result<HttpProxyServerHandlerDecision>> + Send + '_>> {
        Box::pin(async move {
            let id: u64 = self.next_id.fetch_add(1, Ordering::Relaxed);
            let timestamp: DateTime<Local> = Local::now();

            self.tx
                .send(MiddlewareRequest {
                    id,
                    timestamp,
                    uri,
                    req: req.clone(),
                    raw: format_request(&req).map_err(io::Error::other)?,
                })
                .map_err(io::Error::other)?;

            // TODO: intercept

            Ok(HttpProxyServerHandlerDecision::Forward(Box::new(req)))
        })
    }
}
