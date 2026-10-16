use std::mem;

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
    /// Toggle interception and resolve waiting requests when switching OFF.
    ToggleIntercept,
    /// Return to the caller to shut down the proxy.
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
    if model.list.is_history()
        && matches!(
            message,
            Message::Forward
                | Message::Drop
                | Message::ForwardAll
                | Message::DropAll
                | Message::Edit
        )
    {
        model.status = "History is read only".into();
        return None;
    }

    match message {
        Message::Forward => return Some(Effect::Forward),
        Message::Drop => return Some(Effect::Drop),
        Message::ForwardAll => return Some(Effect::ForwardAll),
        Message::DropAll => return Some(Effect::DropAll),
        Message::Edit => return Some(Effect::Edit),
        Message::Repeat => return Some(Effect::Repeat),
        Message::Save => return Some(Effect::Save),
        Message::Quit => return Some(Effect::Quit),
        Message::ToggleIntercept => return Some(Effect::ToggleIntercept),
        Message::SwitchList => {
            let previous_id: Option<u64> = model.current().map(|request| request.id);
            let previous_state: ListState = mem::take(&mut model.preview_state);

            model.list = model.list.switch();

            let selected: usize = model.list_state().selected().unwrap_or(0);

            model.select(selected);
            model.refresh_preview();

            if let Some((id, state)) = model.inactive_preview.take() {
                if model.current().is_some_and(|request| request.id == id) {
                    model.preview_state = state;
                }
            }

            model.inactive_preview = previous_id.map(|id| (id, previous_state));
        }
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

            model.select_queue(selected);

            if model.list.is_pending() && model.current().map(|request| request.id) != selected_id {
                model.refresh_preview();
            }

            if let Some(status) = status {
                model.status = status;
            }
        }
        Message::Edited { id, raw } => {
            if model.queue.set_draft(id, raw) {
                if model.current().is_some_and(|request| request.id == id) {
                    model.refresh_preview();
                }

                model.status = format!("Request #{id} updated; awaiting a decision");
            } else {
                model.status = format!("Request #{id} is no longer pending");
            }
        }
        Message::Key(key) => match model.focus {
            Focus::Queue => {
                let previous: Option<usize> = model.list_state().selected();
                let page_size: u16 = model.queue_page_size;

                navigate(model.list_state(), key.code, page_size);

                let selected: usize = model.list_state().selected().unwrap_or(0);

                model.select(selected);

                if model.list_state().selected() != previous {
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
    use crossterm::event::KeyEvent;
    use http::Request;
    use tokio::sync::oneshot;

    use super::super::model::RequestList;
    use super::*;
    use crate::http::proxy::{PendingRequest, RequestRecord, RequestStatus};

    fn model() -> Model {
        let mut model: Model = Model::new("127.0.0.1:8001".parse().unwrap(), Default::default());
        model.list = RequestList::Pending;

        for id in [10, 20, 30] {
            let (reply, _receiver) = oneshot::channel();

            let request: PendingRequest = PendingRequest::new(
                id,
                Local::now(),
                "http://localhost/".parse().unwrap(),
                Request::new(Bytes::new()),
                reply,
            )
            .unwrap();

            model.queue.push(request);
        }

        model.select(1);
        model.refresh_preview();

        model
    }

    fn record(model: &Model, id: u64) {
        model.state.record(RequestRecord {
            id,
            timestamp: Local::now(),
            original_uri: format!("http://localhost/history-{id}").parse().unwrap(),
            raw: format!("GET /history-{id} HTTP/1.1\r\nHost: localhost\r\n\r\n").into(),
            status: RequestStatus::Forwarded,
        });
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

        assert_eq!(model.current().unwrap().id, 20);
        assert_eq!(model.queue_state.selected(), Some(0));

        model.queue.resolve(0, false).unwrap();
        update(
            &mut model,
            Message::QueueChanged {
                selected_id: Some(20),
                status: None,
            },
        );

        assert_eq!(model.current().unwrap().id, 30);

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

        assert_eq!(model.queue.get(0).unwrap().record().raw, raw);
        assert_eq!(model.current().unwrap().id, 20);
        assert_eq!(model.queue.len(), 3);
    }

    #[test]
    fn test_switching_lists_preserves_independent_selections_and_scroll() {
        let mut model: Model = model();

        *model.queue_state.offset_mut() = 1;
        model.preview_state.select(Some(1));

        record(&model, 100);
        record(&model, 200);

        model.sync_history();

        update(&mut model, Message::from_key(KeyEvent::from(KeyCode::F(2))));

        assert_eq!(model.current().unwrap().id, 100);

        update(&mut model, Message::Key(KeyEvent::from(KeyCode::Down)));

        *model.history_state.offset_mut() = 1;
        model.preview_state.select(Some(2));

        update(&mut model, Message::SwitchFocus);
        update(&mut model, Message::SwitchList);

        assert_eq!(model.current().unwrap().id, 20);
        assert_eq!(model.queue_state.offset(), 1);
        assert_eq!(model.preview_state.selected(), Some(1));
        assert_eq!(model.focus, Focus::Preview);

        update(&mut model, Message::SwitchList);

        assert_eq!(model.current().unwrap().id, 200);
        assert_eq!(model.history_state.offset(), 1);
        assert_eq!(model.preview_state.selected(), Some(2));
    }

    #[test]
    fn test_history_arrivals_preserve_selection_and_preview_when_inserted_before_it() {
        let mut model: Model = model();

        record(&model, 200);

        model.sync_history();

        update(&mut model, Message::SwitchList);

        model.preview_state.select(Some(2));

        record(&model, 100);

        assert!(model.sync_history());

        assert_eq!(model.current().unwrap().id, 200);
        assert_eq!(model.history_state.selected(), Some(1));
        assert_eq!(model.preview_state.selected(), Some(2));
    }

    #[test]
    fn test_history_blocks_pending_actions_but_allows_repeat_save_and_toggle() {
        let mut model: Model = model();

        update(&mut model, Message::SwitchList);

        for key in [
            KeyCode::Enter,
            KeyCode::F(5),
            KeyCode::F(6),
            KeyCode::F(7),
            KeyCode::F(8),
        ] {
            assert!(update(&mut model, Message::from_key(KeyEvent::from(key))).is_none());
            assert_eq!(model.queue.len(), 3);
        }

        assert!(matches!(
            update(&mut model, Message::Repeat),
            Some(Effect::Repeat)
        ));
        assert!(matches!(
            update(&mut model, Message::Save),
            Some(Effect::Save)
        ));
        assert!(matches!(
            update(&mut model, Message::from_key(KeyEvent::from(KeyCode::F(9)))),
            Some(Effect::ToggleIntercept)
        ));
    }
}
