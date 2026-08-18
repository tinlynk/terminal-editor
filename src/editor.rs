use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{config::LineEnding, document::Document};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditorView {
    pub cursor: usize,
    pub selection_anchor: Option<usize>,
    pub viewport_line: usize,
    pub viewport_column: usize,
    preferred_column: Option<usize>,
}

impl EditorView {
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.selection_anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn selected_text(&self, document: &Document) -> Option<String> {
        self.selection()
            .map(|range| document.slice(range.start, range.end))
    }

    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    pub fn move_left(&mut self, document: &Document, selecting: bool) {
        self.prepare_selection(selecting);
        if self.cursor == 0 {
            return;
        }
        let line = document.char_to_line(self.cursor);
        let line_start = document.line_to_char(line);
        if self.cursor == line_start {
            self.cursor -= 1;
        } else {
            let prefix = document.slice(line_start, self.cursor);
            let previous = prefix
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(byte, _)| prefix[..byte].chars().count());
            self.cursor = line_start + previous;
        }
        self.preferred_column = None;
    }

    pub fn move_right(&mut self, document: &Document, selecting: bool) {
        self.prepare_selection(selecting);
        if self.cursor >= document.len_chars() {
            return;
        }
        let suffix = document.slice(self.cursor, document.len_chars());
        let advance = suffix
            .graphemes(true)
            .next()
            .map_or(1, |value| value.chars().count());
        self.cursor = (self.cursor + advance).min(document.len_chars());
        self.preferred_column = None;
    }

    pub fn move_home(&mut self, document: &Document, selecting: bool) {
        self.prepare_selection(selecting);
        self.cursor = document.line_to_char(document.char_to_line(self.cursor));
        self.preferred_column = None;
    }

    pub fn move_end(&mut self, document: &Document, selecting: bool) {
        self.prepare_selection(selecting);
        let line = document.char_to_line(self.cursor);
        self.cursor = content_end(document, line);
        self.preferred_column = None;
    }

    pub fn move_vertical(&mut self, document: &Document, delta: isize, selecting: bool) {
        self.prepare_selection(selecting);
        let line = document.char_to_line(self.cursor);
        let desired = self
            .preferred_column
            .unwrap_or_else(|| display_column(document, line, self.cursor));
        self.preferred_column = Some(desired);
        let target_line = line
            .saturating_add_signed(delta)
            .min(document.len_lines().saturating_sub(1));
        self.cursor = char_at_display_column(document, target_line, desired);
    }

    pub fn page(&mut self, document: &Document, rows: usize, down: bool, selecting: bool) {
        let delta = isize::try_from(rows.max(1)).unwrap_or(isize::MAX);
        self.move_vertical(document, if down { delta } else { -delta }, selecting);
        self.ensure_visible(document, rows, 1);
    }

    pub fn insert(&mut self, document: &mut Document, text: &str) {
        if let Some(selection) = self.selection() {
            self.cursor = document.replace(selection.start, selection.end, text);
        } else {
            self.cursor = document.insert(self.cursor, text);
        }
        self.clear_selection();
        self.preferred_column = None;
    }

    pub fn insert_typed(&mut self, document: &mut Document, text: &str) {
        if self.selection().is_some() {
            self.insert(document, text);
            return;
        }
        self.cursor = document.insert_typed(self.cursor, text);
        self.clear_selection();
        self.preferred_column = None;
    }

    pub fn newline(&mut self, document: &mut Document) {
        let ending = match document.line_ending {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        };
        self.insert(document, ending);
    }

    pub fn delete_backward(&mut self, document: &mut Document) {
        if let Some(selection) = self.selection() {
            self.cursor = selection.start;
            document.delete(selection.start, selection.end);
            self.clear_selection();
            return;
        }
        let end = self.cursor;
        self.move_left(document, false);
        document.delete_backward_coalesced(self.cursor, end);
    }

    pub fn delete_forward(&mut self, document: &mut Document) {
        if let Some(selection) = self.selection() {
            self.cursor = selection.start;
            document.delete(selection.start, selection.end);
            self.clear_selection();
            return;
        }
        let start = self.cursor;
        self.move_right(document, false);
        let end = self.cursor;
        self.cursor = start;
        document.delete_forward_coalesced(start, end);
    }

    pub fn select_word_at(&mut self, document: &Document, char_index: usize) {
        let line = document.char_to_line(char_index.min(document.len_chars()));
        let start = document.line_to_char(line);
        let text = document.slice(start, content_end(document, line));
        let local = char_index.saturating_sub(start);
        for (byte, word) in text.split_word_bound_indices() {
            let word_start = text[..byte].chars().count();
            let word_end = word_start + word.chars().count();
            if local >= word_start && local < word_end && !word.trim().is_empty() {
                self.selection_anchor = Some(start + word_start);
                self.cursor = start + word_end;
                return;
            }
        }
        self.cursor = char_index.min(document.len_chars());
        self.clear_selection();
    }

    pub fn select_line_at(&mut self, document: &Document, char_index: usize) {
        let line = document.char_to_line(char_index.min(document.len_chars()));
        self.selection_anchor = Some(document.line_to_char(line));
        self.cursor = document.line_to_char((line + 1).min(document.len_lines()));
    }

    pub fn ensure_visible(&mut self, document: &Document, height: usize, width: usize) {
        let line = document.char_to_line(self.cursor);
        if line < self.viewport_line {
            self.viewport_line = line;
        } else if line >= self.viewport_line.saturating_add(height.max(1)) {
            self.viewport_line = line.saturating_sub(height.saturating_sub(1));
        }
        let column = display_column(document, line, self.cursor);
        if column < self.viewport_column {
            self.viewport_column = column;
        } else if column >= self.viewport_column.saturating_add(width.max(1)) {
            self.viewport_column = column.saturating_sub(width.saturating_sub(1));
        }
    }

    pub fn position_from_viewport(
        &self,
        document: &Document,
        row: usize,
        display_column: usize,
    ) -> usize {
        let line = self
            .viewport_line
            .saturating_add(row)
            .min(document.len_lines().saturating_sub(1));
        char_at_display_column(
            document,
            line,
            self.viewport_column.saturating_add(display_column),
        )
    }

    fn prepare_selection(&mut self, selecting: bool) {
        if selecting {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            self.clear_selection();
        }
    }
}

fn content_end(document: &Document, line: usize) -> usize {
    document.line_content_end(line)
}

pub fn display_column(document: &Document, line: usize, cursor: usize) -> usize {
    let start = document.line_to_char(line);
    document
        .slice(start, cursor.max(start).min(content_end(document, line)))
        .graphemes(true)
        .map(UnicodeWidthStr::width)
        .sum()
}

pub fn char_at_display_column(document: &Document, line: usize, target: usize) -> usize {
    let start = document.line_to_char(line);
    let text = document.slice(start, content_end(document, line));
    let mut width = 0;
    let mut chars = 0;
    for grapheme in text.graphemes(true) {
        let next = width + grapheme.width();
        if next > target {
            break;
        }
        width = next;
        chars += grapheme.chars().count();
    }
    start + chars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LineEnding;

    fn document(text: &str) -> Document {
        Document::new(text, None, LineEnding::Lf, 1024)
    }

    #[test]
    fn horizontal_movement_uses_graphemes() {
        let document = document("a e\u{301} 界🙂");
        let mut view = EditorView {
            cursor: document.len_chars(),
            ..EditorView::default()
        };
        view.move_left(&document, false);
        assert_eq!(view.cursor, document.len_chars() - 1);
        view.move_left(&document, false);
        assert_eq!(view.cursor, document.len_chars() - 2);
        view.move_left(&document, false);
        view.move_left(&document, false);
        assert_eq!(document.slice(view.cursor, view.cursor + 2), "e\u{301}");
    }

    #[test]
    fn vertical_movement_preserves_visual_column() {
        let document = document("a界x\nabcdef\n");
        let mut view = EditorView {
            cursor: 3,
            ..EditorView::default()
        };
        view.move_vertical(&document, 1, false);
        assert_eq!(view.cursor, document.line_to_char(1) + 4);
    }

    #[test]
    fn insertion_replaces_selection() {
        let mut document = document("hello");
        let mut view = EditorView {
            cursor: 4,
            selection_anchor: Some(1),
            ..EditorView::default()
        };
        view.insert(&mut document, "i");
        assert_eq!(document.text(), "hio");
        assert_eq!(view.cursor, 2);
    }

    #[test]
    fn repeated_click_helpers_select_word_and_line() {
        let document = document("alpha beta\nnext\n");
        let mut view = EditorView::default();
        view.select_word_at(&document, 7);
        assert_eq!(view.selected_text(&document).as_deref(), Some("beta"));
        view.select_line_at(&document, 7);
        assert_eq!(
            view.selected_text(&document).as_deref(),
            Some("alpha beta\n")
        );
    }

    #[test]
    fn newline_preserves_crlf_convention() {
        let mut document = Document::new("a\r\nb", None, LineEnding::Lf, 1024);
        let mut view = EditorView {
            cursor: 1,
            ..EditorView::default()
        };
        view.newline(&mut document);
        assert_eq!(document.text(), "a\r\n\r\nb");
    }
}
