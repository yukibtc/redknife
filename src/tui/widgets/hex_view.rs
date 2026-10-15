//! Read-only hex dumps backed by Ratatui's list navigation and scrolling.

use std::fmt::Write;
use std::mem;

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListState, StatefulWidget, Widget};

const BYTES_PER_ROW: usize = 16;

pub struct HexView<'a> {
    rows: List<'a>,
    state: ListState,
    page_size: u16,
}

impl<'a> HexView<'a> {
    pub fn new(bytes: &[u8]) -> Self {
        let rows: List<'a> = List::new(
            bytes
                .chunks(BYTES_PER_ROW)
                .enumerate()
                .map(|(index, chunk)| format_row(index * BYTES_PER_ROW, chunk)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        let selected: Option<usize> = if bytes.is_empty() { None } else { Some(0) };

        Self {
            rows,
            state: ListState::default().with_selected(selected),
            page_size: 1,
        }
    }

    /// Override the current row's style; the default uses reversed colors.
    #[inline]
    pub fn set_highlight_style(&mut self, style: Style) {
        self.rows = mem::take(&mut self.rows).highlight_style(style);
    }

    /// Navigate the dump without exposing or modifying the source bytes.
    pub fn input(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up => self.state.select_previous(),
            KeyCode::Down => self.state.select_next(),
            KeyCode::Home => self.state.select_first(),
            KeyCode::End => self.state.select_last(),
            KeyCode::PageUp => self.state.scroll_up_by(self.page_size),
            KeyCode::PageDown => self.state.scroll_down_by(self.page_size),
            _ => {}
        }
    }
}

impl<'a> Widget for &mut HexView<'a> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        self.page_size = area.height.max(1);

        // List highlights the selected row and keeps it visible while navigating.
        StatefulWidget::render(&self.rows, area, buffer, &mut self.state);
    }
}

/// Keep the ASCII column aligned, including an incomplete final row.
fn format_row(offset: usize, bytes: &[u8]) -> String {
    let mut row: String = format!("{offset:08x}  ");

    for byte in bytes {
        let _ = write!(&mut row, "{byte:02x} ");
    }

    row.push_str(&"   ".repeat(BYTES_PER_ROW - bytes.len()));
    row.push_str(" |");

    for byte in bytes {
        let character: char = if byte.is_ascii_graphic() || *byte == b' ' {
            char::from(*byte)
        } else {
            '.'
        };

        row.push(character);
    }

    row.push('|');

    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_row_format() {
        assert_eq!(
            format_row(16, b"Hello,\0world!   "),
            "00000010  48 65 6c 6c 6f 2c 00 77 6f 72 6c 64 21 20 20 20  |Hello,.world!   |"
        );

        let partial: String = format_row(0, b"A\0");
        assert!(partial.starts_with("00000000  41 00 "));
        assert!(partial.ends_with(" |A.|"));
    }

    #[test]
    fn test_current_row_is_highlighted() {
        let mut view: HexView = HexView::new(&[b'x'; 48]);
        let area: Rect = Rect::new(0, 0, 80, 3);
        let mut buffer: Buffer = Buffer::empty(area);
        (&mut view).render(area, &mut buffer);

        assert!(buffer[(0, 0)].modifier.contains(Modifier::REVERSED));

        view.input(KeyCode::Down);
        buffer.reset();
        (&mut view).render(area, &mut buffer);

        assert!(!buffer[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert!(buffer[(0, 1)].modifier.contains(Modifier::REVERSED));
        assert!(buffer[(79, 1)].modifier.contains(Modifier::REVERSED));
    }
}
