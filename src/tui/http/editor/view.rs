use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};
use ratatui_textarea::TextArea;

use super::document::Document;
use super::model::{Focus, Mode, Model};
use crate::tui::constant::APP_NAME;
use crate::tui::theme;
use crate::tui::widgets::hex_view::HexView;
use crate::tui::widgets::panel;

fn text_editor(
    frame: &mut Frame,
    area: Rect,
    content: &TextArea<'static>,
    title: &'static str,
    active: bool,
) {
    let block: Block = panel(title, active);
    let inner: Rect = block.inner(area);

    frame.render_widget(block, area);
    frame.render_widget(content, inner);
}

fn document_editor(
    frame: &mut Frame,
    area: Rect,
    content: &mut Document,
    title: &'static str,
    active: bool,
    hex: bool,
) {
    if hex || !content.is_editable() {
        let block: Block = panel(title, active);
        let inner: Rect = block.inner(area);
        let style: Style = if active {
            theme::selection()
        } else {
            Style::default()
        };
        let hex_view: &mut HexView = content.hex_view();

        hex_view.set_highlight_style(style);

        frame.render_widget(block, area);
        frame.render_widget(hex_view, inner);
    } else {
        text_editor(frame, area, content.editor(), title, active);
    }
}

/// Draw the panes, available shortcuts, and footer for the current editor mode.
pub(super) fn render(frame: &mut Frame, model: &mut Model) {
    let mode: &str = match model.mode() {
        Mode::Edit => "EDIT",
        Mode::Repeat => "REPEAT",
    };

    let binary: bool = match model.focus() {
        Focus::Response => model
            .response
            .as_ref()
            .is_some_and(|doc| !doc.is_editable()),
        _ => !model.request.is_editable(),
    };

    let view: &str = if model.hex || binary {
        "HEX / READ ONLY"
    } else {
        "RAW"
    };

    let target_title: &str = match model.mode() {
        Mode::Edit => "Destination",
        Mode::Repeat => "Target",
    };

    let [
        heading_area,
        target_area,
        documents_area,
        help_area,
        status_area,
    ]: [Rect; 5] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(0), // Fill all the remaining space
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new(format!("{APP_NAME} / HTTP / {mode} · {view}"))
            .style(Style::default().fg(theme::MAIN)),
        heading_area,
    );

    text_editor(
        frame,
        target_area,
        &model.target,
        target_title,
        model.focus().is_target(),
    );

    let focus: Focus = model.focus();

    match model.mode() {
        Mode::Edit => document_editor(
            frame,
            documents_area,
            &mut model.request,
            "Request",
            true,
            model.hex,
        ),
        Mode::Repeat => {
            let [request_area, response_area]: [Rect; 2] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(documents_area);

            document_editor(
                frame,
                request_area,
                &mut model.request,
                "Request",
                focus.is_request(),
                model.hex,
            );

            match &mut model.response {
                Some(response) => document_editor(
                    frame,
                    response_area,
                    response,
                    "Response (read only)",
                    focus.is_response(),
                    model.hex,
                ),
                None => frame.render_widget(
                    Paragraph::new(if model.sending {
                        "Waiting for response..."
                    } else {
                        "No response"
                    })
                    .block(panel("Response", model.focus().is_response())),
                    response_area,
                ),
            }
        }
    }

    let help: String = match model.mode() {
        Mode::Edit => {
            "F5 Apply · Esc Cancel · F2 Raw/Hex · Ctrl+S Save request\nChanges return to Interceptor; no request is sent.".into()
        }
        Mode::Repeat => {
            format!("F5 Send · Tab / Shift-Tab Focus · F2 Raw/Hex · Ctrl+S Save request\nCtrl+C Cancel send · Ctrl+Q / Esc {} · Response arrows scroll", model.exit_label.as_str())
        }
    };

    frame.render_widget(Paragraph::new(help), help_area);
    frame.render_widget(Paragraph::new(model.status.as_str()), status_area);
}
