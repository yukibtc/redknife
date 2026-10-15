//! HTTP request editor in EDIT (return a draft) or REPEAT (send independent requests).

use std::io;
use std::path::Path;

use ratatui::DefaultTerminal;

mod document;
mod message;
mod model;
mod run;
mod send;
mod update;
mod view;

pub(crate) use self::model::ExitLabel;
use self::model::{Mode, Model};
use crate::http::target::HttpTarget;

/// Edit a request for the caller without sending it or changing its destination.
/// Returns the applied bytes, or `None` when closed without applying the draft.
pub(crate) async fn edit(
    terminal: &mut DefaultTerminal,
    raw: Vec<u8>,
    destination: &HttpTarget,
    request_export_dir: Option<&Path>,
) -> io::Result<Option<Vec<u8>>> {
    let model: Model = Model::new(Mode::Edit, raw, &destination.to_string());

    run::run(terminal, model, request_export_dir).await
}

/// Send and inspect an independent request copy until the editor is closed.
pub(crate) async fn repeat(
    terminal: &mut DefaultTerminal,
    raw: Vec<u8>,
    target: &HttpTarget,
    request_export_dir: Option<&Path>,
    exit_label: ExitLabel,
) -> io::Result<()> {
    let mut model: Model = Model::new(Mode::Repeat, raw, &target.to_string());
    model.exit_label = exit_label;

    run::run(terminal, model, request_export_dir).await?;

    Ok(())
}
