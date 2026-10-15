use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::send::SendResult;

pub(super) enum Message {
    /// Key press.
    Key(KeyEvent),
    /// Focus the next pane with Tab
    FocusNext,
    /// Focus the previous pane with Shift+Tab
    FocusPrevious,
    /// Switch the document display between raw text and read-only hex.
    ToggleView,
    /// Validate and apply
    Submit,
    /// Export the current request bytes, including unapplied edits, to a file.
    Save,
    /// Leave the editor
    Close,
    /// Cancel a send
    Interrupt,
    /// An HTTP send has ended, possibly with a partial response and an error.
    SendFinished(SendResult),
    /// Replace the footer message, for example with an export result.
    SetStatus(String),
}

impl Message {
    pub(super) fn from_key(key: KeyEvent) -> Self {
        match key.code {
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => Self::Close,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Self::Interrupt,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => Self::Save,
            KeyCode::Esc => Self::Close,
            KeyCode::F(2) => Self::ToggleView,
            KeyCode::F(5) => Self::Submit,
            KeyCode::Tab => Self::FocusNext,
            KeyCode::BackTab => Self::FocusPrevious,
            _ => Self::Key(key),
        }
    }
}
