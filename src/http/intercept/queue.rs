use std::mem;

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use indexmap::IndexMap;

use super::{HttpInterceptPendingRequest, HttpInterceptReply};
use crate::http::request::{HttpWireRequestError, format_request, parse_request};
use crate::http::target::{HttpTarget, HttpTargetError};

pub(crate) struct QueuedRequest {
    pending: HttpInterceptPendingRequest,
    raw: Vec<u8>,
    original: Vec<u8>,
}

impl QueuedRequest {
    fn new(pending: HttpInterceptPendingRequest) -> Result<Self, HttpWireRequestError> {
        let raw: Vec<u8> = format_request(&pending.request)?;

        Ok(Self {
            pending,
            original: raw.clone(),
            raw,
        })
    }

    #[inline]
    pub(crate) fn id(&self) -> u64 {
        self.pending.id
    }

    #[inline]
    pub(crate) fn timestamp(&self) -> &DateTime<Local> {
        &self.pending.timestamp
    }

    #[inline]
    pub(crate) fn original_uri(&self) -> &Uri {
        &self.pending.original_uri
    }

    #[inline]
    pub(crate) fn raw(&self) -> &[u8] {
        &self.raw
    }

    #[inline]
    pub(crate) fn target(&self) -> Result<HttpTarget, HttpTargetError> {
        let uri: &Uri = self.original_uri();
        HttpTarget::from_uri(uri)
    }

    fn forward_request(&self) -> Result<Request<Bytes>, HttpWireRequestError> {
        let request: Request<Bytes> = if self.raw == self.original {
            self.pending.request.clone()
        } else {
            parse_request(&self.raw)?
        };

        // The proxy owns the destination; edits may only change the origin-form request.
        if request.uri().scheme().is_some() || request.uri().authority().is_some() {
            return Err(HttpWireRequestError::ExpectedOriginForm);
        }

        Ok(request)
    }

    fn reply(&self, forward: bool) -> Result<HttpInterceptReply, HttpWireRequestError> {
        if forward {
            Ok(HttpInterceptReply::Forward(Box::new(
                self.forward_request()?,
            )))
        } else {
            Ok(HttpInterceptReply::Drop)
        }
    }
}

#[derive(Default)]
pub(crate) struct RequestQueue {
    // Handler-assigned IDs are unique
    //
    // Iteration follows arrival order, not ID order.
    requests: IndexMap<u64, QueuedRequest>,
}

impl RequestQueue {
    #[inline]
    pub(crate) fn push(
        &mut self,
        pending: HttpInterceptPendingRequest,
    ) -> Result<(), HttpWireRequestError> {
        self.requests
            .insert(pending.id, QueuedRequest::new(pending)?);
        Ok(())
    }

    /// Access a request by its current queue position
    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<&QueuedRequest> {
        self.requests.get_index(index).map(|(_, request)| request)
    }

    /// Find an ID's current queue position
    #[inline]
    pub(crate) fn index_of(&self, id: u64) -> Option<usize> {
        self.requests.get_index_of(&id)
    }

    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &QueuedRequest> {
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

    /// Replace the draft by ID.
    pub(crate) fn set_draft(&mut self, id: u64, raw: Vec<u8>) -> bool {
        let request: Option<&mut QueuedRequest> = self.requests.get_mut(&id);

        let Some(request) = request else {
            return false;
        };

        request.raw = raw;
        true
    }

    /// Resolve one position
    pub(crate) fn resolve(
        &mut self,
        index: usize,
        forward: bool,
    ) -> Result<(), HttpWireRequestError> {
        let Some(current) = self.get(index) else {
            return Ok(());
        };

        let reply: HttpInterceptReply = current.reply(forward)?;

        if let Some((_, current)) = self.requests.shift_remove_index(index) {
            let _ = current.pending.reply.send(reply);
        }

        Ok(())
    }

    /// Resolve a snapshot
    pub(crate) fn resolve_all(&mut self, forward: bool) -> (usize, usize) {
        let requests: IndexMap<u64, QueuedRequest> = mem::take(&mut self.requests);

        let count: usize = requests.len();

        let mut failed: usize = 0;

        // Consuming the map avoids repeated shifts; invalid drafts retain their relative order.
        for (id, current) in requests {
            let reply: HttpInterceptReply = match current.reply(forward) {
                Ok(reply) => reply,
                Err(_) => {
                    self.requests.insert(id, current);
                    failed += 1;
                    continue;
                }
            };

            let _ = current.pending.reply.send(reply);
        }

        (count - failed, failed)
    }
}

impl Drop for RequestQueue {
    fn drop(&mut self) {
        // Closing the UI or failing to draw must unblock every pending handler.
        self.resolve_all(false);
    }
}

#[cfg(test)]
mod tests {
    use chrono::Local;
    use tokio::sync::oneshot;

    use super::*;

    fn pending(
        id: u64,
    ) -> (
        HttpInterceptPendingRequest,
        oneshot::Receiver<HttpInterceptReply>,
    ) {
        let (reply, receiver) = oneshot::channel();
        let request: Request<Bytes> = Request::builder()
            .method("POST")
            .uri("/original")
            .header("host", "virtual.host")
            .body(Bytes::from_static(b"\xff\x00body\n\n"))
            .unwrap();

        (
            HttpInterceptPendingRequest {
                id,
                timestamp: Local::now(),
                original_uri: "http://127.0.0.1:8000/original".parse().unwrap(),
                request,
                reply,
            },
            receiver,
        )
    }

    #[test]
    fn test_ids_follow_queue_order() {
        let mut queue: RequestQueue = RequestQueue::default();
        for id in [30, 10, 20] {
            let (request, _reply) = pending(id);
            queue.push(request).unwrap();
        }

        assert_eq!(queue.index_of(10), Some(1));
        assert_eq!(queue.index_of(99), None);
        assert!(!queue.set_draft(99, Vec::new()));

        let draft: Vec<u8> = b"GET /edited HTTP/1.1\r\n\r\n".to_vec();
        assert!(queue.set_draft(10, draft.clone()));
        assert_eq!(queue.get(1).unwrap().raw(), draft);

        queue.resolve(0, false).unwrap();

        let ids: Vec<u64> = queue.iter().map(QueuedRequest::id).collect();
        assert_eq!(ids, [10, 20]);
        assert_eq!(queue.index_of(10), Some(0));
        assert_eq!(queue.index_of(30), None);
    }

    #[test]
    fn test_forward_preserves_binary_body() {
        let mut queue: RequestQueue = RequestQueue::default();
        let (request, mut reply) = pending(1);
        queue.push(request).unwrap();

        queue.resolve(0, true).unwrap();

        let HttpInterceptReply::Forward(request) = reply.try_recv().unwrap() else {
            panic!("expected a forwarded request");
        };
        assert_eq!(request.body(), b"\xff\x00body\n\n".as_slice());
        assert_eq!(request.uri(), "/original");
        assert!(queue.is_empty());
    }

    #[test]
    fn test_forward_uses_draft_and_original_destination() {
        let mut queue: RequestQueue = RequestQueue::default();
        let (request, mut reply) = pending(1);
        let original_uri: Uri = request.original_uri.clone();
        queue.push(request).unwrap();
        assert!(queue.set_draft(
            1,
            b"POST /edited HTTP/1.1\r\nHost: another.host\r\n\r\nnew body".to_vec()
        ));

        assert_eq!(queue.get(0).unwrap().original_uri(), &original_uri);
        queue.resolve(0, true).unwrap();

        let HttpInterceptReply::Forward(request) = reply.try_recv().unwrap() else {
            panic!("expected a forwarded draft");
        };
        assert_eq!(request.uri(), "/edited");
        assert_eq!(request.headers()["host"], "another.host");
        assert_eq!(request.body(), b"new body".as_slice());
    }

    #[test]
    fn test_forward_all_keeps_invalid_drafts() {
        let mut queue: RequestQueue = RequestQueue::default();
        let (first, mut first_reply) = pending(10);
        let (second, mut second_reply) = pending(20);
        let (third, mut third_reply) = pending(30);
        for request in [first, second, third] {
            queue.push(request).unwrap();
        }

        let absolute: &[u8] = b"GET http://other.host/ HTTP/1.1\r\n\r\n";
        let incomplete: &[u8] = b"GET / HTTP/1.1\r\n";
        assert!(queue.set_draft(10, absolute.to_vec()));
        assert!(queue.set_draft(30, incomplete.to_vec()));

        assert_eq!(queue.resolve_all(true), (1, 2));

        let ids: Vec<u64> = queue.iter().map(QueuedRequest::id).collect();
        assert_eq!(ids, [10, 30]);
        assert_eq!(queue.get(0).unwrap().raw(), absolute);
        assert_eq!(queue.get(1).unwrap().raw(), incomplete);
        assert!(matches!(
            second_reply.try_recv(),
            Ok(HttpInterceptReply::Forward(_))
        ));
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
        queue.push(first).unwrap();
        queue.push(second).unwrap();
        assert!(queue.set_draft(1, b"invalid draft".to_vec()));

        drop(queue);

        assert!(matches!(
            first_reply.try_recv(),
            Ok(HttpInterceptReply::Drop)
        ));
        assert!(matches!(
            second_reply.try_recv(),
            Ok(HttpInterceptReply::Drop)
        ));
    }
}
