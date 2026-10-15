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
use crate::http::intercept::{HttpInterceptPendingRequest, RequestQueue};
use crate::tui::terminal::read_event;

/// Drain available arrivals up to the local queue limit, preserving selection by ID.
/// Returns whether arrivals were consumed, including requests that could not be queued.
fn receive_pending(
    model: &mut Model,
    receiver: &mut mpsc::Receiver<HttpInterceptPendingRequest>,
) -> bool {
    let selected_id: Option<u64> = model.current().map(|request| request.id());
    let was_empty: bool = model.queue.is_empty();

    let mut changed: bool = false;

    // Backpressure also bounds the queue while the interceptor screen is active.
    while model.queue.len() < receiver.max_capacity() {
        let Ok(pending) = receiver.try_recv() else {
            break;
        };

        changed = true;

        if let Err(error) = model.queue.push(pending) {
            update::update(
                model,
                Message::SetStatus(format!("Cannot inspect request: {error}")),
            );
        }
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
    receiver: &mut mpsc::Receiver<HttpInterceptPendingRequest>,
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

        if queue.push(pending).is_err() {
            failed += 1;
            continue;
        }

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
    let selected_id: Option<u64> = model.current().map(|request| request.id());

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
    receiver: &mut mpsc::Receiver<HttpInterceptPendingRequest>,
    forward: bool,
) -> Message {
    let selected_id: Option<u64> = model.current().map(|request| request.id());

    let (resolved, failed) = resolve_all(&mut model.queue, receiver, forward);
    let status: String = format!("Resolved {resolved} requests; {failed} errors");

    Message::QueueChanged {
        selected_id,
        status: Some(status),
    }
}

fn export_selected(model: &Model, request_export_dir: Option<&Path>) -> Message {
    let status: String = match model.current() {
        None => "No request selected".into(),
        Some(request) => match save_request(request.raw(), request_export_dir) {
            Ok(path) => format!("Saved {}", path.display()),
            Err(error) => format!("Save failed: {error}"),
        },
    };

    Message::SetStatus(status)
}

pub(super) async fn run(
    terminal: &mut DefaultTerminal,
    model: &mut Model,
    receiver: &mut mpsc::Receiver<HttpInterceptPendingRequest>,
    request_export_dir: Option<&Path>,
) -> io::Result<Action> {
    terminal.clear()?;

    let mut redraw: bool = true;

    loop {
        redraw |= receive_pending(model, receiver);

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
                        let raw: Vec<u8> = request.raw().to_vec();

                        let action: Action = if effect.is_edit() {
                            Action::Edit {
                                id: request.id(),
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

    use super::*;
    use crate::http::intercept::HttpInterceptReply;

    fn pending(
        id: u64,
    ) -> (
        HttpInterceptPendingRequest,
        oneshot::Receiver<HttpInterceptReply>,
    ) {
        let (reply, receiver) = oneshot::channel();

        (
            HttpInterceptPendingRequest {
                id,
                timestamp: Local::now(),
                original_uri: format!("http://localhost/request-{id}").parse().unwrap(),
                request: Request::builder()
                    .uri(format!("/request-{id}"))
                    .body(Bytes::new())
                    .unwrap(),
                reply,
            },
            receiver,
        )
    }

    #[test]
    fn test_forward_after_end_uses_the_last_request() {
        let mut model: Model = Model::new("127.0.0.1:8001".parse().unwrap());
        let (first, mut first_reply) = pending(10);
        let (last, mut last_reply) = pending(20);
        model.queue.push(first).unwrap();
        model.queue.push(last).unwrap();
        model.select(0);

        update::update(&mut model, Message::Key(KeyEvent::from(KeyCode::End)));
        let message: Message = resolve_selected(&mut model, true);
        update::update(&mut model, message);

        let HttpInterceptReply::Forward(request) = last_reply.try_recv().unwrap() else {
            panic!("expected the last request to be forwarded");
        };
        assert_eq!(request.uri(), "/request-20");
        assert_eq!(model.current().unwrap().id(), 10);
        assert!(matches!(
            first_reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn test_arrivals_respect_queue_capacity() {
        let (sender, mut receiver) = mpsc::channel(2);
        let mut model: Model = Model::new("127.0.0.1:8001".parse().unwrap());
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

        assert_eq!(model.current().unwrap().id(), 20);
        assert_eq!(model.queue.get(1).unwrap().id(), 30);
        assert!(receiver.is_empty());
    }

    #[test]
    fn test_forward_all_includes_waiting_arrivals() {
        let (sender, mut receiver) = mpsc::channel(1);
        let mut queue: RequestQueue = RequestQueue::default();
        let (queued, mut queued_reply) = pending(10);
        let (waiting, mut waiting_reply) = pending(20);
        queue.push(queued).unwrap();
        sender.try_send(waiting).unwrap();

        assert_eq!(resolve_all(&mut queue, &mut receiver, true), (2, 0));

        assert!(queue.is_empty());
        assert!(receiver.is_empty());
        assert!(matches!(
            queued_reply.try_recv(),
            Ok(HttpInterceptReply::Forward(_))
        ));
        assert!(matches!(
            waiting_reply.try_recv(),
            Ok(HttpInterceptReply::Forward(_))
        ));
    }
}
