use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) enum Message {
    /// Key press.
    Key(KeyEvent),
    /// Move focus between the request list and preview.
    SwitchFocus,
    /// Alternate the pending queue and history in the request panel.
    SwitchList,
    /// Return focus to the request list without resolving a request.
    FocusQueue,
    /// Forward the selected request with its current draft.
    Forward,
    /// Drop the selected request without forwarding it.
    Drop,
    /// Forward the displayed queue and the requests already waiting in the channel.
    ForwardAll,
    /// Drop the displayed queue and the requests already waiting in the channel.
    DropAll,
    /// Enable or disable interception for this proxy session.
    ToggleIntercept,
    /// Open EDIT for the selected request, keeping its destination fixed.
    Edit,
    /// Open REPEAT with an independent copy of the selected request.
    Repeat,
    /// Export the selected request's current draft to a file.
    Save,
    /// Leave the proxy
    Quit,
    /// Reconcile selection and preview after the runner changes the queue.
    QueueChanged {
        /// Selection before the change
        selected_id: Option<u64>,
        /// New footer text, or `None` to preserve the current message.
        status: Option<String>,
    },
    /// Apply a draft returned from EDIT to the pending request with this ID.
    Edited { id: u64, raw: Vec<u8> },
    /// Replace the footer message, for example with an export result or an error.
    SetStatus(String),
}

impl Message {
    pub(super) fn from_key(key: KeyEvent) -> Self {
        match key.code {
            KeyCode::Char('q' | 'c') if key.modifiers.contains(KeyModifiers::CONTROL) => Self::Quit,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => Self::Save,
            KeyCode::F(2) => Self::SwitchList,
            KeyCode::F(5) => Self::Forward,
            KeyCode::F(6) => Self::Drop,
            KeyCode::F(7) => Self::ForwardAll,
            KeyCode::F(8) => Self::DropAll,
            KeyCode::F(9) => Self::ToggleIntercept,
            KeyCode::Enter => Self::Edit,
            KeyCode::Char('r') => Self::Repeat,
            KeyCode::Tab | KeyCode::BackTab => Self::SwitchFocus,
            KeyCode::Esc => Self::FocusQueue,
            _ => Self::Key(key),
        }
    }
}
