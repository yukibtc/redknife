use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, Paragraph, Wrap};

use super::model::{Focus, Model, RequestList, escaped_line};
use crate::http::proxy::{RequestRecord, RequestStatus};
use crate::tui::constant::APP_NAME;
use crate::tui::theme;
use crate::tui::widgets::panel;
use crate::tui::widgets::scroll::clipped_line;

fn request_lines(request: &RequestRecord) -> [String; 2] {
    let method: &[u8] = request
        .raw
        .split(|byte| *byte == b' ')
        .next()
        .unwrap_or_default();

    let method: String = escaped_line(method).chars().take(20).collect();

    let mut label: String = format!(
        "#{} {} {} ({} B)",
        request.id,
        request.timestamp.format("%H:%M:%S"),
        method,
        request.raw.len()
    );

    if request.status != RequestStatus::Pending {
        label.push_str(" · ");
        label.push_str(request.status.as_str());
    }

    let uri: String = request.original_uri.to_string().escape_debug().to_string();

    [label, uri]
}

fn render_queue(frame: &mut Frame, area: Rect, model: &mut Model) {
    let title: &str = match model.list {
        RequestList::Pending => "Pending requests",
        RequestList::History => "History",
    };

    let block: Block = panel(title, model.focus.is_queue());
    let inner: Rect = block.inner(area);

    model.queue_page_size = (inner.height / 2).max(1);

    let rows: Vec<[String; 2]> = match model.list {
        RequestList::Pending => model
            .queue
            .iter()
            .map(|request| request_lines(request.record()))
            .collect(),
        RequestList::History => model
            .history
            .iter()
            .map(|entry| request_lines(entry))
            .collect(),
    };

    if rows.is_empty() {
        let text: &str = match model.list {
            RequestList::Pending if !model.state.is_intercepting() => "Intercept OFF",
            RequestList::Pending => "Waiting for HTTP requests",
            RequestList::History => "No resolved HTTP requests",
        };
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let width = inner.width.saturating_sub(2); // Keep the selection marker fixed.

    let max_width: usize = model
        .list_state()
        .selected()
        .and_then(|index| rows.get(index))
        .into_iter()
        .flatten()
        .map(|line| Line::raw(line.as_str()).width())
        .max()
        .unwrap_or(0);

    if !inner.is_empty() {
        model.list_scroll().resize(width, max_width);
    }

    let offset = model.list_scroll().offset;
    let items = rows.iter().map(|lines| {
        ListItem::new(
            lines
                .iter()
                .map(|line| Line::raw(clipped_line(line, offset, width)))
                .collect::<Vec<_>>(),
        )
    });

    let list: List = List::new(items)
        .block(block)
        .highlight_style(theme::selection())
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, model.list_state());
}

fn render_preview(frame: &mut Frame, area: Rect, model: &mut Model) {
    let block: Block = panel("Request preview (read only)", model.focus.is_preview());
    let inner: Rect = block.inner(area);

    let style: Style = if model.focus.is_preview() {
        theme::selection()
    } else {
        Style::default()
    };

    frame.render_widget(block, area);
    model
        .preview
        .render(inner, frame.buffer_mut(), &mut model.preview_state, style);
}

pub(super) fn render(frame: &mut Frame, model: &mut Model) {
    let [heading_area, queue_and_preview_area, help_area, status_area]: [Rect; 4] =
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());

    let heading: String = format!(
        "{APP_NAME} / HTTP / PROXY · {} · Intercept {} · {} pending · {} history",
        model.address,
        if model.state.is_intercepting() {
            "ON"
        } else {
            "OFF"
        },
        model.queue.len(),
        model.history.len()
    );

    frame.render_widget(
        Paragraph::new(heading).style(Style::default().fg(theme::MAIN)),
        heading_area,
    );

    if queue_and_preview_area.height < 6 {
        match model.focus {
            Focus::Queue => render_queue(frame, queue_and_preview_area, model),
            Focus::Preview => render_preview(frame, queue_and_preview_area, model),
        }
    } else {
        let layout: Layout = if queue_and_preview_area.width >= 100 {
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
        } else {
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)])
        };
        let [queue, preview]: [Rect; 2] = layout.areas(queue_and_preview_area);

        render_queue(frame, queue, model);
        render_preview(frame, preview, model);
    }

    let help: &str = match model.list {
        RequestList::Pending => {
            "↑/↓ Select · Tab Preview · Enter Edit · R Go to Repeater\nF5 Forward · F6 Drop · F7 Forward All · F8 Drop All\nF2 History · F9 Intercept ON/OFF · Ctrl+S Save · Ctrl+Q Quit"
        }
        RequestList::History => {
            "↑/↓ Select · Tab Preview · R Go to Repeater\nF2 Pending · F9 Intercept ON/OFF\nCtrl+S Save · Ctrl+Q Quit"
        }
    };

    frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: true }), help_area);
    frame.render_widget(Paragraph::new(model.status.as_str()), status_area);
}
