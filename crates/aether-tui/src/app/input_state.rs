//! Application input state.
use super::Effort;
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// A large paste and its placeholder's tracked range in the owning draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastedText {
    pub placeholder: String,
    pub text: String,
    pub range: std::ops::Range<usize>,
}

/// Multi-line editor for the input pane.
#[derive(Debug, Default)]
pub struct InputState {
    pub text: String,
    pub(crate) revision: u64,
    /// Byte offset of the cursor.
    pub cursor: usize,
    pub(crate) pastes: Vec<PastedText>,
    /// Images that leave with the next message; the chips above the draft.
    pub(crate) images: Vec<aether_core::attachment::ImageAttachment>,
    /// Images asked for and not yet read; a message waits for them.
    pub(crate) image_reads: usize,
    /// Reads asked for by an agent no longer on screen; their results are dropped.
    pub(crate) stale_reads: usize,
    pub(super) history: Vec<String>,
    pub(super) history_pos: Option<usize>,
}

/// What one agent's composer holds while another agent is on screen.
#[derive(Debug, Default)]
pub(crate) struct Draft {
    text: String,
    cursor: usize,
    pastes: Vec<PastedText>,
    images: Vec<aether_core::attachment::ImageAttachment>,
}

impl Draft {
    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty() && self.images.is_empty()
    }
}

/// Effort levels in picker order, with a one-line description each.
pub const EFFORTS: &[(Effort, &str)] = &[
    (Effort::Low, "fastest, quick answers"),
    (Effort::Medium, "balanced"),
    (Effort::High, "thorough, default"),
    (Effort::XHigh, "extra depth"),
    (Effort::Max, "maximum reasoning"),
];

impl InputState {
    /// Fold a parked draft into the current one: its text (pastes included)
    /// follows on a new line and its images join the chips.
    pub(crate) fn absorb_draft(&mut self, parked: Draft) {
        self.revision += 1;
        self.history_pos = None;
        self.images.extend(parked.images);
        if parked.text.is_empty() {
            return;
        }
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let offset = self.text.len();
        self.text.push_str(&parked.text);
        for mut paste in parked.pastes {
            paste.range = paste.range.start + offset..paste.range.end + offset;
            self.pastes.push(paste);
        }
        assert!(self.pastes.iter().all(|p| p.range.end <= self.text.len()));
        assert!(self.cursor <= self.text.len());
    }

    /// Swap the composer's draft for `other`, returning the draft that was in it.
    pub(crate) fn swap_draft(&mut self, other: Draft) -> Draft {
        assert!(
            other.cursor <= other.text.len(),
            "draft cursor past its text"
        );
        self.revision += 1;
        self.history_pos = None;
        Draft {
            text: std::mem::replace(&mut self.text, other.text),
            cursor: std::mem::replace(&mut self.cursor, other.cursor),
            pastes: std::mem::replace(&mut self.pastes, other.pastes),
            images: std::mem::replace(&mut self.images, other.images),
        }
    }

    /// Replace the whole text; cursor goes to the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.revision += 1;
        self.text = text.into();
        self.pastes.clear();
        self.cursor = self.text.len();
    }

    pub fn insert_str(&mut self, s: &str) {
        self.revision += 1;
        self.replace_range(self.cursor..self.cursor, s);
        self.cursor += s.len();
        self.snap_cursor();
        self.history_pos = None;
    }

    pub fn insert_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.insert_str(c.encode_utf8(&mut buf));
    }

    pub fn backspace(&mut self) {
        if let Some((idx, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() {
            self.revision += 1;
            self.replace_range(idx..self.cursor, "");
            self.cursor = idx;
            self.snap_cursor();
        }
    }

    pub fn delete(&mut self) {
        if let Some(g) = self.text[self.cursor..].graphemes(true).next() {
            let end = self.cursor + g.len();
            self.revision += 1;
            self.replace_range(self.cursor..end, "");
            self.snap_cursor();
        }
    }

    pub fn left(&mut self) {
        if let Some((idx, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() {
            self.cursor = idx;
        }
    }

    pub fn right(&mut self) {
        if let Some(g) = self.text[self.cursor..].graphemes(true).next() {
            self.cursor += g.len();
        }
    }

    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
    }

    pub fn end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len());
    }

    /// Byte offset where the word before the cursor starts (spaces skipped first).
    pub(super) fn word_start_before(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(char::is_whitespace);
        trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0)
    }

    /// Byte offset just past the word after the cursor (spaces skipped first).
    pub(super) fn word_end_after(&self) -> usize {
        let after = &self.text[self.cursor..];
        let rest = after.trim_start_matches(char::is_whitespace);
        let word = rest.find(char::is_whitespace).unwrap_or(rest.len());
        self.cursor + (after.len() - rest.len()) + word
    }

    pub fn word_left(&mut self) {
        self.cursor = self.word_start_before();
    }

    pub fn word_right(&mut self) {
        self.cursor = self.word_end_after();
    }

    pub fn delete_word_back(&mut self) {
        self.revision += 1;
        let start = self.word_start_before();
        self.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.snap_cursor();
        self.history_pos = None;
    }

    pub fn kill_to_line_start(&mut self) {
        self.revision += 1;
        let end = self.cursor;
        self.home();
        self.replace_range(self.cursor..end, "");
        self.snap_cursor();
        self.history_pos = None;
    }

    pub fn kill_to_line_end(&mut self) {
        self.revision += 1;
        let start = self.cursor;
        self.end();
        self.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.snap_cursor();
        self.history_pos = None;
    }

    /// (line index, column in graphemes) of the cursor.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let line = before.matches('\n').count();
        let col = before
            .rsplit('\n')
            .next()
            .map(|s| s.graphemes(true).count())
            .unwrap_or(0);
        (line, col)
    }

    pub fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    /// Move to the same column on the line above. False when already on the first line.
    pub fn up(&mut self) -> bool {
        let (line, col) = self.cursor_line_col();
        if line == 0 {
            return false;
        }
        self.cursor = self.offset_of(line - 1, col);
        true
    }

    pub fn down(&mut self) -> bool {
        let (line, col) = self.cursor_line_col();
        if line + 1 >= self.line_count() {
            return false;
        }
        self.cursor = self.offset_of(line + 1, col);
        true
    }

    pub(super) fn offset_of(&self, line: usize, col: usize) -> usize {
        let mut offset = 0;
        for (i, l) in self.text.split('\n').enumerate() {
            if i == line {
                let within: usize = l.graphemes(true).take(col).map(str::len).sum();
                return offset + within;
            }
            offset += l.len() + 1;
        }
        self.text.len()
    }

    /// Remove `range` and return it; the cursor moves to its start.
    pub(crate) fn cut(&mut self, range: std::ops::Range<usize>) -> String {
        self.revision += 1;
        self.history_pos = None;
        self.cursor = range.start;
        let text = self.text[range.clone()].to_owned();
        self.replace_range(range, "");
        self.snap_cursor();
        text
    }

    fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        let removed = range.len();
        self.pastes.retain_mut(|paste| {
            if removed > 0 && range.start <= paste.range.start && paste.range.end <= range.end {
                return false;
            }
            let shifted = |at: usize| at - removed + text.len();
            if range.end <= paste.range.start {
                paste.range = shifted(paste.range.start)..shifted(paste.range.end);
            } else if range.start < paste.range.end {
                // Keep a partially edited paste visible to the damaged-paste guard.
                paste.range.start = paste.range.start.min(range.start);
                paste.range.end = shifted(paste.range.end.max(range.end));
            }
            true
        });
        self.text.replace_range(range, text);
    }

    fn snap_cursor(&mut self) {
        let mut cursor = GraphemeCursor::new(self.cursor, self.text.len(), true);
        let boundary = match cursor.is_boundary(&self.text, 0) {
            Ok(boundary) => boundary,
            Err(error) => unreachable!("whole draft supplies grapheme context: {error:?}"),
        };
        if !boundary {
            self.cursor = match cursor.next_boundary(&self.text, 0) {
                Ok(Some(at)) => at,
                Ok(None) => self.text.len(),
                Err(error) => unreachable!("whole draft supplies grapheme context: {error:?}"),
            };
        }
    }

    /// Byte offset of the start of the cursor's logical line.
    pub(crate) fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    /// Byte offset of the end of the cursor's logical line (before its `\n`).
    pub(crate) fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }

    /// Sent messages, oldest first.
    pub(crate) fn history(&self) -> &[String] {
        &self.history
    }

    /// Take the text for submission and record it in history.
    pub fn take(&mut self) -> String {
        self.revision += 1;
        let text = std::mem::take(&mut self.text);
        self.pastes.clear();
        self.cursor = 0;
        self.history_pos = None;
        if !text.trim().is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        text
    }

    pub(super) fn history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let pos = match self.history_pos {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.history_pos = Some(pos);
        self.revision += 1;
        self.text = self.history[pos].clone();
        self.pastes.clear();
        self.cursor = self.text.len();
        true
    }

    pub(super) fn history_next(&mut self) -> bool {
        let Some(pos) = self.history_pos else {
            return false;
        };
        self.revision += 1;
        self.pastes.clear();
        if pos + 1 < self.history.len() {
            self.history_pos = Some(pos + 1);
            self.text = self.history[pos + 1].clone();
        } else {
            self.history_pos = None;
            self.text.clear();
        }
        self.cursor = self.text.len();
        true
    }
}
