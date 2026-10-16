use crossterm::event::KeyCode;
use ratatui::widgets::ListState;

use super::message::Message;
use super::model::{Focus, Model};

pub(super) enum Effect {
    /// Forward the selected request, including any draft edits.
    Forward,
    /// Drop the selected request.
    Drop,
    /// Forward the queue and the current snapshot of waiting arrivals.
    ForwardAll,
    /// Drop the queue and the current snapshot of waiting arrivals.
    DropAll,
    /// Return to the caller to open EDIT for the selected request.
    Edit,
    /// Return to the caller to open REPEAT with a copy of the selected request.
    Repeat,
    /// Export the selected request's draft.
    Save,
    /// Return to the caller to shut down the interceptor.
    Quit,
}

impl Effect {
    #[inline]
    pub(super) fn is_forward(&self) -> bool {
        matches!(self, Self::Forward)
    }

    #[inline]
    pub(super) fn is_forward_all(&self) -> bool {
        matches!(self, Self::ForwardAll)
    }

    #[inline]
    pub(super) fn is_edit(&self) -> bool {
        matches!(self, Self::Edit)
    }
}

pub(super) fn update(model: &mut Model, message: Message) -> Option<Effect> {
    match message {
        Message::Forward => return Some(Effect::Forward),
        Message::Drop => return Some(Effect::Drop),
        Message::ForwardAll => return Some(Effect::ForwardAll),
        Message::DropAll => return Some(Effect::DropAll),
        Message::Edit => return Some(Effect::Edit),
        Message::Repeat => return Some(Effect::Repeat),
        Message::Save => return Some(Effect::Save),
        Message::Quit => return Some(Effect::Quit),
        Message::SwitchFocus => {
            model.focus = model.focus.switch();
        }
        Message::FocusQueue => model.focus = Focus::Queue,
        Message::SetStatus(status) => model.status = status,
        Message::QueueChanged {
            selected_id,
            status,
        } => {
            // Keep the same request by ID
            let selected: usize = selected_id
                .and_then(|id| model.queue.index_of(id))
                .unwrap_or(model.queue_state.selected().unwrap_or(0));

            model.select(selected);

            if model.current().map(|request| request.id()) != selected_id {
                model.refresh_preview();
            }

            if let Some(status) = status {
                model.status = status;
            }
        }
        Message::Edited { id, raw } => {
            if model.queue.set_draft(id, raw) {
                if model.current().is_some_and(|request| request.id() == id) {
                    model.refresh_preview();
                }

                model.status = format!("Request #{id} updated; awaiting a decision");
            } else {
                model.status = format!("Request #{id} is no longer pending");
            }
        }
        Message::Key(key) => match model.focus {
            Focus::Queue => {
                let previous: Option<usize> = model.queue_state.selected();

                navigate(&mut model.queue_state, key.code, model.queue_page_size);

                model.select(model.queue_state.selected().unwrap_or(0));

                if model.queue_state.selected() != previous {
                    model.refresh_preview();
                }
            }
            Focus::Preview => navigate(&mut model.preview_state, key.code, model.preview_page_size),
        },
    }

    None
}

fn navigate(state: &mut ListState, key: KeyCode, page_size: u16) {
    match key {
        KeyCode::Up => state.select_previous(),
        KeyCode::Down => state.select_next(),
        KeyCode::Home => state.select_first(),
        KeyCode::End => state.select_last(),
        KeyCode::PageUp => state.scroll_up_by(page_size),
        KeyCode::PageDown => state.scroll_down_by(page_size),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use chrono::Local;
    use http::Request;
    use tokio::sync::oneshot;

    use super::*;
    use crate::http::intercept::HttpInterceptPendingRequest;

    fn model() -> Model {
        let mut model: Model = Model::new("127.0.0.1:8001".parse().unwrap());
        for id in [10, 20, 30] {
            let (reply, _receiver) = oneshot::channel();
            let request: HttpInterceptPendingRequest = HttpInterceptPendingRequest {
                id,
                timestamp: Local::now(),
                original_uri: "http://localhost/".parse().unwrap(),
                request: Request::new(Bytes::new()),
                reply,
            };
            model.queue.push(request).unwrap();
        }
        model.select(1);
        model.refresh_preview();

        model
    }

    #[test]
    fn test_queue_changes_preserve_selection_by_id() {
        let mut model: Model = model();
        model.queue.resolve(0, false).unwrap();

        update(
            &mut model,
            Message::QueueChanged {
                selected_id: Some(20),
                status: None,
            },
        );

        assert_eq!(model.current().unwrap().id(), 20);
        assert_eq!(model.queue_state.selected(), Some(0));

        model.queue.resolve(0, false).unwrap();
        update(
            &mut model,
            Message::QueueChanged {
                selected_id: Some(20),
                status: None,
            },
        );

        assert_eq!(model.current().unwrap().id(), 30);

        model.queue.resolve_all(false);
        update(
            &mut model,
            Message::QueueChanged {
                selected_id: Some(30),
                status: None,
            },
        );

        assert!(model.current().is_none());
    }

    #[test]
    fn test_edit_updates_the_request_by_id() {
        let mut model: Model = model();
        let raw: &[u8] = b"GET /edited HTTP/1.1\r\n\r\n";

        update(
            &mut model,
            Message::Edited {
                id: 10,
                raw: raw.to_vec(),
            },
        );

        assert_eq!(model.queue.get(0).unwrap().raw(), raw);
        assert_eq!(model.current().unwrap().id(), 20);
        assert_eq!(model.queue.len(), 3);
    }
}
