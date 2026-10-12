use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, Paragraph};

use super::app::{App, Focus};
use super::messages::PendingRequest;

const ACTIVE_BORDER: Color = Color::LightRed;
const INACTIVE_BORDER: Color = Color::Gray;
const HIGHLIGHT_TEXT: Color = Color::White;
const ACTIVE_BACKGROUND: Color = Color::Red;
const INACTIVE_BACKGROUND: Color = Color::DarkGray;

fn highlight_style(active: bool) -> Style {
    let style: Style = Style::default().fg(HIGHLIGHT_TEXT).bg(if active {
        ACTIVE_BACKGROUND
    } else {
        INACTIVE_BACKGROUND
    });

    if active {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn panel_block(title: &'static str, active: bool) -> Block<'static> {
    let style: Style = if active {
        Style::default()
            .fg(ACTIVE_BORDER)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(INACTIVE_BORDER)
    };

    Block::bordered()
        .title(title)
        .title_style(style)
        .border_style(style)
        .border_type(if active {
            BorderType::Double
        } else {
            BorderType::Plain
        })
}

pub fn render(frame: &mut Frame, app: &mut App, listen_addr: &str) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let mode: &str = match app.focus {
        Focus::Queue => "QUEUE",
        Focus::Editor => {
            if app.current().is_some_and(|req| req.editable) {
                "EDITING"
            } else {
                "READ ONLY"
            }
        }
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "REDKNIFE / INTERCEPT",
                Style::default()
                    .fg(ACTIVE_BORDER)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  |  "),
            Span::styled(format!(" {mode} "), highlight_style(true)),
            Span::raw(format!(
                "  |  {listen_addr}  |  {} pending",
                app.queue.len()
            )),
        ])),
        rows[0],
    );

    let columns =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(rows[1]);

    render_queue(frame, columns[0], app);
    render_editor(frame, columns[1], app);

    let shortcuts: &str = match app.focus {
        Focus::Queue => {
            "Up/Down Select | Tab Edit | F5 Forward | F6 Drop\nF7 Forward All | F8 Drop All | Ctrl+S Save | Ctrl+Q Quit"
        }
        Focus::Editor => {
            "Tab/Esc Queue | F5 Forward | F6 Drop\nF7 Forward All | F8 Drop All | Ctrl+S Save | Ctrl+Q Quit"
        }
    };

    frame.render_widget(Paragraph::new(shortcuts), rows[2]);
    frame.render_widget(Paragraph::new(app.status.as_str()), rows[3]);
}

fn render_queue(frame: &mut Frame, area: Rect, app: &mut App) {
    let items: Vec<ListItem> = app
        .queue
        .iter()
        .map(|item| {
            let req: &PendingRequest = &item.pending;
            ListItem::new(vec![
                Line::from(format!(
                    "#{} {} {}",
                    req.id,
                    req.timestamp,
                    req.request.method()
                )),
                Line::from(format!("  {}", req.original_uri)),
            ])
        })
        .collect();

    let active: bool = app.focus.is_queue();

    let list: List = List::new(items)
        .block(panel_block("Request Queue", active))
        .highlight_style(highlight_style(active))
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut app.selection);
}

fn render_editor(frame: &mut Frame, area: Rect, app: &mut App) {
    let active: bool = app.focus.is_editor();

    if let Some(req) = app.current_mut() {
        let title: &str = if !req.editable {
            "HTTP Request [HEX / READ ONLY]"
        } else if active {
            "HTTP Request [EDITING]"
        } else {
            "HTTP Request"
        };

        req.editor.set_block(panel_block(title, active));

        req.editor.set_cursor_line_style(if active {
            Style::default().fg(HIGHLIGHT_TEXT).bg(INACTIVE_BACKGROUND)
        } else {
            Style::default()
        });

        req.editor.set_cursor_style(if active {
            highlight_style(true)
        } else {
            Style::default()
        });

        frame.render_widget(&req.editor, area);
    } else {
        frame.render_widget(
            Paragraph::new("Waiting for HTTP requests...")
                .block(panel_block("HTTP Request", active)),
            area,
        );
    }
}
