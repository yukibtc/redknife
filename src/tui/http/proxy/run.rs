use std::io;
use std::path::Path;

use crossterm::event::{Event, KeyEventKind};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use super::message::Message;
use super::model::Model;
use super::update::{self, Effect};
use super::{Action, view};
use crate::http::export::save_request;
use crate::http::proxy::{PendingRequest, RequestQueue};
use crate::tui::terminal::read_event;

/// Drain available arrivals up to the local queue limit, preserving selection by ID.
/// Returns whether arrivals were consumed.
fn receive_pending(model: &mut Model, receiver: &mut mpsc::Receiver<PendingRequest>) -> bool {
    let selected_id: Option<u64> = model
        .queue_state
        .selected()
        .and_then(|index| model.queue.get(index))
        .map(|request| request.record().id);

    let was_empty: bool = model.queue.is_empty();

    let mut changed: bool = false;

    // Backpressure also bounds the queue while the proxy screen is active.
    while model.queue.len() < receiver.max_capacity() {
        let Ok(pending) = receiver.try_recv() else {
            break;
        };

        changed = true;

        model.queue.push(pending);
    }

    if changed {
        let status: Option<String> = if was_empty && !model.queue.is_empty() {
            Some("Awaiting a decision".into())
        } else {
            None
        };

        update::update(
            model,
            Message::QueueChanged {
                selected_id,
                status,
            },
        );
    }

    changed
}

fn resolve_all(
    queue: &mut RequestQueue,
    receiver: &mut mpsc::Receiver<PendingRequest>,
    forward: bool,
) -> (usize, usize) {
    // Resolve a snapshot, leaving subsequently arriving requests for inspection.
    let waiting: usize = receiver.len();

    let (mut resolved, mut failed) = queue.resolve_all(forward);

    // Process the channel snapshot one request at a time, keeping memory bounded.
    for _ in 0..waiting {
        if queue.len() >= receiver.max_capacity() {
            break;
        }

        let Ok(pending) = receiver.try_recv() else {
            break;
        };

        queue.push(pending);

        let index: usize = queue.len() - 1;

        match queue.resolve(index, forward) {
            Ok(()) => resolved += 1,
            Err(_) => failed += 1,
        }
    }

    (resolved, failed)
}

fn resolve_selected(model: &mut Model, forward: bool) -> Message {
    let selected_index: Option<usize> = model.queue_state.selected();
    let selected_id: Option<u64> = model
        .queue_state
        .selected()
        .and_then(|index| model.queue.get(index))
        .map(|request| request.record().id);

    let status: String = match (selected_index, selected_id) {
        (Some(index), Some(id)) => match model.queue.resolve(index, forward) {
            Ok(()) => format!(
                "Request #{id} {}",
                if forward { "forwarded" } else { "dropped" }
            ),
            Err(error) => format!("Cannot forward request #{id}: {error}"),
        },
        _ => "No request selected".into(),
    };

    Message::QueueChanged {
        selected_id,
        status: Some(status),
    }
}

fn resolve_waiting(
    model: &mut Model,
    receiver: &mut mpsc::Receiver<PendingRequest>,
    forward: bool,
) -> Message {
    let selected_id: Option<u64> = model
        .queue_state
        .selected()
        .and_then(|index| model.queue.get(index))
        .map(|request| request.record().id);

    let (resolved, failed) = resolve_all(&mut model.queue, receiver, forward);
    let status: String = format!("Resolved {resolved} requests; {failed} errors");

    Message::QueueChanged {
        selected_id,
        status: Some(status),
    }
}

fn toggle_intercept(model: &mut Model, receiver: &mut mpsc::Receiver<PendingRequest>) -> Message {
    let enabled: bool = model.state.toggle_intercepting();

    if enabled {
        return Message::SetStatus("Intercept ON".into());
    }

    let selected_id: Option<u64> = model
        .queue_state
        .selected()
        .and_then(|index| model.queue.get(index))
        .map(|request| request.record().id);

    let (forwarded, invalid) = model.queue.resolve_all(true);

    // Free local capacity even if every draft is invalid before draining the channel.
    model.queue.resolve_all(false);

    let (waiting_forwarded, waiting_invalid) = resolve_all(&mut model.queue, receiver, true);

    model.queue.resolve_all(false);

    let forwarded: usize = forwarded + waiting_forwarded;
    let invalid: usize = invalid + waiting_invalid;

    Message::QueueChanged {
        selected_id,
        status: Some(format!(
            "Intercept OFF; {forwarded} forwarded; {invalid} invalid drafts dropped"
        )),
    }
}

fn export_selected(model: &Model, request_export_dir: Option<&Path>) -> Message {
    let status: String = match model.current() {
        None => "No request selected".into(),
        Some(request) => match save_request(&request.raw, request_export_dir) {
            Ok(path) => format!("Saved {}", path.display()),
            Err(error) => format!("Save failed: {error}"),
        },
    };

    Message::SetStatus(status)
}

pub(super) async fn run(
    terminal: &mut DefaultTerminal,
    model: &mut Model,
    receiver: &mut mpsc::Receiver<PendingRequest>,
    request_export_dir: Option<&Path>,
) -> io::Result<Action> {
    terminal.clear()?;

    let mut redraw: bool = true;

    loop {
        redraw |= receive_pending(model, receiver);
        redraw |= model.sync_history();

        if redraw {
            terminal.draw(|frame| view::render(frame, model))?;
        }

        redraw = false;

        let message: Message = match read_event().await? {
            Some(Event::Key(key)) if key.kind != KeyEventKind::Release => Message::from_key(key),
            Some(Event::Resize(_, _)) => {
                redraw = true;
                continue;
            }
            _ => continue,
        };

        redraw = true;

        let Some(effect) = update::update(model, message) else {
            continue;
        };

        match effect {
            Effect::Quit => return Ok(Action::Quit),
            Effect::ToggleIntercept => {
                let message: Message = toggle_intercept(model, receiver);
                update::update(model, message);
            }
            Effect::Forward | Effect::Drop => {
                let forward: bool = effect.is_forward();
                let message: Message = resolve_selected(model, forward);

                update::update(model, message);
            }
            Effect::ForwardAll | Effect::DropAll => {
                let forward: bool = effect.is_forward_all();
                let message: Message = resolve_waiting(model, receiver, forward);

                update::update(model, message);
            }
            Effect::Save => {
                let message: Message = export_selected(model, request_export_dir);

                update::update(model, message);
            }
            Effect::Edit | Effect::Repeat => {
                let Some(request) = model.current() else {
                    update::update(model, Message::SetStatus("No request selected".into()));
                    continue;
                };

                match request.target() {
                    Ok(target) => {
                        let raw: Vec<u8> = request.raw.to_vec();

                        let action: Action = if effect.is_edit() {
                            Action::Edit {
                                id: request.id,
                                raw,
                                target,
                            }
                        } else {
                            Action::Repeat { raw, target }
                        };

                        return Ok(action);
                    }
                    Err(error) => {
                        update::update(model, Message::SetStatus(error.to_string()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use chrono::Local;
    use crossterm::event::{KeyCode, KeyEvent};
    use http::Request;
    use tokio::sync::oneshot;

    use super::super::model::RequestList;
    use super::*;

    fn pending(id: u64) -> (PendingRequest, oneshot::Receiver<Option<Request<Bytes>>>) {
        let (reply, receiver) = oneshot::channel();

        (
            PendingRequest::new(
                id,
                Local::now(),
                format!("http://localhost/request-{id}").parse().unwrap(),
                Request::builder()
                    .uri(format!("/request-{id}"))
                    .body(Bytes::new())
                    .unwrap(),
                reply,
            )
            .unwrap(),
            receiver,
        )
    }

    #[test]
    fn test_forward_after_end_uses_the_last_request() {
        let mut model: Model = Model::new("127.0.0.1:8001".parse().unwrap(), Default::default());
        model.list = RequestList::Pending;

        let (first, mut first_reply) = pending(10);
        let (last, mut last_reply) = pending(20);

        model.queue.push(first);
        model.queue.push(last);
        model.select(0);

        update::update(&mut model, Message::Key(KeyEvent::from(KeyCode::End)));
        let message: Message = resolve_selected(&mut model, true);
        update::update(&mut model, message);

        let Some(request) = last_reply.try_recv().unwrap() else {
            panic!("expected the last request to be forwarded");
        };

        assert_eq!(request.uri(), "/request-20");
        assert_eq!(model.current().unwrap().id, 10);
        assert!(matches!(
            first_reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn test_arrivals_respect_queue_capacity() {
        let (sender, mut receiver) = mpsc::channel(2);

        let mut model: Model = Model::new("127.0.0.1:8001".parse().unwrap(), Default::default());
        model.list = RequestList::Pending;

        for id in [10, 20] {
            let (request, _reply) = pending(id);
            sender.try_send(request).unwrap();
        }
        assert!(receive_pending(&mut model, &mut receiver));
        model.select(1);

        let (request, _reply) = pending(30);
        sender.try_send(request).unwrap();

        assert!(!receive_pending(&mut model, &mut receiver));
        assert_eq!(model.queue.len(), 2);
        assert_eq!(receiver.len(), 1);

        model.queue.resolve(0, false).unwrap();
        update::update(
            &mut model,
            Message::QueueChanged {
                selected_id: Some(20),
                status: None,
            },
        );
        assert!(receive_pending(&mut model, &mut receiver));

        assert_eq!(model.current().unwrap().id, 20);
        assert_eq!(model.queue.get(1).unwrap().record().id, 30);
        assert!(receiver.is_empty());
    }

    #[test]
    fn test_disabling_interception_resolves_drafts_and_waiting_arrivals() {
        let (sender, mut receiver) = mpsc::channel(2);

        let mut model: Model = Model::new("127.0.0.1:8001".parse().unwrap(), Default::default());

        assert!(model.state.toggle_intercepting());

        let (valid, mut valid_reply) = pending(10);
        let (invalid, mut invalid_reply) = pending(20);
        let (waiting, mut waiting_reply) = pending(30);

        model.queue.push(valid);
        model.queue.push(invalid);
        model
            .queue
            .set_draft(10, b"GET /edited HTTP/1.1\r\n\r\n".to_vec());
        model.queue.set_draft(20, b"invalid draft".to_vec());

        sender.try_send(waiting).unwrap();

        let message: Message = toggle_intercept(&mut model, &mut receiver);
        update::update(&mut model, message);

        assert!(!model.state.is_intercepting());
        assert!(model.queue.is_empty());
        assert!(receiver.is_empty());
        assert_eq!(valid_reply.try_recv().unwrap().unwrap().uri(), "/edited");
        assert!(matches!(invalid_reply.try_recv(), Ok(None)));
        assert!(matches!(waiting_reply.try_recv(), Ok(Some(_))));
    }
}
