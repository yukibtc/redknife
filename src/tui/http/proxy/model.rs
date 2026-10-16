use std::net::SocketAddr;
use std::sync::Arc;

use ratatui::widgets::{List, ListState};

use crate::http::proxy::{ProxyState, RequestQueue, RequestRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Queue,
    Preview,
}

impl Focus {
    #[inline]
    pub(super) fn is_queue(&self) -> bool {
        matches!(self, Self::Queue)
    }

    #[inline]
    pub(super) fn is_preview(&self) -> bool {
        matches!(self, Self::Preview)
    }

    pub(super) fn switch(&self) -> Self {
        match self {
            Self::Queue => Self::Preview,
            Self::Preview => Self::Queue,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestList {
    Pending,
    History,
}

impl RequestList {
    #[inline]
    pub(super) fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    #[inline]
    pub(super) fn is_history(&self) -> bool {
        matches!(self, Self::History)
    }

    #[inline]
    pub(super) fn switch(&self) -> Self {
        match self {
            Self::Pending => Self::History,
            Self::History => Self::Pending,
        }
    }
}

pub(super) struct Model {
    pub(super) state: Arc<ProxyState>,
    pub(super) list: RequestList,
    pub(super) history: Vec<Arc<RequestRecord>>,
    pub(super) history_state: ListState,
    pub(super) queue: RequestQueue,
    pub(super) queue_state: ListState,
    pub(super) focus: Focus,
    /// Request preview lines, prepared only when the selection or its draft changes.
    pub(super) preview: List<'static>,
    pub(super) preview_state: ListState,
    /// Saves the selected request ID and preview scroll position when switching
    /// between different lists (history, pending). Restores the position when switching back,
    /// provided the same request is still selected.
    pub(super) inactive_preview: Option<(u64, ListState)>,
    /// Visible item counts recorded by the view for PageUp and PageDown.
    pub(super) queue_page_size: u16,
    pub(super) preview_page_size: u16,
    pub(super) address: SocketAddr,
    /// Informational or error text displayed in the footer.
    pub(super) status: String,
}

impl Model {
    pub(super) fn new(address: SocketAddr, state: Arc<ProxyState>) -> Self {
        Self {
            state,
            list: RequestList::History,
            history: Vec::new(),
            history_state: ListState::default(),
            queue: RequestQueue::default(),
            queue_state: ListState::default(),
            focus: Focus::Queue,
            preview: List::default(),
            preview_state: ListState::default(),
            inactive_preview: None,
            queue_page_size: 1,
            preview_page_size: 1,
            address,
            status: "Waiting for HTTP requests".into(),
        }
    }

    pub(super) fn current(&self) -> Option<&RequestRecord> {
        match self.list {
            RequestList::Pending => self
                .queue_state
                .selected()
                .and_then(|index| self.queue.get(index))
                .map(|request| request.record()),
            RequestList::History => self
                .history_state
                .selected()
                .and_then(|index| self.history.get(index))
                .map(|entry| entry.as_ref()),
        }
    }

    #[inline]
    pub(super) fn list_state(&mut self) -> &mut ListState {
        match self.list {
            RequestList::Pending => &mut self.queue_state,
            RequestList::History => &mut self.history_state,
        }
    }

    pub(super) fn select_queue(&mut self, index: usize) {
        let selected: Option<usize> = if self.queue.is_empty() {
            None
        } else {
            Some(index.min(self.queue.len() - 1))
        };

        self.queue_state.select(selected);
    }

    pub(super) fn select(&mut self, index: usize) {
        match self.list {
            RequestList::Pending => self.select_queue(index),
            RequestList::History => {
                let selected: Option<usize> = if self.history.is_empty() {
                    None
                } else {
                    Some(index.min(self.history.len() - 1))
                };
                self.history_state.select(selected);
            }
        }
    }

    /// Returns `true` if the history has been updated.
    pub(super) fn sync_history(&mut self) -> bool {
        let Some(history) = self.state.history_since(self.history.len()) else {
            return false;
        };

        let selected_id: Option<u64> = self
            .history_state
            .selected()
            .and_then(|index| self.history.get(index))
            .map(|entry| entry.id);

        let selected: Option<usize> = selected_id
            .and_then(|id| history.iter().position(|entry| entry.id == id))
            .or_else(|| (!history.is_empty()).then_some(0));

        self.history = history;

        self.history_state.select(selected);

        if self.list.is_history() && self.current().map(|request| request.id) != selected_id {
            self.refresh_preview();
        }

        true
    }

    /// Replace the cached preview when its source changes, starting at the first line.
    pub(super) fn refresh_preview(&mut self) {
        let lines: Vec<String> = match self.current() {
            Some(request) => request
                .raw
                .split(|byte| *byte == b'\n')
                .map(|line| {
                    let line: &[u8] = line.strip_suffix(b"\r").unwrap_or(line);
                    escaped_line(line)
                })
                .collect(),
            None => Vec::new(),
        };
        let selected: Option<usize> = if lines.is_empty() { None } else { Some(0) };

        self.preview = List::new(lines);
        self.preview_state = ListState::default().with_selected(selected);
    }
}

#[inline]
pub(super) fn escaped_line(raw: &[u8]) -> String {
    let mut line: String = String::new();

    for &byte in raw {
        match byte {
            b'\t' => line.push_str("    "),
            b' '..=b'~' => line.push(char::from(byte)),
            _ => line.extend(byte.escape_ascii().map(char::from)),
        }
    }

    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_escapes_control_and_binary_bytes() {
        assert_eq!(escaped_line(b"hello\x1b\x00\xff"), "hello\\x1b\\x00\\xff");
    }

    #[test]
    fn test_preview_preserves_json_text() {
        assert_eq!(
            escaped_line(b"\t\"username\": \"test\""),
            "    \"username\": \"test\""
        );
        assert_eq!(
            escaped_line(br#"{"value": "it's \t"}"#),
            r#"{"value": "it's \t"}"#
        );
    }
}
