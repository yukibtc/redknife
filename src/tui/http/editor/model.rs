use ratatui::style::{Modifier, Style};
use ratatui_textarea::TextArea;

use super::document::Document;
use super::send::SendResult;
use crate::tui::theme;

/// Exit text chosen by the caller for a standalone or embedded editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitLabel {
    /// Close a standalone repeater.
    Close,
    /// Return to the previous TUI.
    Back,
}

impl ExitLabel {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::Back => "Back",
        }
    }
}

/// Determines whether submitting returns a draft or sends an independent request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// Edit a pending request with a fixed destination.
    Edit,
    /// Allow changing the destination and sending repeated requests.
    Repeat,
}

impl Mode {
    #[inline]
    pub(super) fn is_repeat(&self) -> bool {
        matches!(self, Self::Repeat)
    }
}

/// The pane receiving text input or navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    /// Destination field
    Target,
    /// Request document
    Request,
    /// Read-only response document.
    Response,
}

impl Focus {
    #[inline]
    pub(super) fn is_target(&self) -> bool {
        matches!(self, Self::Target)
    }

    #[inline]
    pub(super) fn is_request(&self) -> bool {
        matches!(self, Self::Request)
    }

    #[inline]
    pub(super) fn is_response(&self) -> bool {
        matches!(self, Self::Response)
    }

    /// Follow the pane order used by Tab
    fn next(self, mode: Mode) -> Self {
        match mode {
            Mode::Edit => Self::Request,
            Mode::Repeat => match self {
                Self::Target => Self::Request,
                Self::Request => Self::Response,
                Self::Response => Self::Target,
            },
        }
    }

    /// Follow the reverse pane order used by Shift+Tab
    fn previous(self, mode: Mode) -> Self {
        match mode {
            Mode::Edit => Self::Request,
            Mode::Repeat => match self {
                Self::Target => Self::Response,
                Self::Request => Self::Target,
                Self::Response => Self::Request,
            },
        }
    }
}

/// All state read by the view and changed by the editor's update function.
pub(super) struct Model {
    mode: Mode,
    pub(super) exit_label: ExitLabel,
    pub(super) target: TextArea<'static>,
    pub(super) request: Document,
    pub(super) response: Option<Document>,
    focus: Focus,
    pub(super) hex: bool,
    /// Informational or error text displayed in the footer.
    pub(super) status: String,
    /// True between accepting a send and receiving its completion message.
    pub(super) sending: bool,
}

impl Model {
    /// Start with the request focused and no response or network operation.
    pub(super) fn new(mode: Mode, raw: Vec<u8>, target: &str) -> Self {
        let mut model: Self = Self {
            mode,
            exit_label: ExitLabel::Close,
            target: TextArea::from([target]),
            request: Document::new(raw),
            response: None,
            focus: Focus::Request,
            hex: false,
            status: String::new(),
            sending: false,
        };

        model.sync_focus();

        model
    }

    #[inline]
    pub(super) fn mode(&self) -> Mode {
        self.mode
    }

    #[inline]
    pub(super) fn focus(&self) -> Focus {
        self.focus
    }

    #[inline]
    pub(super) fn focus_next(&mut self) {
        self.focus = self.focus.next(self.mode);
    }

    #[inline]
    pub(super) fn focus_previous(&mut self) {
        self.focus = self.focus.previous(self.mode);
    }

    pub(super) fn send_finished(&mut self, result: SendResult) {
        self.sending = false;
        self.status = match result.error {
            Some(error) => format!("{error} · partial response: {} bytes", result.bytes.len()),
            None => format!("Received {} bytes", result.bytes.len()),
        };
        self.response = Some(Document::new(result.bytes));
    }

    /// Keep text-area cursor styles consistent with focus and edit permissions.
    pub(super) fn sync_focus(&mut self) {
        self.target.set_cursor_line_style(Style::default());
        self.target
            .set_cursor_style(if self.focus == Focus::Target {
                highlight()
            } else {
                Style::default()
            });

        let style: Style =
            if self.focus == Focus::Request && self.request.is_editable() && !self.hex {
                highlight()
            } else {
                Style::default()
            };

        self.request.set_cursor_style(style);
    }
}

#[inline]
fn highlight() -> Style {
    theme::selection().add_modifier(Modifier::BOLD)
}
