use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::config::LineEnding;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DocumentId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextChange {
    pub start: usize,
    pub removed_chars: usize,
    pub inserted_chars: usize,
}

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
    before_revision: u64,
    after_revision: u64,
    kind: EditKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditKind {
    Standalone,
    Typing,
    DeleteBackward,
    DeleteForward,
}

impl Edit {
    fn bytes(&self) -> usize {
        self.removed.len().saturating_add(self.inserted.len())
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    text: Rope,
    line_width_frequencies: BTreeMap<usize, usize>,
    pub path: Option<PathBuf>,
    pub line_ending: LineEnding,
    revision: u64,
    saved_revision: u64,
    next_revision: u64,
    undo: VecDeque<Edit>,
    redo: Vec<Edit>,
    history_bytes: usize,
    history_limit: usize,
    last_change: Option<TextChange>,
}

impl Document {
    pub fn new(
        text: &str,
        path: Option<PathBuf>,
        default_line_ending: LineEnding,
        history_limit: usize,
    ) -> Self {
        let line_ending = detect_line_ending(text).unwrap_or(default_line_ending);
        Self {
            text: Rope::from_str(text),
            line_width_frequencies: initial_line_width_frequencies(text),
            path,
            line_ending,
            revision: 0,
            saved_revision: 0,
            next_revision: 1,
            undo: VecDeque::new(),
            redo: Vec::new(),
            history_bytes: 0,
            history_limit,
            last_change: None,
        }
    }

    pub fn len_chars(&self) -> usize {
        self.text.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.text.len_lines()
    }

    pub fn is_empty(&self) -> bool {
        self.text.len_chars() == 0
    }

    pub fn set_history_limit(&mut self, history_limit: usize) {
        self.history_limit = history_limit;
        self.enforce_history_limit();
    }

    pub fn is_modified(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
        self.break_coalescing();
    }

    pub fn last_change(&self) -> Option<TextChange> {
        self.last_change
    }

    pub fn text(&self) -> String {
        self.text.to_string()
    }

    pub fn slice(&self, start: usize, end: usize) -> String {
        self.text
            .slice(start.min(self.len_chars())..end.min(self.len_chars()))
            .to_string()
    }

    pub fn line(&self, line: usize) -> String {
        self.text
            .line(line.min(self.len_lines().saturating_sub(1)))
            .to_string()
    }

    pub fn line_to_char(&self, line: usize) -> usize {
        self.text.line_to_char(line.min(self.len_lines()))
    }

    pub fn line_content_end(&self, line: usize) -> usize {
        let start = self.line_to_char(line);
        let mut end = self.line_to_char((line + 1).min(self.len_lines()));
        if end > start && self.text.char(end - 1) == '\n' {
            end -= 1;
            if end > start && self.text.char(end - 1) == '\r' {
                end -= 1;
            }
        }
        end
    }

    pub fn line_display_width(&self, line: usize) -> usize {
        display_width(&self.slice(self.line_to_char(line), self.line_content_end(line)))
    }

    pub fn maximum_line_display_width(&self) -> usize {
        self.line_width_frequencies
            .last_key_value()
            .map_or(0, |(width, _)| *width)
    }

    pub fn char_to_line(&self, char_index: usize) -> usize {
        self.text.char_to_line(char_index.min(self.len_chars()))
    }

    pub fn insert(&mut self, char_index: usize, text: &str) -> usize {
        self.replace(char_index, char_index, text)
    }

    pub fn delete(&mut self, start: usize, end: usize) -> usize {
        self.replace(start, end, "")
    }

    pub fn replace(&mut self, start: usize, end: usize, inserted: &str) -> usize {
        self.replace_with_kind(start, end, inserted, EditKind::Standalone)
    }

    pub fn insert_typed(&mut self, char_index: usize, text: &str) -> usize {
        self.replace_with_kind(char_index, char_index, text, EditKind::Typing)
    }

    pub fn delete_backward_coalesced(&mut self, start: usize, end: usize) -> usize {
        self.replace_with_kind(start, end, "", EditKind::DeleteBackward)
    }

    pub fn delete_forward_coalesced(&mut self, start: usize, end: usize) -> usize {
        self.replace_with_kind(start, end, "", EditKind::DeleteForward)
    }

    fn replace_with_kind(
        &mut self,
        start: usize,
        end: usize,
        inserted: &str,
        kind: EditKind,
    ) -> usize {
        let start = start.min(self.len_chars());
        let end = end.clamp(start, self.len_chars());
        let removed = self.slice(start, end);
        if removed == inserted {
            self.last_change = None;
            return start + inserted.chars().count();
        }
        self.replace_text(start, end, inserted);
        let after_revision = self.next_revision;
        self.next_revision = self.next_revision.saturating_add(1);
        let removed_chars = removed.chars().count();
        let edit = Edit {
            start,
            removed,
            inserted: inserted.to_owned(),
            before_revision: self.revision,
            after_revision,
            kind,
        };
        self.revision = after_revision;
        self.redo.clear();
        self.last_change = Some(TextChange {
            start,
            removed_chars,
            inserted_chars: inserted.chars().count(),
        });
        if !self.try_merge_undo(&edit) {
            self.push_undo(edit);
        }
        start + inserted.chars().count()
    }

    pub fn undo(&mut self) -> Option<usize> {
        let edit = self.undo.pop_back()?;
        self.history_bytes = self.history_bytes.saturating_sub(edit.bytes());
        let inserted_end = edit.start + edit.inserted.chars().count();
        self.replace_text(edit.start, inserted_end, &edit.removed);
        self.revision = edit.before_revision;
        self.last_change = Some(TextChange {
            start: edit.start,
            removed_chars: edit.inserted.chars().count(),
            inserted_chars: edit.removed.chars().count(),
        });
        let cursor = edit.start + edit.removed.chars().count();
        self.redo.push(edit);
        self.break_coalescing();
        Some(cursor)
    }

    pub fn redo(&mut self) -> Option<usize> {
        let edit = self.redo.pop()?;
        let removed_end = edit.start + edit.removed.chars().count();
        self.replace_text(edit.start, removed_end, &edit.inserted);
        self.revision = edit.after_revision;
        self.last_change = Some(TextChange {
            start: edit.start,
            removed_chars: edit.removed.chars().count(),
            inserted_chars: edit.inserted.chars().count(),
        });
        let cursor = edit.start + edit.inserted.chars().count();
        self.push_undo(edit);
        self.break_coalescing();
        Some(cursor)
    }

    pub fn break_coalescing(&mut self) {
        if let Some(edit) = self.undo.back_mut() {
            edit.kind = EditKind::Standalone;
        }
    }

    fn try_merge_undo(&mut self, edit: &Edit) -> bool {
        let Some(previous) = self.undo.back_mut() else {
            return false;
        };
        let merged = match edit.kind {
            EditKind::Typing
                if previous.kind == EditKind::Typing
                    && previous.removed.is_empty()
                    && edit.removed.is_empty()
                    && previous.start + previous.inserted.chars().count() == edit.start =>
            {
                previous.inserted.push_str(&edit.inserted);
                true
            }
            EditKind::DeleteBackward
                if previous.kind == EditKind::DeleteBackward
                    && previous.inserted.is_empty()
                    && edit.inserted.is_empty()
                    && edit.start + edit.removed.chars().count() == previous.start =>
            {
                previous.start = edit.start;
                previous.removed.insert_str(0, &edit.removed);
                true
            }
            EditKind::DeleteForward
                if previous.kind == EditKind::DeleteForward
                    && previous.inserted.is_empty()
                    && edit.inserted.is_empty()
                    && previous.start == edit.start =>
            {
                previous.removed.push_str(&edit.removed);
                true
            }
            _ => false,
        };
        if merged {
            previous.after_revision = edit.after_revision;
            self.history_bytes = self.history_bytes.saturating_add(edit.bytes());
            self.enforce_history_limit();
        }
        merged
    }

    fn push_undo(&mut self, edit: Edit) {
        let bytes = edit.bytes();
        self.undo.push_back(edit);
        self.history_bytes = self.history_bytes.saturating_add(bytes);
        self.enforce_history_limit();
    }

    fn enforce_history_limit(&mut self) {
        while self.history_bytes > self.history_limit && self.undo.len() > 1 {
            if let Some(oldest) = self.undo.pop_front() {
                self.history_bytes = self.history_bytes.saturating_sub(oldest.bytes());
            }
        }
    }

    fn replace_text(&mut self, start: usize, end: usize, inserted: &str) {
        let first_line = self.char_to_line(start);
        let old_last_line = self.char_to_line(end);
        let old_widths = (first_line..=old_last_line)
            .map(|line| self.line_display_width(line))
            .collect::<Vec<_>>();
        for width in old_widths {
            self.remove_line_width(width);
        }

        self.text.remove(start..end);
        self.text.insert(start, inserted);

        let inserted_end = start.saturating_add(inserted.chars().count());
        let new_last_line = self.char_to_line(inserted_end);
        let new_widths = (first_line..=new_last_line)
            .map(|line| self.line_display_width(line))
            .collect::<Vec<_>>();
        for width in new_widths {
            *self.line_width_frequencies.entry(width).or_default() += 1;
        }
    }

    fn remove_line_width(&mut self, width: usize) {
        let remove = {
            let count = self
                .line_width_frequencies
                .get_mut(&width)
                .expect("line width index must contain every document line");
            *count -= 1;
            *count == 0
        };
        if remove {
            self.line_width_frequencies.remove(&width);
        }
    }
}

fn initial_line_width_frequencies(text: &str) -> BTreeMap<usize, usize> {
    let mut frequencies = BTreeMap::new();
    for line in text.split('\n') {
        let width = display_width(line.strip_suffix('\r').unwrap_or(line));
        *frequencies.entry(width).or_default() += 1;
    }
    frequencies
}

fn display_width(text: &str) -> usize {
    if text.is_ascii() {
        return text.bytes().filter(|byte| !byte.is_ascii_control()).count();
    }
    text.graphemes(true).map(UnicodeWidthStr::width).sum()
}

fn detect_line_ending(text: &str) -> Option<LineEnding> {
    let lf = text.find('\n')?;
    Some(if lf > 0 && text.as_bytes()[lf - 1] == b'\r' {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> Document {
        Document::new(text, None, LineEnding::Lf, 1024)
    }

    #[test]
    fn edits_unicode_at_character_boundaries() {
        let mut document = document("a界🙂z");
        document.replace(1, 3, "e\u{301}");
        assert_eq!(document.text(), "ae\u{301}z");
        assert!(document.is_modified());
    }

    #[test]
    fn maximum_line_width_is_indexed_across_the_complete_document() {
        let document = document("short\n界界界\ne\u{301}\noffscreen-longest");

        assert_eq!(document.line_display_width(1), 6);
        assert_eq!(document.line_display_width(2), 1);
        assert_eq!(document.maximum_line_display_width(), 17);
    }

    #[test]
    fn line_width_index_tracks_multiline_edits_and_maximum_shrinkage() {
        let mut document = document("short\nabcdefghij\nmedium");
        assert_eq!(document.maximum_line_display_width(), 10);

        document.insert(5, "-now-longest");
        assert_eq!(document.maximum_line_display_width(), 17);

        document.replace(0, document.line_to_char(1), "x\nyy\n");
        assert_eq!(document.maximum_line_display_width(), 10);

        let longest_start = document.line_to_char(2);
        document.delete(longest_start, document.line_to_char(3));
        assert_eq!(document.maximum_line_display_width(), 6);
    }

    #[test]
    fn undo_and_redo_update_the_index_after_coalesced_typing() {
        let mut document = document("base\nother");
        for character in "-extended".chars() {
            document.insert_typed(document.line_content_end(0), &character.to_string());
        }
        assert_eq!(document.maximum_line_display_width(), 13);

        document.undo();
        assert_eq!(document.maximum_line_display_width(), 5);
        document.redo();
        assert_eq!(document.maximum_line_display_width(), 13);
    }

    #[test]
    fn undo_and_redo_restore_revision_and_text() {
        let mut document = document("abc");
        document.insert(3, "d");
        assert_eq!(document.undo(), Some(3));
        assert_eq!(document.text(), "abc");
        assert!(!document.is_modified());
        assert_eq!(document.redo(), Some(4));
        assert_eq!(document.text(), "abcd");
    }

    #[test]
    fn oversized_latest_edit_is_retained_alone() {
        let mut document = Document::new("", None, LineEnding::Lf, 2);
        document.insert(0, "small");
        assert_eq!(document.undo(), Some(0));
        assert!(document.is_empty());
    }

    #[test]
    fn detects_crlf_without_normalizing_content() {
        let document = Document::new("one\r\ntwo\r\n", None, LineEnding::Lf, 1024);
        assert_eq!(document.line_ending, LineEnding::Crlf);
        assert_eq!(document.text(), "one\r\ntwo\r\n");
    }

    #[test]
    fn contiguous_typing_is_one_undo_transaction() {
        let mut document = document("");
        document.insert_typed(0, "a");
        document.insert_typed(1, "界");
        document.insert_typed(2, "c");
        assert_eq!(document.undo(), Some(0));
        assert_eq!(document.text(), "");
    }

    #[test]
    fn adjacent_deletions_coalesce_by_direction() {
        let mut backward = document("abc");
        backward.delete_backward_coalesced(2, 3);
        backward.delete_backward_coalesced(1, 2);
        assert_eq!(backward.undo(), Some(3));
        assert_eq!(backward.text(), "abc");

        let mut forward = document("abc");
        forward.delete_forward_coalesced(0, 1);
        forward.delete_forward_coalesced(0, 1);
        assert_eq!(forward.undo(), Some(2));
        assert_eq!(forward.text(), "abc");
    }

    #[test]
    fn save_and_standalone_edits_break_typing_groups() {
        let mut document = document("");
        document.insert_typed(0, "a");
        document.mark_saved();
        document.insert_typed(1, "b");
        document.insert(2, "paste");
        assert_eq!(document.undo(), Some(2));
        assert_eq!(document.text(), "ab");
        assert_eq!(document.undo(), Some(1));
        assert_eq!(document.text(), "a");
        assert!(!document.is_modified());
    }
}
