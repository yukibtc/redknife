//! Byte-preserving HTTP documents and their optional text-editing representation.

use std::borrow::Cow;
use std::str::{self, Bytes};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui_textarea::{CursorMove, Scrolling, TextArea};

use crate::tui::widgets::hex_view::HexView;

pub(super) struct Document {
    editor: TextArea<'static>,
    editable: bool,
    // The original bytes remain authoritative until a lossless text edit changes them.
    original_raw: Vec<u8>,
    original_text: Text,
    // Keep formatted rows and navigation until an edit invalidates their bytes.
    hex_view: Option<HexView<'static>>,
}

impl Document {
    /// Prepare a text editor, retaining binary or unsafe text as read-only bytes.
    pub(super) fn new(raw: Vec<u8>) -> Self {
        let text: Option<Text> = Text::parse(&raw);
        let editable: bool = text.is_some();

        let text: Text = text.unwrap_or_else(Text::empty_with_lf);

        let mut editor: TextArea<'static> = TextArea::new(text.lines.clone());
        editor.set_cursor_line_style(Style::default());
        editor.set_cursor_style(Style::default());

        let hex_view: Option<HexView> = if editable {
            None
        } else {
            Some(HexView::new(&raw))
        };

        Self {
            editor,
            editable,
            original_raw: raw,
            original_text: text,
            hex_view,
        }
    }

    #[inline]
    pub(super) fn is_editable(&self) -> bool {
        self.editable
    }

    #[inline]
    pub(super) fn editor(&self) -> &TextArea<'static> {
        &self.editor
    }

    #[inline]
    pub(super) fn set_cursor_style(&mut self, style: Style) {
        self.editor.set_cursor_style(style);
    }

    /// Prepare the dump lazily, reusing it across focus and raw/hex changes.
    pub(super) fn hex_view(&mut self) -> &mut HexView<'static> {
        match self.hex_view {
            Some(ref mut view) => view,
            None => {
                let view: HexView<'static> = HexView::new(&self.bytes());
                self.hex_view.insert(view)
            }
        }
    }

    pub(super) fn bytes(&self) -> Cow<'_, [u8]> {
        if !self.editable || self.editor.lines() == self.original_text.lines {
            return Cow::Borrowed(&self.original_raw);
        }

        let lines: &[String] = self.editor.lines();

        match lines.iter().position(String::is_empty) {
            Some(separator) => {
                let head: String = lines[..separator].join(LineEnding::CrLf.as_str());
                let body: String =
                    lines[separator + 1..].join(self.original_text.body_line_ending.as_str());

                Cow::Owned(format!("{head}\r\n\r\n{body}").into_bytes())
            }
            None => Cow::Owned(lines.join(LineEnding::CrLf.as_str()).into_bytes()),
        }
    }

    /// Navigate any document
    pub(super) fn input(&mut self, key: KeyEvent, allow_edit: bool) {
        if allow_edit && self.editable {
            // TextArea reports content changes, including undo and redo, separately from navigation.
            if self.editor.input(key) {
                self.hex_view = None;
            }

            return;
        }

        // Allow scrolling without allowing the read-only view to become request data.
        match key.code {
            KeyCode::Up => self.editor.move_cursor(CursorMove::Up),
            KeyCode::Down => self.editor.move_cursor(CursorMove::Down),
            KeyCode::Left => self.editor.move_cursor(CursorMove::Back),
            KeyCode::Right => self.editor.move_cursor(CursorMove::Forward),
            KeyCode::Home => self.editor.move_cursor(CursorMove::Head),
            KeyCode::End => self.editor.move_cursor(CursorMove::End),
            KeyCode::PageUp => self.editor.scroll(Scrolling::PageUp),
            KeyCode::PageDown => self.editor.scroll(Scrolling::PageDown),
            _ => {}
        }
    }
}

/// Line endings preserved when reconstructing an edited HTTP body.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    /// Line feed (`\n`).
    Lf,
    /// Carriage return followed by line feed (`\r\n`).
    CrLf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }

    /// Reject mixed or bare CR endings, which a text-area edit would otherwise lose.
    fn detect(body: &str) -> Option<Self> {
        let mut bytes: Bytes<'_> = body.bytes();
        let mut detected: Option<Self> = None;

        while let Some(byte) = bytes.next() {
            let current: Self = match byte {
                b'\r' => {
                    if bytes.next() != Some(b'\n') {
                        return None;
                    }

                    Self::CrLf
                }
                b'\n' => Self::Lf,
                _ => continue,
            };

            if let Some(previous) = detected {
                if previous != current {
                    return None;
                }
            }

            detected = Some(current);
        }

        // A body without line endings uses LF for any lines inserted later.
        Some(detected.unwrap_or(Self::Lf))
    }
}

/// Text lines and the body format needed to reconstruct an edited document.
struct Text {
    lines: Vec<String>,
    body_line_ending: LineEnding,
}

impl Text {
    fn empty_with_lf() -> Self {
        Self {
            lines: vec![String::new()],
            body_line_ending: LineEnding::Lf,
        }
    }

    /// Only text which can round-trip through TextArea may be edited.
    fn parse(raw: &[u8]) -> Option<Self> {
        let text: &str = str::from_utf8(raw).ok()?;

        for character in text.chars() {
            if is_unsupported_character(character) {
                return None;
            }
        }

        let body: &str = request_body(text);
        let body_line_ending: LineEnding = LineEnding::detect(body)?;

        let normalized: String = text.replace("\r\n", "\n");

        if normalized.contains('\r') {
            // Bare CR in the head cannot be represented without changing its bytes either.
            return None;
        }

        // split preserves the final empty line; str::lines would discard it.
        let lines: Vec<String> = normalized.split('\n').map(str::to_owned).collect();

        Some(Self {
            lines,
            body_line_ending,
        })
    }
}

/// Exclude controls and invisible direction changes which obscure the displayed bytes.
fn is_unsupported_character(character: char) -> bool {
    match character {
        '\r' | '\n' | '\t' => false,
        // Arabic, left-to-right, and right-to-left direction marks.
        '\u{061c}' | '\u{200e}' | '\u{200f}' => true,
        // Bidirectional embeddings, overrides, and their closing marker.
        '\u{202a}'..='\u{202e}' => true,
        // Bidirectional isolation markers.
        '\u{2066}'..='\u{2069}' => true,
        _ => character.is_control(),
    }
}

/// Locate the body using the earliest supported HTTP separator.
fn request_body(text: &str) -> &str {
    let mut first_separator: usize = text.len();
    let mut body_start: usize = text.len();

    // Use the first separator: an LF header may precede CRLF sequences in the body.
    for separator in ["\r\n\r\n", "\n\n"] {
        let Some(position) = text.find(separator) else {
            continue;
        };

        if position < first_separator {
            first_separator = position;
            body_start = position + separator.len();
        }
    }

    &text[body_start..]
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    use super::*;

    fn hex_row(document: &mut Document) -> String {
        let area: Rect = Rect::new(0, 0, 80, 1);
        let mut buffer: Buffer = Buffer::empty(area);
        document.hex_view().render(area, &mut buffer);

        buffer.content.iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn test_original_bytes_are_preserved() {
        let raw: &[u8] = b"POST / HTTP/1.1\nX-Test: a\nX-Test: b\n\nbody\r\n\r\n";
        let document: Document = Document::new(raw.to_vec());

        assert_eq!(document.bytes(), raw);
    }

    #[test]
    fn test_edit_preserves_body_line_endings() {
        for (raw, expected) in [
            (
                "POST / HTTP/1.1\n\ncaffè\r\n\r\n",
                "XPOST / HTTP/1.1\r\n\r\ncaffè\r\n\r\n",
            ),
            (
                "POST / HTTP/1.1\r\n\r\nbody\n\n",
                "XPOST / HTTP/1.1\r\n\r\nbody\n\n",
            ),
            (
                "POST / HTTP/1.1\r\n\r\nbody\r\n\r\n",
                "XPOST / HTTP/1.1\r\n\r\nbody\r\n\r\n",
            ),
        ] {
            let mut document: Document = Document::new(raw.as_bytes().to_vec());
            document.input(KeyEvent::from(KeyCode::Char('X')), true);

            assert_eq!(document.bytes(), expected.as_bytes());
        }
    }

    #[test]
    fn test_unrepresentable_bytes_are_read_only() {
        for raw in [
            b"POST / HTTP/1.1\r\n\r\n\xff\x00".as_slice(),
            b"POST / HTTP/1.1\r\n\r\n\x1b[31mred",
            b"POST / HTTP/1.1\r\n\r\none\r\ntwo\n",
            b"GET / HTTP/1.1\rX-Test: a\r\n\r\n",
            "GET / HTTP/1.1\r\n\r\n\u{202e}hidden".as_bytes(),
        ] {
            let mut document: Document = Document::new(raw.to_vec());
            document.input(KeyEvent::from(KeyCode::Char('X')), true);

            assert!(!document.is_editable());
            assert_eq!(document.bytes(), raw);
        }
    }

    #[test]
    fn test_incomplete_request_can_be_edited() {
        let mut document: Document = Document::new(b"GET /".to_vec());
        document.input(KeyEvent::from(KeyCode::Char('é')), true);

        assert_eq!(document.bytes(), "éGET /".as_bytes());
    }

    #[test]
    fn test_hex_follows_edit_and_undo() {
        let raw: &[u8] = b"GET / HTTP/1.1\r\n\r\n";
        let mut document: Document = Document::new(raw.to_vec());
        let original_hex: String = hex_row(&mut document);

        document.input(KeyEvent::from(KeyCode::Char('X')), true);

        assert_eq!(document.bytes(), b"XGET / HTTP/1.1\r\n\r\n".as_slice());
        assert_ne!(hex_row(&mut document), original_hex);

        document.input(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            true,
        );

        assert_eq!(document.bytes(), raw);
        assert_eq!(hex_row(&mut document), original_hex);
    }
}
