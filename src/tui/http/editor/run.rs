//! Drive the editor: terminal events, model updates, and requested I/O effects.

use std::io;
use std::path::Path;

use crossterm::event::{Event, KeyEventKind};
use ratatui::DefaultTerminal;

use super::message::Message;
use super::model::Model;
use super::send::{PendingSend, SendResult};
use super::update::{self, Effect};
use super::view;
use crate::http::export::save_request;
use crate::tui::terminal::read_event;

/// Export request bytes
fn export_request(raw: &[u8], request_export_dir: Option<&Path>) -> Message {
    let status: String = match save_request(raw, request_export_dir) {
        Ok(path) => format!("Saved {}", path.display()),
        Err(error) => format!("Save failed: {error}"),
    };

    Message::SetStatus(status)
}

/// Run until EDIT applies a draft (`Some`) or the screen is closed (`None`).
/// A missing export directory saves requests in the current working directory.
pub(super) async fn run(
    terminal: &mut DefaultTerminal,
    mut model: Model,
    request_export_dir: Option<&Path>,
) -> io::Result<Option<Vec<u8>>> {
    terminal.clear()?;

    let mut pending_send: Option<PendingSend> = None;
    let mut redraw: bool = true;

    loop {
        // Collect a finished send before drawing so its result is visible immediately.
        if pending_send.as_ref().is_some_and(PendingSend::is_finished) {
            if let Some(send) = pending_send.take() {
                let result: SendResult = send.finish().await;
                update::update(&mut model, Message::SendFinished(result));
                redraw = true;
            }
        }

        if redraw {
            terminal.draw(|frame| view::render(frame, &mut model))?;
        }

        redraw = false;

        // Each bounded read finishes before the editor can return control to its caller.
        let message: Message = match read_event().await? {
            Some(Event::Key(key)) if key.kind != KeyEventKind::Release => Message::from_key(key),
            Some(Event::Resize(_, _)) => {
                redraw = true;
                continue;
            }
            _ => continue,
        };

        redraw = true;

        let Some(effect) = update::update(&mut model, message) else {
            continue;
        };

        // Execute the effect requested by update.
        match effect {
            Effect::Apply(raw) => return Ok(Some(raw)),
            Effect::Close => {
                if let Some(send) = pending_send.take() {
                    send.cancel().await;
                }

                return Ok(None);
            }
            Effect::Save(raw) => {
                let message: Message = export_request(&raw, request_export_dir);

                update::update(&mut model, message);
            }
            Effect::Send { request, target } => {
                pending_send = Some(PendingSend::start(*request, target));
            }
            Effect::CancelSend => {
                if let Some(send) = pending_send.take() {
                    let result: SendResult = send.cancel().await;
                    update::update(&mut model, Message::SendFinished(result));
                }
            }
        }
    }
}
