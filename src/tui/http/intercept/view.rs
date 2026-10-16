use std::mem;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, Paragraph, Wrap};

use super::model::{Focus, Model, escaped_line};
use crate::tui::constant::APP_NAME;
use crate::tui::theme;
use crate::tui::widgets::panel;

fn render_queue(frame: &mut Frame, area: Rect, model: &mut Model) {
    let block: Block = panel("Pending requests", model.focus.is_queue());
    let inner: Rect = block.inner(area);

    model.queue_page_size = (inner.height / 2).max(1);

    if model.queue.is_empty() {
        frame.render_widget(
            Paragraph::new("Waiting for HTTP requests").block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = model
        .queue
        .iter()
        .map(|request| {
            let method: &[u8] = request
                .raw()
                .split(|byte| *byte == b' ')
                .next()
                .unwrap_or_default();
            let method: String = escaped_line(method).chars().take(20).collect();
            let label: String = format!(
                "#{} {} {} ({} B)",
                request.id(),
                request.timestamp().format("%H:%M:%S"),
                method,
                request.raw().len()
            );
            let uri: String = request
                .original_uri()
                .to_string()
                .escape_debug()
                .to_string();

            ListItem::new(vec![Line::raw(label), Line::raw(uri)])
        })
        .collect();

    let list: List = List::new(items)
        .block(block)
        .highlight_style(theme::selection())
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut model.queue_state);
}

fn render_preview(frame: &mut Frame, area: Rect, model: &mut Model) {
    let block: Block = panel("Request preview (read only)", model.focus.is_preview());
    let inner: Rect = block.inner(area);

    model.preview_page_size = inner.height.max(1);

    let style: Style = if model.focus.is_preview() {
        theme::selection()
    } else {
        Style::default()
    };

    model.preview = mem::take(&mut model.preview).highlight_style(style);

    frame.render_widget(block, area);
    frame.render_stateful_widget(&model.preview, inner, &mut model.preview_state);
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
        "{APP_NAME} / HTTP / INTERCEPT · {} · {} pending",
        model.address,
        model.queue.len()
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

    frame.render_widget(Paragraph::new(
        "↑/↓ Select · Tab Preview · Enter Edit · R Go to Repeater\nF5 Forward · F6 Drop · F7 Forward All · F8 Drop All\nCtrl+S Save · Ctrl+Q Quit"
    ).wrap(Wrap { trim: true }), help_area);
    frame.render_widget(Paragraph::new(model.status.as_str()), status_area);
}
