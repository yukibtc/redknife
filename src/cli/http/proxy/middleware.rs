use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use tokio::sync::{mpsc, watch};

use super::rules::{RuleOutcome, RuleReport, Rules};
use crate::http::proxy::{HttpProxyHandler, HttpProxyHandlerDecision};
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
    rules: watch::Receiver<Arc<Rules>>,
}

impl HttpProxyServerMiddleware {
    #[inline]
    pub(super) fn new(
        tx: mpsc::UnboundedSender<MiddlewareEvent>,
        rules: watch::Receiver<Arc<Rules>>,
    ) -> Self {
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

impl HttpProxyHandler for HttpProxyServerMiddleware {
    fn handle(
        &self,
        uri: Uri,
        mut req: Request<Bytes>,
    ) -> Pin<Box<dyn Future<Output = io::Result<HttpProxyHandlerDecision>> + Send + '_>> {
        Box::pin(async move {
            let id: u64 = self.next_id();
            let timestamp: DateTime<Local> = Local::now();
            let rules: Arc<Rules> = Arc::clone(&self.rules.borrow());

            let original: Request<Bytes> = req.clone();

            let report: RuleReport = rules.apply(&uri, &mut req);

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
                RuleOutcome::Continue => HttpProxyHandlerDecision::Forward(Box::new(req)),
                RuleOutcome::Deny(code) => HttpProxyHandlerDecision::Deny(code),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use http::StatusCode;

    use super::*;

    #[tokio::test]
    async fn uses_latest_rules() {
        let (rules, receiver) = watch::channel(Arc::new(Rules::default()));
        let (sender, mut events) = mpsc::unbounded_channel();
        let middleware = HttpProxyServerMiddleware::new(sender, receiver);

        let uri = Uri::from_static("http://example.com/test");
        let request = Request::builder()
            .uri("/test")
            .body(Bytes::from_static(b"original"))
            .unwrap();

        // Requests initially use the empty rule set
        let decision = middleware
            .handle(uri.clone(), request.clone())
            .await
            .unwrap();
        assert!(matches!(decision, HttpProxyHandlerDecision::Forward(_)));
        assert!(events.try_recv().unwrap().report.matched_rules.is_empty());

        // The same middleware uses the next published rule set
        let updated: Rules = toml::from_str(
            r#"
[[rules]]
name = "block"
match = {}
deny = { code = 429 }
"#,
        )
        .unwrap();
        rules.send_replace(Arc::new(updated));

        let decision = middleware.handle(uri, request).await.unwrap();

        assert!(matches!(
            decision,
            HttpProxyHandlerDecision::Deny(StatusCode::TOO_MANY_REQUESTS)
        ));
        assert_eq!(
            events.try_recv().unwrap().report.matched_rules,
            vec!["block"]
        );
    }

    #[tokio::test]
    async fn forwards_without_a_reload_source() {
        let (rules, receiver) = watch::channel(Arc::new(Rules::default()));
        drop(rules);

        let (sender, mut events) = mpsc::unbounded_channel();
        let middleware = HttpProxyServerMiddleware::new(sender, receiver);

        let uri = Uri::from_static("http://example.com/test");
        let request = Request::builder()
            .uri("/test")
            .body(Bytes::from_static(b"original"))
            .unwrap();

        let decision = middleware.handle(uri, request).await.unwrap();
        let HttpProxyHandlerDecision::Forward(request) = decision else {
            panic!("Request was unexpectedly blocked");
        };

        assert_eq!(request.body().as_ref(), b"original");

        let event = events.try_recv().unwrap();

        assert!(event.report.matched_rules.is_empty());
        assert!(event.modified_raw.is_none());
    }
}
