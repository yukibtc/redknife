use std::mem;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListState, StatefulWidget};

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct HorizontalScroll {
    pub(crate) offset: usize,
    width: usize,
    max: usize,
}

impl HorizontalScroll {
    pub(crate) fn resize(&mut self, width: u16, content_width: usize) {
        self.width = visible_width(width);
        self.max = content_width.saturating_sub(self.width);
        self.offset = self.offset.min(self.max);
    }

    pub(crate) fn input(&mut self, key: KeyEvent) -> bool {
        let step: usize = if key.modifiers.contains(KeyModifiers::SHIFT) {
            self.width.max(1)
        } else {
            1
        };

        match key.code {
            KeyCode::Left => self.offset = self.offset.saturating_sub(step),
            KeyCode::Right => self.offset = self.offset.saturating_add(step).min(self.max),
            _ => return false,
        }

        true
    }
}

#[inline]
fn visible_width(width: u16) -> usize {
    // Narrow areas still display data, larger ones reserve two overflow indicators.
    usize::from(if width >= 4 { width - 2 } else { width })
}

/// Extract only the visible columns, keeping grapheme clusters intact. A partially
/// visible wide character occupies blank cells until it can be displayed in full.
fn columns(text: &str, offset: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    if text.is_ascii() {
        return text
            .get(offset..offset.saturating_add(width).min(text.len()))
            .unwrap_or_default()
            .to_owned();
    }

    let line: Line = Line::raw(text);

    let mut result: String = String::new();
    let mut column: usize = 0;

    let end: usize = offset.saturating_add(width);

    for grapheme in line.styled_graphemes(Style::default()) {
        let next: usize = column + Span::raw(grapheme.symbol).width();

        if column >= end {
            break;
        }

        if column >= offset && next <= end {
            result.push_str(grapheme.symbol);
        } else if next > offset {
            result.push_str(&" ".repeat(next.min(end) - column.max(offset)));
        }

        column = next;
    }

    result
}

pub(crate) fn clipped_line(text: &str, offset: usize, width: u16) -> String {
    let available: usize = visible_width(width);
    let content: String = columns(text, offset, available);

    if width < 4 {
        return content;
    }

    let padding: usize = available.saturating_sub(Line::raw(content.as_str()).width());

    format!(
        "{}{}{}{}",
        if offset > 0 { '‹' } else { ' ' },
        content,
        " ".repeat(padding),
        if Line::raw(text).width() > offset.saturating_add(available) {
            '›'
        } else {
            ' '
        },
    )
}

pub(crate) fn navigate(state: &mut ListState, key: KeyCode, page_size: u16) {
    match key {
        KeyCode::Up => state.select_previous(),
        KeyCode::Down => state.select_next(),
        KeyCode::Home => state.select_first(),
        KeyCode::End => state.select_last(),
        KeyCode::PageUp => state.scroll_up_by(page_size),
        KeyCode::PageDown => state.scroll_down_by(page_size),
        _ => {}
    }
}

#[derive(Default)]
pub(crate) struct ScrollableLines<'a> {
    lines: Vec<String>,
    max_width: usize,
    rows: List<'a>,
    viewport: Option<(u16, usize)>,
    pub(crate) horizontal: HorizontalScroll,
    page_size: u16,
}

impl<'a> ScrollableLines<'a> {
    pub(crate) fn new(lines: Vec<String>) -> Self {
        let max_width: usize = lines
            .iter()
            .map(|line| Line::raw(line.as_str()).width())
            .max()
            .unwrap_or(0);

        Self {
            lines,
            max_width,
            page_size: 1,
            ..Self::default()
        }
    }

    pub(crate) fn input(&mut self, key: KeyEvent, state: &mut ListState) {
        if !self.horizontal.input(key) {
            navigate(state, key.code, self.page_size);
        }
    }

    pub(crate) fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        state: &mut ListState,
        highlight: Style,
    ) {
        if area.is_empty() {
            return;
        }

        self.page_size = area.height;

        self.horizontal.resize(area.width, self.max_width);

        let viewport: (u16, usize) = (area.width, self.horizontal.offset);

        if self.viewport != Some(viewport) {
            self.rows = List::new(
                self.lines
                    .iter()
                    .map(|line| clipped_line(line, self.horizontal.offset, area.width)),
            );
            self.viewport = Some(viewport);
        }

        self.rows = mem::take(&mut self.rows).highlight_style(highlight);

        StatefulWidget::render(&self.rows, area, buffer, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_columns_preserve_graphemes_and_alignment() {
        assert_eq!(columns("a中e\u{301}🙂z", 1, 5), "中e\u{301}🙂");
        assert_eq!(columns("a中e\u{301}🙂z", 2, 4), " e\u{301}🙂");
        assert_eq!(columns("中z", 0, 1), " ");
        assert_eq!(columns("é", 10, 2), "");
        assert_eq!(columns("abc", usize::MAX, 2), "");
    }

    #[test]
    fn long_rows_remain_reachable_beyond_u16_coordinates() {
        let mut lines = ScrollableLines::new(vec![format!("{}TAIL", "x".repeat(70_000))]);
        let mut state = ListState::default().with_selected(Some(0));
        let area = Rect::new(0, 0, 20, 1);

        let mut buffer = Buffer::empty(area);

        lines.render(area, &mut buffer, &mut state, Style::default());

        assert_eq!(buffer[(19, 0)].symbol(), "›");

        for _ in 0..4000 {
            lines.input(
                KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
                &mut state,
            );
        }

        buffer.reset();

        lines.render(area, &mut buffer, &mut state, Style::default());

        let row: String = buffer.content.iter().map(|cell| cell.symbol()).collect();

        assert!(row.ends_with("TAIL "), "{row}");
        assert_eq!(buffer[(0, 0)].symbol(), "‹");
        assert!(lines.horizontal.offset > usize::from(u16::MAX));

        // Resize without losing the selected row or scrolling into blank space.
        let wide = Rect::new(0, 0, 100, 1);
        lines.render(wide, &mut Buffer::empty(wide), &mut state, Style::default());

        assert_eq!(lines.horizontal.offset, 70_004 - 98);

        lines.render(
            Rect::default(),
            &mut Buffer::empty(Rect::default()),
            &mut state,
            Style::default(),
        );

        assert_eq!(state.selected(), Some(0));
    }
}
