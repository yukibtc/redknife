use std::mem;

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use indexmap::IndexMap;
use tokio::sync::oneshot;

use super::{RequestRecord, RequestStatus};
use crate::http::request::{HttpWireRequestError, format_request, parse_request};

pub(crate) struct PendingRequest {
    record: RequestRecord,
    original: Request<Bytes>,
    original_raw: Bytes,
    reply: oneshot::Sender<Option<Request<Bytes>>>,
}

impl PendingRequest {
    pub(crate) fn new(
        id: u64,
        timestamp: DateTime<Local>,
        original_uri: Uri,
        original: Request<Bytes>,
        reply: oneshot::Sender<Option<Request<Bytes>>>,
    ) -> Result<Self, HttpWireRequestError> {
        let raw: Bytes = format_request(&original)?.into();

        Ok(Self {
            record: RequestRecord {
                id,
                timestamp,
                original_uri,
                raw: raw.clone(),
                status: RequestStatus::Pending,
            },
            original,
            original_raw: raw,
            reply,
        })
    }

    #[inline]
    pub(crate) fn record(&self) -> &RequestRecord {
        &self.record
    }

    #[inline]
    pub(super) fn original(self) -> Request<Bytes> {
        self.original
    }

    /// Returns `Some` if the request can be forwarded, `None` otherwise.
    fn decision(&self, forward: bool) -> Result<Option<Request<Bytes>>, HttpWireRequestError> {
        if !forward {
            return Ok(None);
        }

        // Reuse the parsed original after unchanged edits too: the editor parser has
        // stricter header limits than the HTTP server accepting the original request.
        let request: Request<Bytes> = if self.record.raw == self.original_raw {
            self.original.clone()
        } else {
            parse_request(&self.record.raw)?
        };

        if request.uri().scheme().is_some() || request.uri().authority().is_some() {
            return Err(HttpWireRequestError::ExpectedOriginForm);
        }

        Ok(Some(request))
    }
}

#[derive(Default)]
pub(crate) struct RequestQueue {
    requests: IndexMap<u64, PendingRequest>,
}

impl RequestQueue {
    #[inline]
    pub(crate) fn push(&mut self, pending: PendingRequest) {
        self.requests.insert(pending.record.id, pending);
    }

    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<&PendingRequest> {
        self.requests.get_index(index).map(|(_, request)| request)
    }

    #[inline]
    pub(crate) fn index_of(&self, id: u64) -> Option<usize> {
        self.requests.get_index_of(&id)
    }

    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &PendingRequest> {
        self.requests.values()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.requests.len()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    pub(crate) fn set_draft(&mut self, id: u64, raw: Vec<u8>) -> bool {
        let Some(request) = self.requests.get_mut(&id) else {
            return false;
        };

        request.record.raw = raw.into();

        true
    }

    pub(crate) fn resolve(
        &mut self,
        index: usize,
        forward: bool,
    ) -> Result<(), HttpWireRequestError> {
        let Some(current) = self.get(index) else {
            return Ok(());
        };

        let decision: Option<Request<Bytes>> = current.decision(forward)?;

        if let Some((_, current)) = self.requests.shift_remove_index(index) {
            let _ = current.reply.send(decision);
        }

        Ok(())
    }

    /// Resolve a snapshot, retaining invalid drafts in their original relative order.
    pub(crate) fn resolve_all(&mut self, forward: bool) -> (usize, usize) {
        let requests: IndexMap<u64, PendingRequest> = mem::take(&mut self.requests);
        let count: usize = requests.len();
        let mut failed: usize = 0;

        for (id, current) in requests {
            let decision: Option<Request<Bytes>> = match current.decision(forward) {
                Ok(decision) => decision,
                Err(_) => {
                    self.requests.insert(id, current);
                    failed += 1;
                    continue;
                }
            };

            let _ = current.reply.send(decision);
        }

        (count - failed, failed)
    }
}

impl Drop for RequestQueue {
    fn drop(&mut self) {
        self.resolve_all(false);
    }
}

#[cfg(test)]
mod tests {
    use chrono::Local;
    use tokio::sync::oneshot;

    use super::*;

    fn pending(id: u64) -> (PendingRequest, oneshot::Receiver<Option<Request<Bytes>>>) {
        let (reply, receiver) = oneshot::channel();
        let request: Request<Bytes> = Request::builder()
            .method("POST")
            .uri("/original")
            .header("host", "virtual.host")
            .body(Bytes::from_static(b"\xff\x00body\n\n"))
            .unwrap();

        (
            PendingRequest::new(
                id,
                Local::now(),
                "http://127.0.0.1:8000/original".parse().unwrap(),
                request,
                reply,
            )
            .unwrap(),
            receiver,
        )
    }

    #[test]
    fn test_ids_follow_queue_order() {
        let mut queue: RequestQueue = RequestQueue::default();

        for id in [30, 10, 20] {
            let (request, _reply) = pending(id);
            queue.push(request);
        }

        assert_eq!(queue.index_of(10), Some(1));
        assert_eq!(queue.index_of(99), None);
        assert!(!queue.set_draft(99, Vec::new()));

        let draft: Vec<u8> = b"GET /edited HTTP/1.1\r\n\r\n".to_vec();

        assert!(queue.set_draft(10, draft.clone()));
        assert_eq!(queue.get(1).unwrap().record.raw, draft);

        queue.resolve(0, false).unwrap();

        let ids: Vec<u64> = queue.iter().map(|request| request.record.id).collect();

        assert_eq!(ids, [10, 20]);
        assert_eq!(queue.index_of(10), Some(0));
        assert_eq!(queue.index_of(30), None);
    }

    #[test]
    fn test_restoring_the_original_draft_does_not_reparse_it() {
        let (reply, mut receiver) = oneshot::channel();

        let mut original: Request<Bytes> = Request::new(Bytes::from_static(b"original\x00\xff"));

        for index in 0..160 {
            original.headers_mut().append("x-value", index.into());
        }

        let pending: PendingRequest = PendingRequest::new(
            1,
            Local::now(),
            "http://localhost/".parse().unwrap(),
            original.clone(),
            reply,
        )
        .unwrap();

        let raw: Bytes = pending.record.raw.clone();

        // The original is valid even when it exceeds the editor parser's header limit.
        assert!(parse_request(&raw).is_err());

        let mut queue: RequestQueue = RequestQueue::default();
        queue.push(pending);

        queue.set_draft(1, b"GET /edited HTTP/1.1\r\n\r\n".to_vec());
        queue.set_draft(1, raw.to_vec());
        queue.resolve(0, true).unwrap();

        let forwarded: Request<Bytes> = receiver.try_recv().unwrap().unwrap();

        assert_eq!(forwarded.headers(), original.headers());
        assert_eq!(forwarded.body(), original.body());
        assert_eq!(forwarded.uri(), original.uri());
    }

    #[test]
    fn test_forward_all_keeps_invalid_drafts() {
        let mut queue: RequestQueue = RequestQueue::default();

        let (first, mut first_reply) = pending(10);
        let (second, mut second_reply) = pending(20);
        let (third, mut third_reply) = pending(30);

        for request in [first, second, third] {
            queue.push(request);
        }

        let absolute: &[u8] = b"GET http://other.host/ HTTP/1.1\r\n\r\n";
        let incomplete: &[u8] = b"GET / HTTP/1.1\r\n";
        assert!(queue.set_draft(10, absolute.to_vec()));
        assert!(queue.set_draft(30, incomplete.to_vec()));

        assert_eq!(queue.resolve_all(true), (1, 2));

        let ids: Vec<u64> = queue.iter().map(|request| request.record.id).collect();

        assert_eq!(ids, [10, 30]);
        assert_eq!(queue.get(0).unwrap().record.raw, absolute);
        assert_eq!(queue.get(1).unwrap().record.raw, incomplete);
        assert!(matches!(second_reply.try_recv(), Ok(Some(_))));
        assert!(matches!(
            first_reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            third_reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn test_drop_releases_pending_requests() {
        let mut queue: RequestQueue = RequestQueue::default();

        let (first, mut first_reply) = pending(1);
        let (second, mut second_reply) = pending(2);

        queue.push(first);
        queue.push(second);

        assert!(queue.set_draft(1, b"invalid draft".to_vec()));

        drop(queue);

        assert!(matches!(first_reply.try_recv(), Ok(None)));
        assert!(matches!(second_reply.try_recv(), Ok(None)));
    }
}
