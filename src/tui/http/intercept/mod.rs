use std::io;
use std::net::SocketAddr;
use std::path::Path;

use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

mod message;
mod model;
mod run;
mod update;
mod view;

use self::message::Message;
use self::model::Model;
use crate::http::intercept::HttpInterceptPendingRequest;
use crate::http::target::HttpTarget;

pub(crate) enum Action {
    /// Edit a pending draft
    Edit {
        id: u64,
        raw: Vec<u8>,
        target: HttpTarget,
    },
    /// Repeat an independent copy without resolving or modifying the pending request.
    Repeat { raw: Vec<u8>, target: HttpTarget },
    /// Shut down
    Quit,
}

pub(crate) struct HttpInterceptTuiSession {
    model: Model,
    receiver: mpsc::Receiver<HttpInterceptPendingRequest>,
}

impl HttpInterceptTuiSession {
    /// Start an empty screen for a proxy already sending requests to this receiver.
    pub(crate) fn new(
        receiver: mpsc::Receiver<HttpInterceptPendingRequest>,
        address: SocketAddr,
    ) -> Self {
        Self {
            model: Model::new(address),
            receiver,
        }
    }

    #[inline]
    pub(crate) async fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        request_export_dir: Option<&Path>,
    ) -> io::Result<Action> {
        run::run(
            terminal,
            &mut self.model,
            &mut self.receiver,
            request_export_dir,
        )
        .await
    }

    /// Apply a returned draft by ID, even if the current selection has changed.
    #[inline]
    pub(crate) fn apply_edit(&mut self, id: u64, raw: Vec<u8>) {
        update::update(&mut self.model, Message::Edited { id, raw });
    }
}
