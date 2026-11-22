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
    pub viewport_wrap_segment: usize,
    preferred_column: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct WrappedPosition {
    pub line: usize,
    pub segment: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WrappedRow {
    pub position: WrappedPosition,
    pub start: usize,
    pub end: usize,
    pub display_start: usize,
}

impl EditorView {
    pub fn maximum_viewport_line(document: &Document, height: usize) -> usize {
        document.len_lines().saturating_sub(height.max(1))
    }

    pub fn clamped_viewport_line(&self, document: &Document, height: usize) -> usize {
        self.viewport_line
            .min(Self::maximum_viewport_line(document, height))
    }

    pub fn clamp_viewport(&mut self, document: &Document, height: usize) {
        self.viewport_line = self.clamped_viewport_line(document, height);
    }

    pub fn scroll_vertical(&mut self, document: &Document, height: usize, delta: isize) {
        self.viewport_line = self
            .viewport_line
            .saturating_add_signed(delta)
            .min(Self::maximum_viewport_line(document, height));
    }

    pub fn maximum_horizontal_offset(document: &Document, width: usize) -> usize {
        document
            .maximum_line_display_width()
            .saturating_sub(width.max(1))
    }

    pub fn clamp_horizontal(&mut self, document: &Document, width: usize) {
        self.viewport_column = self
            .viewport_column
            .min(Self::maximum_horizontal_offset(document, width));
    }

    pub fn scroll_horizontal(&mut self, document: &Document, width: usize, delta: isize) {
        let maximum = Self::maximum_horizontal_offset(document, width);
        self.viewport_column = self
            .viewport_column
            .saturating_add_signed(delta)
            .min(maximum);
    }

    pub fn scroll_vertical_wrapped(
        &mut self,
        document: &Document,
        width: usize,
        height: usize,
        delta: isize,
    ) {
        let current = WrappedPosition {
            line: self.viewport_line,
            segment: self.viewport_wrap_segment,
        };
        let mut next = advance_wrapped_position(document, current, width, delta);
        if delta > 0 {
            let remaining = wrapped_positions(document, next, width, height.max(1));
            if remaining.len() < height.max(1)
                && let Some(last) = remaining.last().copied()
            {
                next = advance_wrapped_position(
                    document,
                    last,
                    width,
                    -isize::try_from(height.saturating_sub(1)).unwrap_or(isize::MAX),
                );
            }
        }
        self.viewport_line = next.line;
        self.viewport_wrap_segment = next.segment;
        self.viewport_column = 0;
    }

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
            if self.cursor > 0
                && document.char_at(self.cursor - 1) == Some('\r')
                && document.char_at(self.cursor) == Some('\n')
            {
                self.cursor -= 1;
            }
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
        if self.cursor > 0
            && document.char_at(self.cursor - 1) == Some('\r')
            && document.char_at(self.cursor) == Some('\n')
        {
            self.cursor += 1;
            self.preferred_column = None;
            return;
        }
        if document.char_at(self.cursor) == Some('\r')
            && self.cursor + 1 < document.len_chars()
            && document.char_at(self.cursor + 1) == Some('\n')
        {
            self.cursor += 2;
            self.preferred_column = None;
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

    pub fn move_vertical_wrapped(
        &mut self,
        document: &Document,
        width: usize,
        delta: isize,
        selecting: bool,
    ) {
        self.prepare_selection(selecting);
        let current = wrapped_position_for_cursor(document, self.cursor, width);
        let display_column = display_column(document, current.line, self.cursor);
        let current_row = wrapped_row(document, current, width);
        let desired = self
            .preferred_column
            .unwrap_or_else(|| display_column.saturating_sub(current_row.display_start));
        self.preferred_column = Some(desired);
        let target = advance_wrapped_position(document, current, width, delta);
        let target_row = wrapped_row(document, target, width);
        self.cursor = char_at_display_column(
            document,
            target.line,
            target_row.display_start.saturating_add(desired),
        );
    }

    pub fn page(&mut self, document: &Document, rows: usize, down: bool, selecting: bool) {
        let delta = isize::try_from(rows.max(1)).unwrap_or(isize::MAX);
        self.move_vertical(document, if down { delta } else { -delta }, selecting);
        self.ensure_visible(document, rows, 1);
    }

    pub fn page_wrapped(
        &mut self,
        document: &Document,
        width: usize,
        rows: usize,
        down: bool,
        selecting: bool,
    ) {
        let delta = isize::try_from(rows.max(1)).unwrap_or(isize::MAX);
        self.move_vertical_wrapped(
            document,
            width,
            if down { delta } else { -delta },
            selecting,
        );
        self.ensure_visible_wrapped(document, rows, width);
    }

    pub fn insert(&mut self, document: &mut Document, text: &str) {
        self.normalize_crlf_cursor(document);
        if let Some(selection) = self.selection() {
            self.cursor = document.replace(selection.start, selection.end, text);
        } else {
            self.cursor = document.insert(self.cursor, text);
        }
        self.clear_selection();
        self.preferred_column = None;
    }

    pub fn insert_typed(&mut self, document: &mut Document, text: &str) {
        self.normalize_crlf_cursor(document);
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
        self.normalize_crlf_cursor(document);
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
        self.normalize_crlf_cursor(document);
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
        self.clamp_viewport(document, height);
    }

    pub fn ensure_visible_wrapped(&mut self, document: &Document, height: usize, width: usize) {
        let width = width.max(1);
        let height = height.max(1);
        let mut viewport = WrappedPosition {
            line: self.viewport_line,
            segment: self.viewport_wrap_segment,
        };
        viewport = advance_wrapped_position(document, viewport, width, 0);
        let cursor = wrapped_position_for_cursor(document, self.cursor, width);
        let visible = wrapped_positions(document, viewport, width, height)
            .into_iter()
            .any(|position| position == cursor);
        if cursor < viewport {
            viewport = cursor;
        } else if !visible {
            viewport = advance_wrapped_position(
                document,
                cursor,
                width,
                -isize::try_from(height.saturating_sub(1)).unwrap_or(isize::MAX),
            );
        }
        self.viewport_line = viewport.line;
        self.viewport_wrap_segment = viewport.segment;
        self.viewport_column = 0;
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

    pub fn position_from_wrapped_viewport(
        &self,
        document: &Document,
        row: usize,
        display_column: usize,
        width: usize,
    ) -> usize {
        let start = WrappedPosition {
            line: self.viewport_line,
            segment: self.viewport_wrap_segment,
        };
        let position = advance_wrapped_position(
            document,
            start,
            width,
            isize::try_from(row).unwrap_or(isize::MAX),
        );
        let row = wrapped_row(document, position, width);
        char_at_display_column(
            document,
            position.line,
            row.display_start.saturating_add(display_column),
        )
    }

    fn prepare_selection(&mut self, selecting: bool) {
        if selecting {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            self.clear_selection();
        }
    }

    fn normalize_crlf_cursor(&mut self, document: &Document) {
        if self.cursor > 0
            && self.cursor < document.len_chars()
            && document.char_at(self.cursor - 1) == Some('\r')
            && document.char_at(self.cursor) == Some('\n')
        {
            self.cursor -= 1;
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

pub fn line_display_width(document: &Document, line: usize) -> usize {
    document.line_display_width(line)
}

pub fn wrapped_row_count(document: &Document, line: usize, width: usize) -> usize {
    wrapped_line_boundaries(document, line, width)
        .len()
        .saturating_sub(1)
        .max(1)
}

pub fn wrapped_position_for_cursor(
    document: &Document,
    cursor: usize,
    width: usize,
) -> WrappedPosition {
    let line = document.char_to_line(cursor);
    let boundaries = wrapped_line_boundaries(document, line, width);
    let segment = boundaries
        .windows(2)
        .position(|range| cursor >= range[0] && cursor < range[1])
        .unwrap_or_else(|| boundaries.len().saturating_sub(2));
    WrappedPosition { line, segment }
}

pub fn wrapped_row(document: &Document, position: WrappedPosition, width: usize) -> WrappedRow {
    let width = width.max(1);
    let line = position.line.min(document.len_lines().saturating_sub(1));
    let segment = position
        .segment
        .min(wrapped_row_count(document, line, width).saturating_sub(1));
    let boundaries = wrapped_line_boundaries(document, line, width);
    let start = boundaries[segment];
    let end = boundaries[segment + 1];
    WrappedRow {
        position: WrappedPosition { line, segment },
        start,
        end,
        display_start: display_column(document, line, start),
    }
}

fn wrapped_line_boundaries(document: &Document, line: usize, width: usize) -> Vec<usize> {
    let width = width.max(1);
    let start = document.line_to_char(line);
    let end = content_end(document, line);
    let text = document.slice(start, end);
    let mut boundaries = vec![start];
    let mut row_width = 0usize;
    let mut char_offset = 0usize;
    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        if row_width > 0 && row_width.saturating_add(grapheme_width) > width {
            boundaries.push(start + char_offset);
            row_width = 0;
        }
        row_width = row_width.saturating_add(grapheme_width);
        char_offset += grapheme.chars().count();
    }
    if boundaries.last().copied() != Some(end) || boundaries.len() == 1 {
        boundaries.push(end);
    }
    boundaries
}

pub fn advance_wrapped_position(
    document: &Document,
    mut position: WrappedPosition,
    width: usize,
    delta: isize,
) -> WrappedPosition {
    position.line = position.line.min(document.len_lines().saturating_sub(1));
    position.segment = position
        .segment
        .min(wrapped_row_count(document, position.line, width).saturating_sub(1));
    let mut remaining = delta.unsigned_abs();
    if delta >= 0 {
        while remaining > 0 {
            let count = wrapped_row_count(document, position.line, width);
            let available = count.saturating_sub(position.segment + 1);
            if available > 0 {
                let step = available.min(remaining);
                position.segment += step;
                remaining -= step;
            } else if position.line + 1 < document.len_lines() {
                position.line += 1;
                position.segment = 0;
                remaining -= 1;
            } else {
                break;
            }
        }
    } else {
        while remaining > 0 {
            if position.segment > 0 {
                let step = position.segment.min(remaining);
                position.segment -= step;
                remaining -= step;
            } else if position.line > 0 {
                position.line -= 1;
                position.segment = wrapped_row_count(document, position.line, width) - 1;
                remaining -= 1;
            } else {
                break;
            }
        }
    }
    position
}

pub fn wrapped_positions(
    document: &Document,
    start: WrappedPosition,
    width: usize,
    limit: usize,
) -> Vec<WrappedPosition> {
    wrapped_rows(document, start, width, limit)
        .into_iter()
        .map(|row| row.position)
        .collect()
}

pub fn wrapped_rows(
    document: &Document,
    start: WrappedPosition,
    width: usize,
    limit: usize,
) -> Vec<WrappedRow> {
    let mut rows = Vec::with_capacity(limit);
    let start = advance_wrapped_position(document, start, width, 0);
    let mut line = start.line;
    let mut first_segment = start.segment;
    while line < document.len_lines() && rows.len() < limit {
        let boundaries = wrapped_line_boundaries(document, line, width);
        for segment in first_segment..boundaries.len().saturating_sub(1) {
            let start = boundaries[segment];
            rows.push(WrappedRow {
                position: WrappedPosition { line, segment },
                start,
                end: boundaries[segment + 1],
                display_start: display_column(document, line, start),
            });
            if rows.len() == limit {
                break;
            }
        }
        line += 1;
        first_segment = 0;
    }
    rows
}

pub fn maximum_wrapped_viewport(
    document: &Document,
    width: usize,
    height: usize,
) -> WrappedPosition {
    let line = document.len_lines().saturating_sub(1);
    let last = WrappedPosition {
        line,
        segment: wrapped_row_count(document, line, width).saturating_sub(1),
    };
    advance_wrapped_position(
        document,
        last,
        width,
        -isize::try_from(height.saturating_sub(1)).unwrap_or(isize::MAX),
    )
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
    use crate::document::EncodingState;

    fn document(text: &str) -> Document {
        Document::new(text, None, LineEnding::Lf, 1024, EncodingState::Utf8)
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
    fn crlf_navigation_and_deletion_treat_line_ending_as_one_boundary() {
        let mut first_document = document("a\r\nb");
        let mut view = EditorView {
            cursor: 1,
            ..EditorView::default()
        };
        view.move_right(&first_document, false);
        assert_eq!(view.cursor, 3);
        view.move_left(&first_document, false);
        assert_eq!(view.cursor, 1);

        view.cursor = 3;
        view.delete_backward(&mut first_document);
        assert_eq!(first_document.text(), "ab");
        assert_eq!(view.cursor, 1);

        let mut second_document = document("a\r\nb");
        let mut view = EditorView {
            cursor: 1,
            ..EditorView::default()
        };
        view.delete_forward(&mut second_document);
        assert_eq!(second_document.text(), "ab");
        assert_eq!(view.cursor, 1);
    }

    #[test]
    fn crlf_home_end_and_typing_do_not_enter_line_ending_bytes() {
        let mut document = document("a\r\nb");
        let mut view = EditorView {
            cursor: 1,
            ..EditorView::default()
        };
        view.move_end(&document, false);
        assert_eq!(view.cursor, 1);
        view.insert_typed(&mut document, "x");
        assert_eq!(document.text(), "ax\r\nb");

        view.move_home(&document, false);
        assert_eq!(view.cursor, 0);
        view.cursor = 3;
        view.insert(&mut document, "y");
        assert_eq!(view.cursor, 3);
        assert_eq!(document.text(), "axy\r\nb");
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
        let mut document = Document::new("a\r\nb", None, LineEnding::Lf, 1024, EncodingState::Utf8);
        let mut view = EditorView {
            cursor: 1,
            ..EditorView::default()
        };
        view.newline(&mut document);
        assert_eq!(document.text(), "a\r\n\r\nb");
    }

    #[test]
    fn vertical_scroll_stops_at_the_last_full_viewport() {
        let document = document("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");
        let mut view = EditorView::default();

        view.scroll_vertical(&document, 4, 100);
        assert_eq!(view.viewport_line, 6);
        view.scroll_vertical(&document, 4, 1);
        assert_eq!(view.viewport_line, 6);
        view.scroll_vertical(&document, 4, -2);
        assert_eq!(view.viewport_line, 4);
    }

    #[test]
    fn short_and_trailing_newline_documents_do_not_scroll_past_their_content() {
        for text in ["", "one line", "one line\n"] {
            let document = document(text);
            let mut view = EditorView {
                viewport_line: usize::MAX,
                ..EditorView::default()
            };

            view.clamp_viewport(&document, 3);
            assert_eq!(view.viewport_line, 0);
        }
    }

    #[test]
    fn taller_viewport_clamps_a_previously_valid_offset() {
        let document = document("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");
        let mut view = EditorView {
            viewport_line: 7,
            ..EditorView::default()
        };

        view.clamp_viewport(&document, 6);
        assert_eq!(view.viewport_line, 4);
        view.clamp_viewport(&document, 1);
        assert_eq!(view.viewport_line, 4);
        assert_eq!(EditorView::maximum_viewport_line(&document, 0), 9);
        assert_eq!(EditorView::maximum_viewport_line(&document, 1), 9);
    }

    #[test]
    fn horizontal_scroll_is_bounded_without_moving_the_cursor() {
        let document = document("abcdefghijklmnopqrstuvwxyz");
        let mut view = EditorView::default();

        view.scroll_horizontal(&document, 10, 100);
        assert_eq!(view.viewport_column, 16);
        assert_eq!(view.cursor, 0);
        view.scroll_horizontal(&document, 10, -100);
        assert_eq!(view.viewport_column, 0);
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn horizontal_scroll_uses_the_longest_document_line() {
        let document = document("short\nmedium line\nout of view is much longer");
        let mut view = EditorView::default();

        view.scroll_horizontal(&document, 10, 100);

        assert_eq!(view.viewport_column, 16);
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn wrapping_preserves_wide_and_combining_graphemes() {
        let document = document("aaaa界bbbe\u{301}z");
        let positions = wrapped_positions(&document, WrappedPosition::default(), 5, 10);
        let rows = positions
            .into_iter()
            .map(|position| {
                let row = wrapped_row(&document, position, 5);
                document.slice(row.start, row.end)
            })
            .collect::<Vec<_>>();

        assert_eq!(rows, vec!["aaaa", "界bbb", "e\u{301}z"]);
    }

    #[test]
    fn wrapped_vertical_movement_and_mouse_mapping_use_visual_rows() {
        let document = document("abcdefghijklmno");
        let mut view = EditorView {
            cursor: 2,
            ..EditorView::default()
        };

        view.move_vertical_wrapped(&document, 5, 1, false);
        assert_eq!(view.cursor, 7);
        view.move_vertical_wrapped(&document, 5, 1, false);
        assert_eq!(view.cursor, 12);
        assert_eq!(view.position_from_wrapped_viewport(&document, 2, 1, 5), 11);
    }

    #[test]
    fn wrapped_vertical_scroll_stops_at_the_last_full_viewport() {
        let document = document("abcdefghijklmnopqrst");
        let mut view = EditorView::default();

        view.scroll_vertical_wrapped(&document, 5, 2, 100);
        assert_eq!(view.viewport_line, 0);
        assert_eq!(view.viewport_wrap_segment, 2);
        view.scroll_vertical_wrapped(&document, 5, 2, -100);
        assert_eq!(view.viewport_wrap_segment, 0);
    }
}
