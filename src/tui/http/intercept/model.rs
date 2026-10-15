use std::net::SocketAddr;

use ratatui::widgets::{List, ListState};

use crate::http::intercept::{QueuedRequest, RequestQueue};

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

pub(super) struct Model {
    pub(super) queue: RequestQueue,
    pub(super) queue_state: ListState,
    pub(super) focus: Focus,
    /// Escaped request lines, prepared only when the selection or its draft changes.
    pub(super) preview: List<'static>,
    pub(super) preview_state: ListState,
    /// Visible item counts recorded by the view for PageUp and PageDown.
    pub(super) queue_page_size: u16,
    pub(super) preview_page_size: u16,
    pub(super) address: SocketAddr,
    /// Informational or error text displayed in the footer.
    pub(super) status: String,
}

impl Model {
    pub(super) fn new(address: SocketAddr) -> Self {
        Self {
            queue: RequestQueue::default(),
            queue_state: ListState::default(),
            focus: Focus::Queue,
            preview: List::default(),
            preview_state: ListState::default(),
            queue_page_size: 1,
            preview_page_size: 1,
            address,
            status: "Waiting for HTTP requests".into(),
        }
    }

    #[inline]
    pub(super) fn current(&self) -> Option<&QueuedRequest> {
        self.queue_state
            .selected()
            .and_then(|index| self.queue.get(index))
    }

    pub(super) fn select(&mut self, index: usize) {
        let selected: Option<usize> = if self.queue.is_empty() {
            None
        } else {
            Some(index.min(self.queue.len() - 1))
        };

        self.queue_state.select(selected);
    }

    /// Replace the cached preview when its source changes, starting at the first line.
    pub(super) fn refresh_preview(&mut self) {
        let lines: Vec<String> = match self.current() {
            Some(request) => request
                .raw()
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
    raw.iter()
        .flat_map(|byte| byte.escape_ascii())
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_escapes_control_and_binary_bytes() {
        assert_eq!(escaped_line(b"hello\x1b\x00\xff"), "hello\\x1b\\x00\\xff");
    }
}
