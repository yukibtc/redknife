use std::borrow::Cow;

use bytes::Bytes;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use http::Request;

use super::message::Message;
use super::model::{Focus, Mode, Model};
use crate::http::request::{HttpWireRequestError, parse_request};
use crate::http::target::HttpTarget;

/// Work requested by update and carried out by the editor's runner.
pub(super) enum Effect {
    /// Return a validated draft to the caller without sending it (EDIT only).
    Apply(Vec<u8>),
    /// Leave the screen after cancelling any pending network operation.
    Close,
    /// Start one HTTP send to the specified destination (REPEAT only).
    Send {
        request: Box<Request<Bytes>>,
        target: HttpTarget,
    },
    /// Stop the current send while retaining any response bytes received so far.
    CancelSend,
    /// Export these request bytes to the configured directory.
    Save(Vec<u8>),
}

/// Apply one event without I/O, returning any work needed from the runner.
pub(super) fn update(model: &mut Model, message: Message) -> Option<Effect> {
    match message {
        Message::Close => return Some(Effect::Close),
        Message::Interrupt => return Some(Effect::CancelSend),
        Message::Save => return Some(Effect::Save(model.request.bytes().into_owned())),
        Message::ToggleView => {
            model.hex = !model.hex;
        }
        Message::FocusNext => {
            model.focus_next();
        }
        Message::FocusPrevious => {
            model.focus_previous();
        }
        Message::Submit => {
            if !model.sending {
                return submit(model);
            }
        }
        Message::SendFinished(result) => {
            model.send_finished(result);
        }
        Message::SetStatus(status) => model.status = status,
        Message::Key(key) => handle_key(model, key),
    }

    model.sync_focus();

    None
}

/// Validate the draft before either returning it or preparing a network send.
fn submit(model: &mut Model) -> Option<Effect> {
    let raw: Cow<'_, [u8]> = model.request.bytes();

    let request: Request<Bytes> = match parse_request(&raw) {
        Ok(request) => request,
        Err(error) => {
            model.status = error.to_string();
            return None;
        }
    };

    match model.mode() {
        Mode::Edit => {
            // The pending request retains its original destination in the interceptor.
            if request.uri().scheme().is_some() || request.uri().authority().is_some() {
                model.status = HttpWireRequestError::ExpectedOriginForm.to_string();
                return None;
            }

            Some(Effect::Apply(raw.into_owned()))
        }
        Mode::Repeat => {
            let target: HttpTarget = match model.target.lines().join("").parse() {
                Ok(target) => target,
                Err(error) => {
                    model.status = format!("Invalid target: {error}");
                    return None;
                }
            };

            // Mark the send before handing it to the runner to prevent duplicate submissions.
            model.sending = true;
            model.response = None;
            model.status = "Sending · Ctrl+C cancels".into();

            Some(Effect::Send {
                request: Box::new(request),
                target,
            })
        }
    }
}

fn handle_key(model: &mut Model, key: KeyEvent) {
    match model.focus() {
        Focus::Target => {
            if model.mode().is_repeat() {
                let newline: bool = matches!(key.code, KeyCode::Enter | KeyCode::Char('\n' | '\r'))
                    || (key.code == KeyCode::Char('m')
                        && key.modifiers.contains(KeyModifiers::CONTROL));

                if !newline {
                    model.target.input(key);
                }
            }
        }
        Focus::Request => {
            if model.hex || !model.request.is_editable() {
                model.request.hex_view().input(key.code);
            } else {
                model.request.input(key, true);
            }
        }
        Focus::Response => {
            if let Some(response) = &mut model.response {
                if model.hex || !response.is_editable() {
                    response.hex_view().input(key.code);
                } else {
                    response.input(key, false);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui_textarea::TextArea;

    use super::super::send::SendResult;
    use super::*;

    const REQUEST: &[u8] = b"GET / HTTP/1.1\r\n\r\n";

    fn model(mode: Mode) -> Model {
        Model::new(mode, REQUEST.to_vec(), "http://localhost:8080")
    }

    #[test]
    fn test_edit_returns_the_draft() {
        let mut model: Model = model(Mode::Edit);
        let Some(Effect::Apply(raw)) = update(&mut model, Message::Submit) else {
            panic!("expected the edited request");
        };

        assert_eq!(raw, REQUEST);
    }

    #[test]
    fn test_edit_rejects_a_different_destination() {
        for raw in [
            b"GET http://other.host/ HTTP/1.1\r\n\r\n".as_slice(),
            b"CONNECT other.host:443 HTTP/1.1\r\n\r\n",
        ] {
            let mut model: Model = Model::new(Mode::Edit, raw.to_vec(), "http://localhost");

            assert!(update(&mut model, Message::Submit).is_none());
            assert!(!model.status.is_empty());
        }
    }

    #[test]
    fn test_repeat_rejects_an_invalid_target() {
        let mut model: Model = model(Mode::Repeat);
        model.target = TextArea::from(["not a target"]);

        assert!(update(&mut model, Message::Submit).is_none());
        assert!(!model.sending);
    }

    #[test]
    fn test_repeat_prevents_overlapping_sends() {
        let mut model: Model = model(Mode::Repeat);
        let Some(Effect::Send { request, target }) = update(&mut model, Message::Submit) else {
            panic!("expected an HTTP send");
        };

        assert_eq!(request.uri(), "/");
        assert_eq!(target.to_string(), "http://localhost:8080");
        assert!(model.sending);
        assert!(update(&mut model, Message::Submit).is_none());
    }

    #[test]
    fn test_partial_response_is_read_only() {
        let mut model: Model = model(Mode::Repeat);
        let raw: &[u8] = b"HTTP/1.1 200 OK\r\n\r\npartial";
        model.sending = true;

        update(
            &mut model,
            Message::SendFinished(SendResult {
                bytes: raw.to_vec(),
                error: Some("cancelled".into()),
            }),
        );
        update(&mut model, Message::FocusNext);
        update(&mut model, Message::Key(KeyEvent::from(KeyCode::Char('X'))));

        assert_eq!(model.response.as_ref().unwrap().bytes(), raw);
        assert!(!model.sending);
    }

    #[test]
    fn test_hex_ignores_text_input() {
        let mut model: Model = model(Mode::Repeat);
        update(&mut model, Message::ToggleView);
        update(&mut model, Message::Key(KeyEvent::from(KeyCode::Char('X'))));

        assert_eq!(model.request.bytes(), REQUEST);
    }
}
