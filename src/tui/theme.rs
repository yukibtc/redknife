//! Shared colors and selection style for all TUI screens.

use ratatui::style::{Color, Style};

/// Main accent for headings, active borders, selections, and cursors.
pub(super) const MAIN: Color = Color::LightRed;

/// Text displayed on the main accent background.
pub(super) const ON_MAIN: Color = Color::Black;

/// Border color for panes which do not have focus.
pub(super) const INACTIVE: Color = Color::DarkGray;

/// Highlight a selection
pub(super) fn selection() -> Style {
    Style::default().fg(ON_MAIN).bg(MAIN)
}
