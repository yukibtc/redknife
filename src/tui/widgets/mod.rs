use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType};

pub(super) mod hex_view;
pub(super) mod scroll;

use super::theme;

/// Build a rounded panel whose border indicates whether the pane has focus.
pub(super) fn panel(title: &'static str, active: bool) -> Block<'static> {
    let color: Color = if active { theme::MAIN } else { theme::INACTIVE };

    Block::bordered()
        .title(title)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
}
