use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use tokio::sync::mpsc;

use super::rules::{RuleOutcome, RuleReport, Rules};
use crate::http::proxy::{HttpProxyServerHandler, HttpProxyServerHandlerDecision};
use crate::http::request::format_request;

#[derive(Clone)]
pub(super) struct MiddlewareEvent {
    pub(super) id: u64,
    pub(super) timestamp: DateTime<Local>,
    pub(super) uri: Uri,
    pub(super) original: Request<Bytes>,
    pub(super) original_raw: Bytes,
    pub(super) modified_raw: Option<Bytes>,
    pub(super) report: RuleReport,
}

pub(super) struct HttpProxyServerMiddleware {
    next_id: AtomicU64,
    tx: mpsc::UnboundedSender<MiddlewareEvent>,
    rules: Rules,
}

impl HttpProxyServerMiddleware {
    #[inline]
    pub(super) fn new(tx: mpsc::UnboundedSender<MiddlewareEvent>, rules: Rules) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            tx,
            rules,
        }
    }

    #[inline]
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl HttpProxyServerHandler for HttpProxyServerMiddleware {
    fn handle(
        &self,
        uri: Uri,
        mut req: Request<Bytes>,
    ) -> Pin<Box<dyn Future<Output = io::Result<HttpProxyServerHandlerDecision>> + Send + '_>> {
        Box::pin(async move {
            let id: u64 = self.next_id();
            let timestamp: DateTime<Local> = Local::now();

            let original: Request<Bytes> = req.clone();

            let report: RuleReport = self.rules.apply(&uri, &mut req);

            let changed: bool = original.method() != req.method()
                || original.uri() != req.uri()
                || original.version() != req.version()
                || original.headers() != req.headers()
                || original.body() != req.body();

            let modified_raw: Option<Bytes> = if changed {
                Some(format_request(&req).map_err(io::Error::other)?)
            } else {
                None
            };

            let outcome: RuleOutcome = report.outcome;

            self.tx
                .send(MiddlewareEvent {
                    id,
                    timestamp,
                    uri,
                    original_raw: format_request(&original).map_err(io::Error::other)?,
                    original,
                    modified_raw,
                    report,
                })
                .map_err(io::Error::other)?;

            Ok(match outcome {
                RuleOutcome::Continue => HttpProxyServerHandlerDecision::Forward(Box::new(req)),
                RuleOutcome::Deny(code) => HttpProxyServerHandlerDecision::Deny(code),
            })
        })
    }
}
