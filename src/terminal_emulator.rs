use std::{collections::VecDeque, path::PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use unicode_width::UnicodeWidthChar;
use vte::{Params, Parser, Perform};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TerminalColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalCell {
    pub character: char,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub inverse: bool,
    pub continuation: bool,
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self {
            character: ' ',
            foreground: TerminalColor::Default,
            background: TerminalColor::Default,
            bold: false,
            inverse: false,
            continuation: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MouseTrackingMode {
    #[default]
    Disabled,
    Normal,
    ButtonEvent,
    AnyEvent,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalModes {
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub mouse_normal: bool,
    pub mouse_button_event: bool,
    pub mouse_any_event: bool,
    pub mouse_sgr: bool,
}

impl TerminalModes {
    pub fn mouse_tracking(self) -> MouseTrackingMode {
        if self.mouse_any_event {
            MouseTrackingMode::AnyEvent
        } else if self.mouse_button_event {
            MouseTrackingMode::ButtonEvent
        } else if self.mouse_normal {
            MouseTrackingMode::Normal
        } else {
            MouseTrackingMode::Disabled
        }
    }
}

pub struct TerminalEmulator {
    parser: Parser,
    screen: Screen,
}

impl std::fmt::Debug for TerminalEmulator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalEmulator")
            .field("screen", &self.screen)
            .finish_non_exhaustive()
    }
}

impl TerminalEmulator {
    pub fn new(rows: u16, columns: u16, scrollback_limit: usize, allow_child_osc52: bool) -> Self {
        Self {
            parser: Parser::new(),
            screen: Screen::new(rows, columns, scrollback_limit, allow_child_osc52),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.screen, bytes);
    }

    pub fn resize(&mut self, rows: u16, columns: u16) {
        self.screen.resize(rows, columns);
    }

    pub fn visible_lines(&self) -> Vec<String> {
        self.screen
            .grid
            .iter()
            .map(|row| {
                row.iter()
                    .filter(|cell| !cell.continuation)
                    .map(|cell| cell.character)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    pub fn visible_cells(&self) -> &[Vec<TerminalCell>] {
        &self.screen.grid
    }

    pub fn viewport_cells(&self, rows: usize, scroll_offset: usize) -> Vec<Vec<TerminalCell>> {
        let total = self
            .screen
            .scrollback
            .len()
            .saturating_add(self.screen.grid.len());
        let end = total.saturating_sub(scroll_offset.min(self.screen.scrollback.len()));
        let start = end.saturating_sub(rows);
        (start..end)
            .filter_map(|index| {
                if index < self.screen.scrollback.len() {
                    self.screen.scrollback.get(index).cloned()
                } else {
                    self.screen
                        .grid
                        .get(index.saturating_sub(self.screen.scrollback.len()))
                        .cloned()
                }
            })
            .collect()
    }

    pub fn cursor(&self) -> (u16, u16) {
        (
            self.screen.cursor_row as u16,
            self.screen.cursor_column as u16,
        )
    }

    pub fn modes(&self) -> TerminalModes {
        self.screen.modes
    }

    pub fn title(&self) -> &str {
        &self.screen.title
    }

    pub fn working_directory(&self) -> Option<&PathBuf> {
        self.screen.working_directory.as_ref()
    }

    pub fn take_child_clipboard_write(&mut self) -> Option<String> {
        self.screen.child_clipboard_write.take()
    }

    pub fn take_response(&mut self) -> Option<Vec<u8>> {
        self.screen.responses.pop_front()
    }

    pub fn scrollback_len(&self) -> usize {
        self.screen.scrollback.len()
    }

    pub fn apply_runtime_config(&mut self, scrollback_limit: usize, allow_child_osc52: bool) {
        self.screen.scrollback_limit = scrollback_limit;
        while self.screen.scrollback.len() > scrollback_limit {
            self.screen.scrollback.pop_front();
        }
        self.screen.allow_child_osc52 = allow_child_osc52;
    }
}

#[derive(Debug)]
struct Screen {
    rows: usize,
    columns: usize,
    grid: Vec<Vec<TerminalCell>>,
    primary_grid: Option<Vec<Vec<TerminalCell>>>,
    cursor_row: usize,
    cursor_column: usize,
    saved_cursor: (usize, usize),
    current: TerminalCell,
    modes: TerminalModes,
    scrollback: VecDeque<Vec<TerminalCell>>,
    scrollback_limit: usize,
    title: String,
    working_directory: Option<PathBuf>,
    allow_child_osc52: bool,
    child_clipboard_write: Option<String>,
    responses: VecDeque<Vec<u8>>,
}

impl Screen {
    fn new(rows: u16, columns: u16, scrollback_limit: usize, allow_child_osc52: bool) -> Self {
        let rows = usize::from(rows.max(1));
        let columns = usize::from(columns.max(1));
        Self {
            rows,
            columns,
            grid: blank_grid(rows, columns),
            primary_grid: None,
            cursor_row: 0,
            cursor_column: 0,
            saved_cursor: (0, 0),
            current: TerminalCell::default(),
            modes: TerminalModes::default(),
            scrollback: VecDeque::new(),
            scrollback_limit,
            title: String::new(),
            working_directory: None,
            allow_child_osc52,
            child_clipboard_write: None,
            responses: VecDeque::new(),
        }
    }

    fn resize(&mut self, rows: u16, columns: u16) {
        self.rows = usize::from(rows.max(1));
        self.columns = usize::from(columns.max(1));
        resize_grid(&mut self.grid, self.rows, self.columns);
        if let Some(primary_grid) = &mut self.primary_grid {
            resize_grid(primary_grid, self.rows, self.columns);
        }
        self.cursor_row = self.cursor_row.min(self.rows - 1);
        self.cursor_column = self.cursor_column.min(self.columns - 1);
        self.saved_cursor.0 = self.saved_cursor.0.min(self.rows - 1);
        self.saved_cursor.1 = self.saved_cursor.1.min(self.columns - 1);
    }

    fn line_feed(&mut self) {
        if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
            return;
        }
        let removed = self.grid.remove(0);
        if !self.modes.alternate_screen {
            self.scrollback.push_back(removed);
            while self.scrollback.len() > self.scrollback_limit {
                self.scrollback.pop_front();
            }
        }
        self.grid.push(blank_row(self.columns));
    }

    fn write_character(&mut self, character: char) {
        let width = character.width().unwrap_or(0);
        if width == 0 {
            return;
        }
        if self.cursor_column >= self.columns || self.cursor_column + width > self.columns {
            self.cursor_column = 0;
            self.line_feed();
        }
        let mut cell = self.current.clone();
        cell.character = character;
        cell.continuation = false;
        self.grid[self.cursor_row][self.cursor_column] = cell;
        for offset in 1..width {
            self.grid[self.cursor_row][self.cursor_column + offset] = TerminalCell {
                continuation: true,
                ..TerminalCell::default()
            };
        }
        self.cursor_column += width;
    }

    fn clear_line(&mut self, mode: u16) {
        let (start, end) = match mode {
            1 => (0, self.cursor_column.saturating_add(1)),
            2 => (0, self.columns),
            _ => (self.cursor_column, self.columns),
        };
        self.grid[self.cursor_row][start.min(self.columns)..end.min(self.columns)]
            .fill(TerminalCell::default());
    }

    fn clear_display(&mut self, mode: u16) {
        match mode {
            2 | 3 => self.grid = blank_grid(self.rows, self.columns),
            1 => {
                for row in &mut self.grid[..self.cursor_row] {
                    row.fill(TerminalCell::default());
                }
                self.grid[self.cursor_row][..self.cursor_column.min(self.columns)]
                    .fill(TerminalCell::default());
            }
            _ => {
                self.clear_line(0);
                for row in self.grid.iter_mut().skip(self.cursor_row + 1) {
                    row.fill(TerminalCell::default());
                }
            }
        }
    }

    fn set_alternate_screen(&mut self, enabled: bool) {
        if enabled && !self.modes.alternate_screen {
            self.primary_grid = Some(std::mem::replace(
                &mut self.grid,
                blank_grid(self.rows, self.columns),
            ));
            self.saved_cursor = (self.cursor_row, self.cursor_column);
            self.cursor_row = 0;
            self.cursor_column = 0;
        } else if !enabled
            && self.modes.alternate_screen
            && let Some(primary) = self.primary_grid.take()
        {
            self.grid = primary;
            (self.cursor_row, self.cursor_column) = self.saved_cursor;
        }
        self.modes.alternate_screen = enabled;
    }

    fn set_sgr(&mut self, params: &Params) {
        let values = params
            .iter()
            .map(|parameter| parameter.first().copied().unwrap_or(0))
            .collect::<Vec<_>>();
        let values = if values.is_empty() { vec![0] } else { values };
        let mut index = 0;
        while index < values.len() {
            match values[index] {
                0 => self.current = TerminalCell::default(),
                1 => self.current.bold = true,
                22 => self.current.bold = false,
                7 => self.current.inverse = true,
                27 => self.current.inverse = false,
                30..=37 => {
                    self.current.foreground = TerminalColor::Indexed((values[index] - 30) as u8)
                }
                39 => self.current.foreground = TerminalColor::Default,
                40..=47 => {
                    self.current.background = TerminalColor::Indexed((values[index] - 40) as u8)
                }
                49 => self.current.background = TerminalColor::Default,
                90..=97 => {
                    self.current.foreground = TerminalColor::Indexed((values[index] - 82) as u8)
                }
                100..=107 => {
                    self.current.background = TerminalColor::Indexed((values[index] - 92) as u8)
                }
                38 | 48 if values.get(index + 1) == Some(&5) => {
                    if let Some(color) = values
                        .get(index + 2)
                        .and_then(|value| u8::try_from(*value).ok())
                    {
                        if values[index] == 38 {
                            self.current.foreground = TerminalColor::Indexed(color);
                        } else {
                            self.current.background = TerminalColor::Indexed(color);
                        }
                    }
                    index += 2;
                }
                38 | 48 if values.get(index + 1) == Some(&2) => {
                    let color = values.get(index + 2..index + 5).and_then(|values| {
                        Some(TerminalColor::Rgb(
                            u8::try_from(values[0]).ok()?,
                            u8::try_from(values[1]).ok()?,
                            u8::try_from(values[2]).ok()?,
                        ))
                    });
                    if let Some(color) = color {
                        if values[index] == 38 {
                            self.current.foreground = color;
                        } else {
                            self.current.background = color;
                        }
                    }
                    index += 4;
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn param(params: &Params, index: usize, default: u16) -> usize {
        usize::from(
            params
                .iter()
                .nth(index)
                .and_then(|value| value.first())
                .copied()
                .filter(|value| *value > 0)
                .unwrap_or(default),
        )
    }
}

impl Perform for Screen {
    fn print(&mut self, character: char) {
        self.write_character(character);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            b'\r' => self.cursor_column = 0,
            b'\x08' => self.cursor_column = self.cursor_column.saturating_sub(1),
            b'\t' => self.cursor_column = ((self.cursor_column / 8) + 1) * 8,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        let private = intermediates == b"?";
        match action {
            'A' => self.cursor_row = self.cursor_row.saturating_sub(Self::param(params, 0, 1)),
            'B' => {
                self.cursor_row = (self.cursor_row + Self::param(params, 0, 1)).min(self.rows - 1)
            }
            'C' => {
                self.cursor_column =
                    (self.cursor_column + Self::param(params, 0, 1)).min(self.columns - 1)
            }
            'D' => {
                self.cursor_column = self.cursor_column.saturating_sub(Self::param(params, 0, 1))
            }
            'G' => {
                self.cursor_column = Self::param(params, 0, 1)
                    .saturating_sub(1)
                    .min(self.columns - 1)
            }
            'H' | 'f' => {
                self.cursor_row = Self::param(params, 0, 1)
                    .saturating_sub(1)
                    .min(self.rows - 1);
                self.cursor_column = Self::param(params, 1, 1)
                    .saturating_sub(1)
                    .min(self.columns - 1);
            }
            'J' => self.clear_display(Self::param(params, 0, 0) as u16),
            'K' => self.clear_line(Self::param(params, 0, 0) as u16),
            'm' => self.set_sgr(params),
            'n' => match Self::param(params, 0, 0) {
                5 => self.responses.push_back(b"\x1b[0n".to_vec()),
                6 => self.responses.push_back(
                    format!(
                        "\x1b[{};{}R",
                        self.cursor_row.saturating_add(1),
                        self.cursor_column.saturating_add(1)
                    )
                    .into_bytes(),
                ),
                _ => {}
            },
            'c' => self.responses.push_back(b"\x1b[?1;2c".to_vec()),
            's' => self.saved_cursor = (self.cursor_row, self.cursor_column),
            'u' => (self.cursor_row, self.cursor_column) = self.saved_cursor,
            'h' | 'l' if private => {
                let enabled = action == 'h';
                for value in params.iter().filter_map(|value| value.first()) {
                    match value {
                        1 => self.modes.application_cursor = enabled,
                        1000 => self.modes.mouse_normal = enabled,
                        1002 => self.modes.mouse_button_event = enabled,
                        1003 => self.modes.mouse_any_event = enabled,
                        1006 => self.modes.mouse_sgr = enabled,
                        1047 | 1049 => self.set_alternate_screen(enabled),
                        2004 => self.modes.bracketed_paste = enabled,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        match byte {
            b'7' => self.saved_cursor = (self.cursor_row, self.cursor_column),
            b'8' => (self.cursor_row, self.cursor_column) = self.saved_cursor,
            b'D' => self.line_feed(),
            b'E' => {
                self.cursor_column = 0;
                self.line_feed();
            }
            b'c' => {
                self.grid = blank_grid(self.rows, self.columns);
                self.cursor_row = 0;
                self.cursor_column = 0;
                self.current = TerminalCell::default();
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let Some(code) = params
            .first()
            .and_then(|value| std::str::from_utf8(value).ok())
        else {
            return;
        };
        match code {
            "0" | "2" => {
                if let Some(value) = params
                    .get(1)
                    .and_then(|value| std::str::from_utf8(value).ok())
                {
                    self.title = value
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(256)
                        .collect();
                }
            }
            "7" => {
                if let Some(value) = params
                    .get(1)
                    .and_then(|value| std::str::from_utf8(value).ok())
                    && let Some(path) = value
                        .strip_prefix("file://")
                        .and_then(|value| value.find('/').map(|slash| &value[slash..]))
                {
                    self.working_directory = Some(PathBuf::from(path));
                }
            }
            "52" if self.allow_child_osc52 => {
                if let Some(payload) = params.get(2)
                    && *payload != b"?"
                    && let Ok(decoded) = STANDARD.decode(payload)
                    && let Ok(text) = String::from_utf8(decoded)
                {
                    self.child_clipboard_write = Some(text);
                }
            }
            _ => {}
        }
    }
}

fn resize_grid(grid: &mut Vec<Vec<TerminalCell>>, rows: usize, columns: usize) {
    grid.resize_with(rows, || blank_row(columns));
    for row in grid {
        row.resize(columns, TerminalCell::default());
    }
}

fn blank_grid(rows: usize, columns: usize) -> Vec<Vec<TerminalCell>> {
    (0..rows).map(|_| blank_row(columns)).collect()
}

fn blank_row(columns: usize) -> Vec<TerminalCell> {
    vec![TerminalCell::default(); columns]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_sequences_move_cursor_and_apply_colors() {
        let mut terminal = TerminalEmulator::new(3, 8, 10, false);
        terminal.feed(b"ab\x1b[");
        terminal.feed(b"2;3H\x1b[31mX");
        assert_eq!(terminal.cursor(), (1, 3));
        assert_eq!(terminal.screen.grid[1][2].character, 'X');
        assert_eq!(
            terminal.screen.grid[1][2].foreground,
            TerminalColor::Indexed(1)
        );
    }

    #[test]
    fn device_and_cursor_queries_produce_bounded_responses() {
        let mut terminal = TerminalEmulator::new(3, 8, 10, false);
        terminal.feed(b"\x1b[2;3H\x1b[6n\x1b[c");
        assert_eq!(
            terminal.take_response().as_deref(),
            Some(b"\x1b[2;3R".as_slice())
        );
        assert_eq!(
            terminal.take_response().as_deref(),
            Some(b"\x1b[?1;2c".as_slice())
        );
        assert_eq!(terminal.take_response(), None);
    }

    #[test]
    fn alternate_screen_restores_primary_content() {
        let mut terminal = TerminalEmulator::new(2, 8, 10, false);
        terminal.feed(b"primary\x1b[?1049hother\x1b[?1049l");
        assert_eq!(terminal.visible_lines()[0], "primary");
        assert!(!terminal.modes().alternate_screen);
    }

    #[test]
    fn mouse_tracking_and_encoding_modes_are_independent() {
        let mut terminal = TerminalEmulator::new(2, 8, 10, false);
        terminal.feed(b"\x1b[?1000h");
        assert_eq!(terminal.modes().mouse_tracking(), MouseTrackingMode::Normal);
        assert!(!terminal.modes().mouse_sgr);
        terminal.feed(b"\x1b[?1006h");
        assert_eq!(terminal.modes().mouse_tracking(), MouseTrackingMode::Normal);
        assert!(terminal.modes().mouse_sgr);
        terminal.feed(b"\x1b[?1002h\x1b[?1002l");
        assert_eq!(terminal.modes().mouse_tracking(), MouseTrackingMode::Normal);
        terminal.feed(b"\x1b[?1000l");
        assert_eq!(
            terminal.modes().mouse_tracking(),
            MouseTrackingMode::Disabled
        );
        assert!(terminal.modes().mouse_sgr);
    }

    #[test]
    fn scrollback_and_metadata_are_bounded_and_sanitized() {
        let mut terminal = TerminalEmulator::new(2, 8, 1, false);
        terminal.feed(b"one\ntwo\nthree\x1b]2;safe\x01title\x07");
        assert_eq!(terminal.scrollback_len(), 1);
        assert_eq!(terminal.title(), "safetitle");
    }

    #[test]
    fn child_osc52_is_denied_by_default_and_reads_are_never_serviced() {
        let mut denied = TerminalEmulator::new(2, 8, 10, false);
        denied.feed(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(denied.take_child_clipboard_write(), None);
        let mut allowed = TerminalEmulator::new(2, 8, 10, true);
        allowed.feed(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(
            allowed.take_child_clipboard_write(),
            Some("hello".to_owned())
        );
        allowed.feed(b"\x1b]52;c;?\x07");
        assert_eq!(allowed.take_child_clipboard_write(), None);
    }

    #[test]
    fn resize_preserves_visible_content_and_bounds_cursor() {
        let mut terminal = TerminalEmulator::new(2, 4, 10, false);
        terminal.feed("界x".as_bytes());
        terminal.resize(1, 2);
        assert_eq!(terminal.visible_lines(), vec!["界"]);
        assert_eq!(terminal.cursor(), (0, 1));
    }

    #[test]
    fn resize_updates_primary_grid_during_alternate_screen() {
        let mut terminal = TerminalEmulator::new(1, 3, 10, false);
        terminal.feed(b"pri\x1b[?1049halt");

        terminal.resize(3, 8);
        terminal.feed(b"\x1b[?1049l");

        assert_eq!(terminal.visible_lines().len(), 3);
        terminal.feed(b"\x1b[3;8Hx");
        assert_eq!(terminal.visible_lines()[2], "       x");
    }

    #[test]
    fn viewport_can_move_from_live_grid_into_scrollback() {
        let mut terminal = TerminalEmulator::new(2, 8, 10, false);
        terminal.feed(b"one\ntwo\nthree");
        let live = terminal.viewport_cells(2, 0);
        let history = terminal.viewport_cells(2, 1);
        assert_ne!(live, history);
        assert!(history.iter().flatten().any(|cell| cell.character == 'o'));
    }
}
