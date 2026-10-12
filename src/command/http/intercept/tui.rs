use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::{fs, io};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use sha1::digest::Output;
use sha1::{Digest, Sha1};
use tokio::sync::mpsc;
use tokio::task;
use tokio::task::JoinHandle;

use super::app::{App, Focus, RequestAction};
use super::ui;
use crate::command::http::intercept::app::Status;
use crate::command::http::intercept::messages::PendingRequest;

enum InputEvent {
    Key(KeyEvent),
    Resize,
}

struct RestoreTerminal;

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

struct StopKeyboard(Arc<AtomicBool>);

impl Drop for StopKeyboard {
    #[inline]
    fn drop(&mut self) {
        self.stop();
    }
}

impl StopKeyboard {
    #[inline]
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    #[inline]
    fn reader(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }

    #[inline]
    fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Run one persistent UI while proxy tasks enqueue pending requests.
pub async fn run(
    mut receiver: mpsc::Receiver<PendingRequest>,
    addr: SocketAddr,
    out_dir: Option<PathBuf>,
) -> io::Result<()> {
    let mut terminal: DefaultTerminal = ratatui::init();
    let _restore = RestoreTerminal;
    event_loop(&mut terminal, &mut receiver, addr, out_dir).await
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    receiver: &mut mpsc::Receiver<PendingRequest>,
    address: SocketAddr,
    out_dir: Option<PathBuf>,
) -> io::Result<()> {
    let mut app: App = App::new();

    let (input_sender, mut input_receiver) = mpsc::channel::<InputEvent>(128);

    let stop: StopKeyboard = StopKeyboard::new();
    let stop_reader: Arc<AtomicBool> = stop.reader();

    // Crossterm's event::read is blocking; keep it away from the Tokio workers.
    let keyboard_task: JoinHandle<io::Result<()>> =
        task::spawn_blocking(move || -> io::Result<()> {
            while !stop_reader.load(Ordering::Relaxed) {
                if event::poll(Duration::from_millis(50))? {
                    let input: InputEvent = match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => InputEvent::Key(key),
                        Event::Resize(_, _) => InputEvent::Resize,
                        _ => continue,
                    };

                    // Never block shutdown if the channel is full.
                    if input_sender.try_send(input).is_err() && input_sender.is_closed() {
                        break;
                    }
                }
            }
            Ok(())
        });

    let result: io::Result<()> = async {
        let mut proxy_open: bool = true;

        while !app.quit {
            terminal.draw(|frame| ui::render(frame, &mut app, &address.to_string()))?;

            tokio::select! {
                pending = receiver.recv(), if proxy_open => {
                    match pending {
                        Some(pending) => app.push(pending),
                        None => {
                            proxy_open = false;
                            app.status = Status::ProxyChannelClosed;
                        }
                    }
                }
                event = input_receiver.recv() => {
                    match event {
                        Some(InputEvent::Key(key)) => handle_key(&mut app, out_dir.as_ref(), key),
                        Some(InputEvent::Resize) => {}, // Next iteration redraws.
                        None => return Err(io::Error::other("Keyboard reader stopped")),
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    // Closing the UI or failing to draw must unblock every pending handler.
    app.quit();

    stop.stop();

    drop(input_receiver);

    let _ = keyboard_task.await;

    result
}

fn handle_key(app: &mut App, out_dir: Option<&PathBuf>, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('q') => {
                app.quit();
                return;
            }
            KeyCode::Char('s') => {
                save_selected(app, out_dir);
                return;
            }
            _ => {}
        }
    }

    if key.code == KeyCode::Tab {
        app.focus = match app.focus {
            Focus::Queue => Focus::Editor,
            Focus::Editor => Focus::Queue,
        };
        return;
    }

    match key.code {
        KeyCode::F(5) => {
            app.resolve_selected(RequestAction::Forward);
            return;
        }
        KeyCode::F(6) => {
            app.resolve_selected(RequestAction::Drop);
            return;
        }
        KeyCode::F(7) => {
            app.resolve_all(RequestAction::Forward);
            return;
        }
        KeyCode::F(8) => {
            app.resolve_all(RequestAction::Drop);
            return;
        }
        _ => {}
    }

    match app.focus {
        Focus::Queue => match key.code {
            KeyCode::Up => app.previous(),
            KeyCode::Down => app.next(),
            KeyCode::Enter => app.focus = Focus::Editor,
            _ => {}
        },
        Focus::Editor => match key.code {
            KeyCode::Esc => app.focus = Focus::Queue,
            _ => {
                if let Some(selected) = app.current_mut() {
                    selected.input(key);
                }
            }
        },
    }
}

fn save_selected(app: &mut App, out_dir: Option<&PathBuf>) {
    let Some(selected) = app.current() else {
        app.status = Status::NoRequestSelected;
        return;
    };

    let content: Vec<u8> = selected.bytes();

    // Hash the content, so we'll have a stable req identifier
    let mut hasher: Sha1 = Sha1::new();
    hasher.update(&content);
    let hash: Output<Sha1> = hasher.finalize();

    // Use the deterministic content hash as the filename
    let filename: String = format!("req-{}.txt", hex::encode(hash.as_slice()));
    let path: PathBuf = match out_dir {
        Some(out_dir) => out_dir.join(&filename),
        None => PathBuf::from(&filename),
    };

    app.status = match fs::write(&path, &content) {
        Ok(()) => Status::FileSaved { filename },
        Err(err) => Status::FileSaveError {
            error: err.to_string(),
        },
    };
}

#[cfg(test)]
mod tests {
    use std::fs;

    use bytes::Bytes;
    use chrono::Local;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use http::Request;
    use tokio::sync::oneshot;
    use tui_textarea::TextArea;

    use super::{App, Focus, PendingRequest, handle_key};
    use crate::command::http::intercept::messages::InterceptReply;

    fn app_with_queued_request(body: &'static [u8]) -> (App, oneshot::Receiver<InterceptReply>) {
        let (reply, receiver) = oneshot::channel();

        let mut app = App::new();

        app.push(PendingRequest {
            id: 1,
            timestamp: Local::now(),
            original_uri: "http://example.com/".parse().unwrap(),
            request: Request::builder()
                .method("POST")
                .uri("/")
                .header("content-length", body.len())
                .body(Bytes::from_static(body))
                .expect("test request must be valid"),
            reply,
        });

        (app, receiver)
    }

    #[test]
    fn f5_from_queue_forwards_the_edited_request() {
        let (mut app, mut replies) = app_with_queued_request(b"original");
        app.focus = Focus::Queue;

        let selected = app.current_mut().expect("queued request must be selected");
        selected.editor = TextArea::from([
            "PUT /changed HTTP/1.1",
            "content-length: 7",
            "",
            "edited",
            "",
        ]);

        handle_key(
            &mut app,
            None,
            KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE),
        );

        assert!(
            app.queue.is_empty(),
            "forwarded request must leave the queue"
        );
        let InterceptReply::Forward(request) = replies
            .try_recv()
            .expect("F5 must send a decision to the handler")
        else {
            panic!("F5 must forward the request, but sent Drop");
        };
        assert_eq!(request.method(), "PUT");
        assert_eq!(request.uri(), "/changed");
        assert_eq!(request.headers()["content-length"], "7");
        assert_eq!(request.body(), b"edited\n".as_slice());
    }

    #[test]
    fn f5_from_editor_forwards_the_edited_request() {
        let (mut app, mut replies) = app_with_queued_request(b"original");
        app.focus = Focus::Editor;

        let selected = app.current_mut().expect("queued request must be selected");
        selected.editor = TextArea::from([
            "PUT /changed HTTP/1.1",
            "content-length: 7",
            "",
            "edited",
            "",
        ]);

        handle_key(
            &mut app,
            None,
            KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE),
        );

        assert!(
            app.queue.is_empty(),
            "forwarded request must leave the queue"
        );
        let InterceptReply::Forward(request) = replies
            .try_recv()
            .expect("F5 must send a decision to the handler")
        else {
            panic!("F5 must forward the request, but sent Drop");
        };
        assert_eq!(request.method(), "PUT");
        assert_eq!(request.uri(), "/changed");
        assert_eq!(request.headers()["content-length"], "7");
        assert_eq!(request.body(), b"edited\n".as_slice());
    }

    #[test]
    fn saving_a_binary_request_preserves_the_complete_http_bytes() {
        let (app, _replies) = app_with_queued_request(b"\x00\xff\x1b\n");

        let selected = app.current().expect("queued request must be selected");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary-request.txt");

        fs::write(&path, selected.bytes()).expect("binary request must be saved");

        let saved = fs::read(&path).expect("saved request must be readable");
        assert_eq!(
            saved,
            b"POST / HTTP/1.1\r\ncontent-length: 4\r\n\r\n\x00\xff\x1b\n"
        );
    }

    #[test]
    fn saving_an_edited_draft_preserves_trailing_blank_lines() {
        let (mut app, _replies) = app_with_queued_request(b"original");

        let selected = app.current_mut().expect("queued request must be selected");
        selected.editor = TextArea::from(["POST /draft HTTP/1.1", "", "changed", "", ""]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edited-draft.txt");

        fs::write(&path, selected.bytes()).expect("edited draft must be saved");

        let saved = fs::read(&path).expect("saved draft must be readable");
        assert_eq!(saved, b"POST /draft HTTP/1.1\r\n\r\nchanged\n\n");
    }
}
