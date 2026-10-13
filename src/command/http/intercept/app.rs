use std::borrow::Cow;
use std::collections::VecDeque;
use std::str;

use bytes::Bytes;
use crossterm::event::{KeyCode, KeyEvent};
use http::Request;
use ratatui::widgets::ListState;
use tui_textarea::{CursorMove, Scrolling, TextArea};

use super::messages::{InterceptReply, PendingRequest};
use crate::http::request::{HttpWireRequestError, format_request, parse_request};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestAction {
    Forward,
    Drop,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Queue,
    Editor,
}

impl Focus {
    #[inline]
    pub fn is_queue(&self) -> bool {
        matches!(self, Self::Queue)
    }

    #[inline]
    pub fn is_editor(&self) -> bool {
        matches!(self, Self::Editor)
    }
}

pub struct QueuedRequest {
    pub pending: PendingRequest,
    pub editor: TextArea<'static>,
    pub editable: bool,
    original_bytes: Vec<u8>,
    body_newline: &'static str,
}

impl QueuedRequest {
    fn new(pending: PendingRequest) -> Result<Self, HttpWireRequestError> {
        let raw: Vec<u8> = format_request(&pending.request)?;

        let text: Option<(Vec<String>, &'static str)> = editable_text(&raw);

        let editable: bool = text.is_some();

        let (lines, body_newline) = text.unwrap_or_else(|| (hex_lines(&raw), "\n"));

        Ok(Self {
            pending,
            editor: TextArea::new(lines),
            editable,
            original_bytes: raw,
            body_newline,
        })
    }

    pub fn input(&mut self, key: KeyEvent) {
        if self.editable {
            self.editor.input(key);
        } else {
            // Allow scrolling without allowing the hex view to become request data.
            let movement = match key.code {
                KeyCode::Up => CursorMove::Up,
                KeyCode::Down => CursorMove::Down,
                KeyCode::Left => CursorMove::Back,
                KeyCode::Right => CursorMove::Forward,
                KeyCode::Home => CursorMove::Head,
                KeyCode::End => CursorMove::End,
                KeyCode::PageUp => {
                    self.editor.scroll(Scrolling::PageUp);
                    return;
                }
                KeyCode::PageDown => {
                    self.editor.scroll(Scrolling::PageDown);
                    return;
                }
                _ => return,
            };
            self.editor.move_cursor(movement);
        }
    }

    /// Save the complete request or the current draft, never a lossy display.
    pub fn bytes(&self) -> Vec<u8> {
        if !self.editable {
            return self.original_bytes.clone();
        }

        let lines: &[String] = self.editor.lines();

        match lines.iter().position(String::is_empty) {
            Some(separator) => {
                let head = lines[..separator].join("\r\n");
                let body = lines[separator + 1..].join(self.body_newline);
                format!("{head}\r\n\r\n{body}").into_bytes()
            }
            None => lines.join("\r\n").into_bytes(),
        }
    }

    fn forward_request(&self) -> Result<Request<Bytes>, HttpWireRequestError> {
        let raw: Vec<u8> = self.bytes();

        let request: Request<Bytes> = if raw == self.original_bytes {
            self.pending.request.clone()
        } else {
            parse_request(&raw)?
        };

        if request.uri().scheme().is_some() || request.uri().authority().is_some() {
            return Err(HttpWireRequestError::ExpectedOriginForm);
        }

        Ok(request)
    }
}

/// Only text which can round-trip through TextArea may be edited.
fn editable_text(raw: &[u8]) -> Option<(Vec<String>, &'static str)> {
    let text: &str = str::from_utf8(raw).ok()?;

    if text.chars().any(|ch| {
        (ch.is_control() && !matches!(ch, '\r' | '\n' | '\t'))
            || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    }) {
        return None;
    }

    let (_, body) = text.split_once("\r\n\r\n")?;

    let body_newline: &str = if body.contains('\r') {
        let without_crlf: String = body.replace("\r\n", "");
        if without_crlf.contains(['\r', '\n']) {
            return None; // Mixed or bare CR line endings would lose information.
        }
        "\r\n"
    } else {
        "\n"
    };

    // split preserves the final empty line; str::lines would discard it.
    let normalized: String = text.replace("\r\n", "\n");

    Some((
        normalized.split('\n').map(str::to_owned).collect(),
        body_newline,
    ))
}

fn hex_lines(raw: &[u8]) -> Vec<String> {
    raw.chunks(16)
        .enumerate()
        .map(|(index, chunk)| format!("{:08x}  {chunk:02x?}", index * 16))
        .collect()
}

pub enum Status {
    WaitingForRequests,
    NoRequestSelected,
    ProxyChannelClosed,
    Resolved {
        id: u64,
        action: RequestAction,
    },
    ResolvedAll {
        action: RequestAction,
        count: usize,
        failed: usize,
        first_error: Option<String>,
    },
    RequestError {
        id: u64,
        error: String,
    },
    FileSaved {
        filename: String,
    },
    FileSaveError {
        error: String,
    },
}

impl Status {
    pub fn as_str(&self) -> Cow<'static, str> {
        match self {
            Self::WaitingForRequests => Cow::Borrowed("Waiting for requests..."),
            Self::NoRequestSelected => Cow::Borrowed("No request selected"),
            Self::ProxyChannelClosed => Cow::Borrowed("Proxy channel closed"),
            Self::Resolved { id, action } => Cow::Owned(format!("Request #{id}: {action:?}")),
            Self::ResolvedAll {
                action,
                count,
                failed,
                first_error,
            } => {
                let mut text =
                    format!("{action:?}: {count} requests, {failed} invalid requests kept");
                if let Some(error) = first_error {
                    text.push_str(&format!("; {error}"));
                }
                Cow::Owned(text)
            }
            Self::RequestError { id, error } => Cow::Owned(format!("Request #{id}: {error}")),
            Self::FileSaved { filename } => Cow::Owned(format!("Saved {filename}")),
            Self::FileSaveError { error } => Cow::Owned(format!("Save failed: {error}")),
        }
    }
}

pub struct App {
    pub queue: VecDeque<QueuedRequest>,
    pub selection: ListState,
    pub focus: Focus,
    pub status: Status,
    pub quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            selection: ListState::default(),
            focus: Focus::Queue,
            status: Status::WaitingForRequests,
            quit: false,
        }
    }

    pub fn push(&mut self, pending: PendingRequest) {
        let id: u64 = pending.id;

        match QueuedRequest::new(pending) {
            Ok(request) => self.queue.push_back(request),
            Err(error) => {
                self.status = Status::RequestError {
                    id,
                    error: error.to_string(),
                };
                return;
            }
        }

        if self.selection.selected().is_none() {
            self.selection.select(Some(0));
        }
    }

    pub fn current(&self) -> Option<&QueuedRequest> {
        self.queue.get(self.selection.selected()?)
    }

    pub fn current_mut(&mut self) -> Option<&mut QueuedRequest> {
        self.queue.get_mut(self.selection.selected()?)
    }

    pub fn next(&mut self) {
        if !self.queue.is_empty() {
            let index = self.selection.selected().unwrap_or(0);
            self.selection
                .select(Some((index + 1).min(self.queue.len() - 1)));
        }
    }

    pub fn previous(&mut self) {
        if !self.queue.is_empty() {
            let index = self.selection.selected().unwrap_or(0);
            self.selection.select(Some(index.saturating_sub(1)));
        }
    }

    fn repair_selection(&mut self) {
        let index = self.selection.selected().unwrap_or(0);
        self.selection.select(if self.queue.is_empty() {
            None
        } else {
            Some(index.min(self.queue.len() - 1))
        });
    }

    fn resolve_at(
        &mut self,
        index: usize,
        action: RequestAction,
    ) -> Result<u64, HttpWireRequestError> {
        let reply: InterceptReply = match action {
            RequestAction::Forward => {
                InterceptReply::Forward(Box::new(self.queue[index].forward_request()?))
            }
            RequestAction::Drop => InterceptReply::Drop,
        };

        let request: QueuedRequest = self.queue.remove(index).expect("existing queue index");
        let id: u64 = request.pending.id;

        let _ = request.pending.reply.send(reply);

        Ok(id)
    }

    pub fn resolve_selected(&mut self, action: RequestAction) -> Option<u64> {
        let Some(index) = self.selection.selected().filter(|&i| i < self.queue.len()) else {
            self.status = Status::NoRequestSelected;
            return None;
        };

        match self.resolve_at(index, action) {
            Ok(id) => {
                self.repair_selection();
                self.status = Status::Resolved { id, action };
                Some(id)
            }
            Err(error) => {
                self.status = Status::RequestError {
                    id: self.queue[index].pending.id,
                    error: error.to_string(),
                };
                None
            }
        }
    }

    pub fn resolve_all(&mut self, action: RequestAction) -> usize {
        let mut count: usize = 0;
        let mut index: usize = 0;

        let mut first_error: Option<String> = None;

        while index < self.queue.len() {
            match self.resolve_at(index, action) {
                Ok(_) => count += 1,
                Err(error) => {
                    first_error.get_or_insert_with(|| {
                        format!("Request #{}: {error}", self.queue[index].pending.id)
                    });
                    index += 1;
                }
            }
        }

        self.repair_selection();

        self.status = Status::ResolvedAll {
            action,
            count,
            failed: index,
            first_error,
        };

        count
    }

    pub fn quit(&mut self) {
        self.resolve_all(RequestAction::Drop);
        self.quit = true;
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use chrono::Local;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use http::Request;
    use http::header::CONTENT_LENGTH;
    use tokio::sync::oneshot;
    use tui_textarea::TextArea;

    use super::{App, QueuedRequest, RequestAction, Status};
    use crate::command::http::intercept::messages::{InterceptReply, PendingRequest};
    use crate::http::request::HttpWireRequestError;

    fn pending_request(
        id: u64,
        body: &'static [u8],
    ) -> (PendingRequest, oneshot::Receiver<InterceptReply>) {
        let (reply, receiver) = oneshot::channel();
        let request = Request::builder()
            .method("POST")
            .uri(format!("/{id}"))
            .header(CONTENT_LENGTH, body.len())
            .body(Bytes::from_static(body))
            .expect("test request must be valid");
        (
            PendingRequest {
                id,
                timestamp: Local::now(),
                original_uri: format!("http://example.com/{id}").parse().unwrap(),
                request,
                reply,
            },
            receiver,
        )
    }

    #[test]
    fn empty_body_remains_empty_after_editing_the_request() {
        let (pending, _replies) = pending_request(1, b"");
        let mut request = QueuedRequest::new(pending).expect("request must be queued");
        let original_bytes = request.bytes();

        request.editor = TextArea::from(["PATCH /1 HTTP/1.1", "content-length: 0", "", ""]);
        let edited_bytes = request.bytes();
        let forwarded = request
            .forward_request()
            .expect("edited request must parse");

        assert!(request.editable);
        assert_eq!(
            original_bytes,
            b"POST /1 HTTP/1.1\r\ncontent-length: 0\r\n\r\n"
        );
        assert_eq!(
            edited_bytes,
            b"PATCH /1 HTTP/1.1\r\ncontent-length: 0\r\n\r\n"
        );
        assert_eq!(forwarded.method(), "PATCH");
        assert_eq!(forwarded.body(), b"".as_slice());
    }

    #[test]
    fn lf_body_keeps_trailing_blank_lines_when_saved_and_forwarded() {
        let (pending, _replies) = pending_request(1, b"first\nlast\n\n");
        let mut request = QueuedRequest::new(pending).expect("request must be queued");
        let original_bytes = request.bytes();

        request.editor = TextArea::from([
            "PATCH /1 HTTP/1.1",
            "content-length: 12",
            "",
            "first",
            "last",
            "",
            "",
        ]);
        let edited_bytes = request.bytes();
        let forwarded = request
            .forward_request()
            .expect("edited request must parse");

        assert!(request.editable);
        assert_eq!(
            original_bytes,
            b"POST /1 HTTP/1.1\r\ncontent-length: 12\r\n\r\nfirst\nlast\n\n"
        );
        assert_eq!(
            edited_bytes,
            b"PATCH /1 HTTP/1.1\r\ncontent-length: 12\r\n\r\nfirst\nlast\n\n"
        );
        assert_eq!(forwarded.method(), "PATCH");
        assert_eq!(forwarded.body(), b"first\nlast\n\n".as_slice());
    }

    #[test]
    fn crlf_body_keeps_trailing_blank_lines_when_saved_and_forwarded() {
        let (pending, _replies) = pending_request(1, b"first\r\nlast\r\n\r\n");
        let mut request = QueuedRequest::new(pending).expect("request must be queued");
        let original_bytes = request.bytes();

        request.editor = TextArea::from([
            "PATCH /1 HTTP/1.1",
            "content-length: 15",
            "",
            "first",
            "last",
            "",
            "",
        ]);
        let edited_bytes = request.bytes();
        let forwarded = request
            .forward_request()
            .expect("edited request must parse");

        assert!(request.editable);
        assert_eq!(
            original_bytes,
            b"POST /1 HTTP/1.1\r\ncontent-length: 15\r\n\r\nfirst\r\nlast\r\n\r\n"
        );
        assert_eq!(
            edited_bytes,
            b"PATCH /1 HTTP/1.1\r\ncontent-length: 15\r\n\r\nfirst\r\nlast\r\n\r\n"
        );
        assert_eq!(forwarded.method(), "PATCH");
        assert_eq!(forwarded.body(), b"first\r\nlast\r\n\r\n".as_slice());
    }

    #[test]
    fn binary_body_is_read_only_and_preserved_when_saved_and_forwarded() {
        let (pending, _replies) = pending_request(1, b"\x00\xff\x1b[31m");
        let mut request = QueuedRequest::new(pending).expect("request must be queued");
        let original_display = request.editor.lines().to_vec();

        request.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        let saved_bytes = request.bytes();
        let forwarded = request
            .forward_request()
            .expect("binary request must forward");

        assert!(!request.editable);
        assert_eq!(request.editor.lines(), original_display);
        assert!(
            original_display
                .iter()
                .all(|line| line.chars().all(|ch| !ch.is_control())),
            "hex display must contain no terminal control characters"
        );
        assert_eq!(
            saved_bytes,
            b"POST /1 HTTP/1.1\r\ncontent-length: 7\r\n\r\n\x00\xff\x1b[31m"
        );
        assert_eq!(forwarded.body(), b"\x00\xff\x1b[31m".as_slice());
    }

    #[test]
    fn terminal_control_sequences_are_read_only_and_preserved_in_request_bytes() {
        let (pending, _replies) = pending_request(1, b"valid utf8\x1b]52;c;payload\x07");
        let mut request = QueuedRequest::new(pending).expect("request must be queued");
        let original_display = request.editor.lines().to_vec();

        request.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        let saved_bytes = request.bytes();
        let forwarded = request
            .forward_request()
            .expect("request containing control bytes must forward");

        assert!(!request.editable);
        assert_eq!(request.editor.lines(), original_display);
        assert!(
            original_display
                .iter()
                .all(|line| line.chars().all(|ch| !ch.is_control())),
            "terminal control sequences must not reach the text display"
        );
        assert_eq!(
            saved_bytes,
            b"POST /1 HTTP/1.1\r\ncontent-length: 25\r\n\r\nvalid utf8\x1b]52;c;payload\x07"
        );
        assert_eq!(
            forwarded.body(),
            b"valid utf8\x1b]52;c;payload\x07".as_slice()
        );
    }

    #[test]
    fn mixed_body_newlines_are_read_only_and_preserved_in_request_bytes() {
        let (pending, _replies) = pending_request(1, b"mixed\r\nnewlines\n");
        let mut request = QueuedRequest::new(pending).expect("request must be queued");
        let original_display = request.editor.lines().to_vec();

        request.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        let saved_bytes = request.bytes();
        let forwarded = request
            .forward_request()
            .expect("request with mixed newlines must forward");

        assert!(!request.editable);
        assert_eq!(request.editor.lines(), original_display);
        assert!(
            original_display
                .iter()
                .all(|line| line.chars().all(|ch| !ch.is_control())),
            "hex display must contain no terminal control characters"
        );
        assert_eq!(
            saved_bytes,
            b"POST /1 HTTP/1.1\r\ncontent-length: 16\r\n\r\nmixed\r\nnewlines\n"
        );
        assert_eq!(forwarded.body(), b"mixed\r\nnewlines\n".as_slice());
    }

    #[test]
    fn forward_all_keeps_invalid_requests_and_forwards_edits() {
        let mut app = App::new();
        let (edited_pending, mut edited_replies) = pending_request(1, b"ping");
        let (invalid_pending, mut invalid_replies) = pending_request(2, b"ping");
        let (unchanged_pending, mut unchanged_replies) = pending_request(3, b"ping");
        app.push(edited_pending);
        app.push(invalid_pending);
        app.push(unchanged_pending);
        app.queue[0].editor = TextArea::from([
            "PATCH /edited HTTP/1.1",
            "Content-Length: 4",
            "",
            "longer body",
            "",
        ]);
        app.queue[1].editor = TextArea::from(["GET /relative HTTP/1.1", "invalid header", "", ""]);

        let forwarded_count = app.resolve_all(RequestAction::Forward);

        assert_eq!(forwarded_count, 2);
        assert_eq!(app.queue.len(), 1);
        let remaining = app.current().expect("invalid request must remain selected");
        assert_eq!(remaining.pending.id, 2);
        let Status::ResolvedAll {
            action,
            count,
            failed,
            first_error,
        } = &app.status
        else {
            panic!("Forward All must report how many requests were resolved");
        };
        assert_eq!(*action, RequestAction::Forward);
        assert_eq!(*count, 2);
        assert_eq!(*failed, 1);
        assert!(
            first_error
                .as_ref()
                .expect("invalid request must produce an error")
                .starts_with("Request #2:")
        );

        let InterceptReply::Forward(edited_request) = edited_replies
            .try_recv()
            .expect("edited request must receive a decision")
        else {
            panic!("valid edited request must receive Forward");
        };
        assert_eq!(edited_request.method(), "PATCH");
        assert_eq!(edited_request.uri(), "/edited");
        assert_eq!(edited_request.body(), b"longer body\n".as_slice());
        // App preserves the draft's stale length; InterceptHandler normalizes it.
        assert_eq!(edited_request.headers()[CONTENT_LENGTH], "4");

        assert!(
            matches!(
                invalid_replies.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "invalid request must keep waiting for a decision"
        );

        let InterceptReply::Forward(unchanged_request) = unchanged_replies
            .try_recv()
            .expect("unchanged request must receive a decision")
        else {
            panic!("unchanged request must receive Forward");
        };
        assert_eq!(unchanged_request.method(), "POST");
        assert_eq!(unchanged_request.uri(), "/3");
        assert_eq!(unchanged_request.headers()[CONTENT_LENGTH], "4");
        assert_eq!(unchanged_request.body(), b"ping".as_slice());
    }

    #[test]
    fn quitting_drops_all_pending_requests_including_invalid_drafts() {
        let mut app = App::new();
        let (valid_pending, mut valid_replies) = pending_request(1, b"ping");
        let (invalid_pending, mut invalid_replies) = pending_request(2, b"ping");
        app.push(valid_pending);
        app.push(invalid_pending);
        app.queue[1].editor = TextArea::from(["GET /invalid HTTP/1.1"]);

        app.quit();

        assert!(app.quit);
        assert!(app.queue.is_empty());
        assert_eq!(app.selection.selected(), None);
        assert!(matches!(
            valid_replies
                .try_recv()
                .expect("quitting must resolve the valid request"),
            InterceptReply::Drop
        ));
        assert!(matches!(
            invalid_replies
                .try_recv()
                .expect("quitting must also resolve the invalid draft"),
            InterceptReply::Drop
        ));
    }

    #[test]
    fn forwarding_an_invalid_selected_request_keeps_it_pending() {
        let mut app = App::new();
        let (pending, mut replies) = pending_request(1, b"");
        app.push(pending);
        app.current_mut()
            .expect("queued request must be selected")
            .editor = TextArea::from(["GET /invalid HTTP/1.1"]);

        let resolved_id = app.resolve_selected(RequestAction::Forward);

        assert_eq!(resolved_id, None);
        assert_eq!(app.queue.len(), 1);
        assert_eq!(
            app.current()
                .expect("invalid request must remain selected")
                .pending
                .id,
            1
        );
        assert!(matches!(app.status, Status::RequestError { id: 1, .. }));
        assert!(
            matches!(replies.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
            "invalid request must keep waiting for a decision"
        );
    }

    fn app_with_draft(request_line: &str) -> (App, oneshot::Receiver<InterceptReply>) {
        let (pending, replies) = pending_request(1, b"");
        let mut app = App::new();
        app.push(pending);
        app.current_mut().unwrap().editor = TextArea::from([request_line, "", ""]);
        (app, replies)
    }

    #[test]
    fn absolute_url_is_rejected_even_when_it_matches_the_original_destination() {
        let (app, _replies) = app_with_draft("GET http://example.com/1 HTTP/1.1");

        let result = app.current().unwrap().forward_request();

        assert!(matches!(
            result,
            Err(HttpWireRequestError::ExpectedOriginForm)
        ));
    }

    #[test]
    fn absolute_url_cannot_redirect_to_another_http_destination() {
        let (app, _replies) = app_with_draft("GET http://elsewhere.example/edited HTTP/1.1");

        let result = app.current().unwrap().forward_request();

        assert!(matches!(
            result,
            Err(HttpWireRequestError::ExpectedOriginForm)
        ));
    }

    #[test]
    fn absolute_url_cannot_redirect_to_an_https_destination() {
        let (app, _replies) = app_with_draft("GET https://elsewhere.example/edited HTTP/1.1");

        let result = app.current().unwrap().forward_request();

        assert!(matches!(
            result,
            Err(HttpWireRequestError::ExpectedOriginForm)
        ));
    }

    #[test]
    fn authority_without_a_scheme_is_rejected() {
        let (app, _replies) = app_with_draft("GET elsewhere.example:8080 HTTP/1.1");

        let result = app.current().unwrap().forward_request();

        assert!(matches!(
            result,
            Err(HttpWireRequestError::ExpectedOriginForm)
        ));
    }

    #[test]
    fn forwarding_an_absolute_url_shows_an_error_and_keeps_the_draft() {
        let (mut app, mut replies) = app_with_draft("GET http://elsewhere.example/edited HTTP/1.1");
        let original_url = app.current().unwrap().pending.original_uri.clone();
        let draft = app.current().unwrap().bytes();

        let resolved = app.resolve_selected(RequestAction::Forward);

        assert_eq!(resolved, None);
        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.selection.selected(), Some(0));
        assert_eq!(app.current().unwrap().pending.original_uri, original_url);
        assert_eq!(app.current().unwrap().bytes(), draft);
        assert!(matches!(
            replies.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        let Status::RequestError { id, error } = &app.status else {
            panic!("Forward should report a request error");
        };
        assert_eq!(*id, 1);
        assert_eq!(
            error,
            "Expected a path such as /login?q=1; interceptor destination is fixed"
        );
    }

    #[test]
    fn forward_all_leaves_an_absolute_url_pending() {
        let (mut app, mut replies) = app_with_draft("GET http://elsewhere.example/edited HTTP/1.1");
        let draft = app.current().unwrap().bytes();

        let forwarded = app.resolve_all(RequestAction::Forward);

        assert_eq!(forwarded, 0);
        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.current().unwrap().bytes(), draft);
        assert!(matches!(
            app.status,
            Status::ResolvedAll {
                count: 0,
                failed: 1,
                ..
            }
        ));
        assert!(matches!(
            replies.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn an_absolute_url_draft_can_still_be_dropped() {
        let (mut app, mut replies) = app_with_draft("GET http://elsewhere.example/edited HTTP/1.1");

        let resolved = app.resolve_selected(RequestAction::Drop);

        assert_eq!(resolved, Some(1));
        assert!(app.queue.is_empty());
        assert!(matches!(replies.try_recv().unwrap(), InterceptReply::Drop));
    }

    #[test]
    fn options_asterisk_can_be_forwarded() {
        let (mut app, mut replies) = app_with_draft("OPTIONS * HTTP/1.1");

        let resolved = app.resolve_selected(RequestAction::Forward);

        assert_eq!(resolved, Some(1));
        let InterceptReply::Forward(request) = replies.try_recv().unwrap() else {
            panic!("OPTIONS * should be forwarded");
        };
        assert_eq!(request.method(), "OPTIONS");
        assert_eq!(request.uri(), "*");
    }

    #[test]
    fn an_invalid_draft_can_be_serialized_without_parsing_it() {
        let (pending, _replies) = pending_request(1, b"");
        let mut request = QueuedRequest::new(pending).expect("request must be queued");
        // No blank line: this is an incomplete HTTP request, but a savable draft.
        request.editor = TextArea::from(["GET /invalid HTTP/1.1"]);

        let saved_bytes = request.bytes();

        assert_eq!(saved_bytes, b"GET /invalid HTTP/1.1");
    }

    #[test]
    fn dropping_an_invalid_selected_request_removes_it_from_the_queue() {
        let mut app = App::new();
        let (pending, mut replies) = pending_request(1, b"");
        app.push(pending);
        app.current_mut()
            .expect("queued request must be selected")
            .editor = TextArea::from(["GET /invalid HTTP/1.1"]);

        let resolved_id = app.resolve_selected(RequestAction::Drop);

        assert_eq!(resolved_id, Some(1));
        assert!(app.queue.is_empty());
        assert_eq!(app.selection.selected(), None);
        assert!(matches!(
            replies.try_recv().expect("Drop must send a decision"),
            InterceptReply::Drop
        ));
    }
}
