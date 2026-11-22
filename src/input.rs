use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{
    command::Command,
    layout::{FocusDirection, SplitDirection},
};

const LEADER_TIMEOUT: Duration = Duration::from_millis(1_500);

#[derive(Clone, Debug)]
pub struct InputMapper {
    leader_pending_since: Option<Instant>,
    leader: String,
    bindings: BTreeMap<String, String>,
}

impl InputMapper {
    pub fn new(bindings: BTreeMap<String, String>, leader: &str) -> Self {
        Self {
            leader_pending_since: None,
            leader: normalize_key_sequence(leader).unwrap_or_else(|| "ctrl+b".to_owned()),
            bindings,
        }
    }

    pub fn map(&mut self, event: Event, terminal_focused: bool) -> Command {
        self.map_at(event, terminal_focused, Instant::now())
    }

    pub fn map_with_application_cursor(
        &mut self,
        event: Event,
        terminal_focused: bool,
        application_cursor: bool,
    ) -> Command {
        self.map_at_with_application_cursor(
            event,
            terminal_focused,
            application_cursor,
            Instant::now(),
        )
    }

    fn map_at(&mut self, event: Event, terminal_focused: bool, now: Instant) -> Command {
        self.map_at_with_application_cursor(event, terminal_focused, false, now)
    }

    fn map_at_with_application_cursor(
        &mut self,
        event: Event,
        terminal_focused: bool,
        application_cursor: bool,
        now: Instant,
    ) -> Command {
        if self
            .leader_pending_since
            .is_some_and(|started| now.saturating_duration_since(started) >= LEADER_TIMEOUT)
        {
            self.leader_pending_since = None;
        }
        match event {
            Event::Paste(text) => {
                self.cancel_leader();
                Command::Paste(text)
            }
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.map_key(key, terminal_focused, application_cursor, now)
            }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {
                self.cancel_leader();
                Command::NoOp
            }
            _ => Command::NoOp,
        }
    }

    pub fn cancel_leader(&mut self) {
        self.leader_pending_since = None;
    }

    pub fn leader_pending(&self) -> bool {
        self.leader_pending_since
            .is_some_and(|started| started.elapsed() < LEADER_TIMEOUT)
    }

    pub fn expire_leader(&mut self, now: Instant) -> bool {
        let expired = self
            .leader_pending_since
            .is_some_and(|started| now.saturating_duration_since(started) >= LEADER_TIMEOUT);
        if expired {
            self.cancel_leader();
        }
        expired
    }

    fn map_key(
        &mut self,
        key: KeyEvent,
        terminal_focused: bool,
        application_cursor: bool,
        now: Instant,
    ) -> Command {
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('t') {
            self.cancel_leader();
            return Command::ToggleBottomTerminal;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('g') {
            self.cancel_leader();
            return Command::OpenSettings;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('h') {
            self.cancel_leader();
            return Command::OpenHelp;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('f') {
            self.cancel_leader();
            return Command::OpenSearchReplace;
        }
        if self.leader_pending_since.is_some() {
            self.cancel_leader();
            if key.code == KeyCode::Esc {
                return Command::NoOp;
            }
            return self.map_leader_key(key, terminal_focused, application_cursor);
        }
        if self.matches_leader(key) {
            self.leader_pending_since = Some(now);
            return Command::NoOp;
        }
        if terminal_focused {
            return Command::Insert(key_to_terminal_bytes(key, application_cursor));
        }
        if let Some(sequence) = key_sequence(key)
            && let Some(command) = self.bindings.get(&sequence)
        {
            return named_command(command).unwrap_or(Command::NoOp);
        }
        let selecting = key.modifiers.contains(KeyModifiers::SHIFT);
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('q')) => Command::Quit,
            (KeyModifiers::CONTROL, KeyCode::Char('s')) => Command::Save,
            (KeyModifiers::CONTROL, KeyCode::Char('o')) => Command::OpenDialog,
            (KeyModifiers::CONTROL, KeyCode::Char('w')) => Command::ClosePane,
            (KeyModifiers::CONTROL, KeyCode::Char('z')) => Command::Undo,
            (KeyModifiers::CONTROL, KeyCode::Char('y')) => Command::Redo,
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => Command::Copy,
            (KeyModifiers::CONTROL, KeyCode::Char('x')) => Command::Cut,
            (KeyModifiers::CONTROL, KeyCode::Char('v')) => Command::PasteClipboard,
            (KeyModifiers::CONTROL, KeyCode::Char('e')) => Command::ToggleExplorer,
            (KeyModifiers::CONTROL, KeyCode::Char(',')) => Command::OpenSettings,
            (KeyModifiers::CONTROL, KeyCode::Char('g')) => Command::OpenSettings,
            (KeyModifiers::CONTROL, KeyCode::Char('`')) => Command::ToggleBottomTerminal,
            (KeyModifiers::CONTROL, KeyCode::Char('t')) => Command::ToggleBottomTerminal,
            (_, KeyCode::Left) => Command::MoveLeft { selecting },
            (_, KeyCode::Right) => Command::MoveRight { selecting },
            (_, KeyCode::Up) => Command::MoveUp { selecting },
            (_, KeyCode::Down) => Command::MoveDown { selecting },
            (_, KeyCode::Home) => Command::Home { selecting },
            (_, KeyCode::End) => Command::End { selecting },
            (_, KeyCode::PageUp) => Command::PageUp { selecting },
            (_, KeyCode::PageDown) => Command::PageDown { selecting },
            (_, KeyCode::Backspace) => Command::DeleteBackward,
            (_, KeyCode::Delete) => Command::DeleteForward,
            (_, KeyCode::Enter) => Command::Newline,
            (KeyModifiers::NONE, KeyCode::Tab) => Command::Indent,
            (KeyModifiers::SHIFT, KeyCode::BackTab) => Command::Unindent,
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(character)) => {
                Command::Insert(character.to_string())
            }
            _ => Command::NoOp,
        }
    }

    fn map_leader_key(
        &self,
        key: KeyEvent,
        terminal_focused: bool,
        application_cursor: bool,
    ) -> Command {
        if terminal_focused {
            if self.matches_leader(key) {
                return Command::Insert(key_to_terminal_bytes(key, application_cursor));
            }
            match key.code {
                KeyCode::Char('q') => Command::Quit,
                KeyCode::Char('w') => Command::ClosePane,
                KeyCode::Char('n') => Command::NewBottomTerminal,
                KeyCode::Char('[') => Command::PreviousBottomTerminal,
                KeyCode::Char(']') => Command::NextBottomTerminal,
                KeyCode::Char('`') => Command::ToggleBottomTerminal,
                KeyCode::Char('c') => Command::Copy,
                KeyCode::PageUp => Command::Scroll(-10),
                KeyCode::PageDown => Command::Scroll(10),
                _ => Command::NoOp,
            }
        } else {
            match key.code {
                KeyCode::Char('v') => Command::Split(SplitDirection::Vertical),
                KeyCode::Char('h') => Command::Split(SplitDirection::Horizontal),
                KeyCode::Left => Command::MoveFocus(FocusDirection::Left),
                KeyCode::Right => Command::MoveFocus(FocusDirection::Right),
                KeyCode::Up => Command::MoveFocus(FocusDirection::Up),
                KeyCode::Down => Command::MoveFocus(FocusDirection::Down),
                _ => Command::NoOp,
            }
        }
    }

    fn matches_leader(&self, key: KeyEvent) -> bool {
        key_sequence(key).as_deref() == Some(self.leader.as_str())
    }
}

impl Default for InputMapper {
    fn default() -> Self {
        Self::new(BTreeMap::new(), "ctrl+b")
    }
}

fn named_command(name: &str) -> Option<Command> {
    match name {
        "quit" => Some(Command::Quit),
        "save" => Some(Command::Save),
        "open" => Some(Command::OpenDialog),
        "close-pane" => Some(Command::ClosePane),
        "split-vertical" => Some(Command::Split(SplitDirection::Vertical)),
        "split-horizontal" => Some(Command::Split(SplitDirection::Horizontal)),
        "undo" => Some(Command::Undo),
        "redo" => Some(Command::Redo),
        "settings" => Some(Command::OpenSettings),
        "search" => Some(Command::OpenSearchReplace),
        "toggle-terminal" => Some(Command::ToggleBottomTerminal),
        "toggle-explorer" => Some(Command::ToggleExplorer),
        _ => None,
    }
}

pub(crate) fn key_sequence(key: KeyEvent) -> Option<String> {
    let key_name = match key.code {
        KeyCode::Char(character) => character.to_ascii_lowercase().to_string(),
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::BackTab => "backtab".to_owned(),
        KeyCode::Esc => "escape".to_owned(),
        KeyCode::Left => "left".to_owned(),
        KeyCode::Right => "right".to_owned(),
        KeyCode::Up => "up".to_owned(),
        KeyCode::Down => "down".to_owned(),
        _ => return None,
    };
    let mut parts = Vec::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("shift");
    }
    parts.push(&key_name);
    Some(parts.join("+"))
}

pub(crate) fn normalize_key_sequence(sequence: &str) -> Option<String> {
    let mut control = false;
    let mut alt = false;
    let mut shift = false;
    let mut key = None;
    for part in sequence
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" if !control && key.is_none() => control = true,
            "alt" if !alt && key.is_none() => alt = true,
            "shift" if !shift && key.is_none() => shift = true,
            value if key.is_none() && valid_key_name(value) => key = Some(value.to_owned()),
            _ => return None,
        }
    }
    let key = key?;
    let mut parts = Vec::new();
    if control {
        parts.push("ctrl");
    }
    if alt {
        parts.push("alt");
    }
    if shift {
        parts.push("shift");
    }
    parts.push(&key);
    Some(parts.join("+"))
}

fn valid_key_name(value: &str) -> bool {
    value.chars().count() == 1
        || matches!(
            value,
            "enter" | "tab" | "backtab" | "escape" | "left" | "right" | "up" | "down"
        )
}

fn key_to_terminal_bytes(key: KeyEvent, application_cursor: bool) -> String {
    match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let value =
                u32::from(character.to_ascii_lowercase()).saturating_sub(u32::from('a')) + 1;
            char::from_u32(value).map_or_else(String::new, |value| value.to_string())
        }
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::ALT) => {
            format!("\u{1b}{character}")
        }
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => "\u{1b}\r".to_owned(),
        KeyCode::Enter => "\r".to_owned(),
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::ALT) => "\u{1b}\t".to_owned(),
        KeyCode::Tab => "\t".to_owned(),
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
            "\u{1b}\u{7f}".to_owned()
        }
        KeyCode::Backspace => "\u{7f}".to_owned(),
        KeyCode::Esc => "\u{1b}".to_owned(),
        KeyCode::Up if application_cursor => "\u{1b}OA".to_owned(),
        KeyCode::Down if application_cursor => "\u{1b}OB".to_owned(),
        KeyCode::Right if application_cursor => "\u{1b}OC".to_owned(),
        KeyCode::Left if application_cursor => "\u{1b}OD".to_owned(),
        KeyCode::Up => "\u{1b}[A".to_owned(),
        KeyCode::Down => "\u{1b}[B".to_owned(),
        KeyCode::Right => "\u{1b}[C".to_owned(),
        KeyCode::Left => "\u{1b}[D".to_owned(),
        KeyCode::Home => "\u{1b}[H".to_owned(),
        KeyCode::End => "\u{1b}[F".to_owned(),
        KeyCode::Delete => "\u{1b}[3~".to_owned(),
        KeyCode::PageUp => "\u{1b}[5~".to_owned(),
        KeyCode::PageDown => "\u{1b}[6~".to_owned(),
        KeyCode::F(number) => terminal_function_key(number, key.modifiers),
        _ => String::new(),
    }
}

fn terminal_function_key(number: u8, modifiers: KeyModifiers) -> String {
    let modifier = if modifiers.contains(KeyModifiers::SHIFT)
        && modifiers.contains(KeyModifiers::ALT)
    {
        4
    } else if modifiers.contains(KeyModifiers::CONTROL) && modifiers.contains(KeyModifiers::ALT) {
        7
    } else if modifiers.contains(KeyModifiers::CONTROL) {
        5
    } else if modifiers.contains(KeyModifiers::ALT) {
        3
    } else if modifiers.contains(KeyModifiers::SHIFT) {
        2
    } else {
        1
    };
    match number {
        1..=4 => {
            let final_byte = match number {
                1 => 'P',
                2 => 'Q',
                3 => 'R',
                _ => 'S',
            };
            if modifier == 1 {
                format!("\u{1b}O{final_byte}")
            } else {
                format!("\u{1b}[1;{modifier}{final_byte}")
            }
        }
        5..=12 => {
            let final_number = match number {
                1 => 11,
                2 => 12,
                3 => 13,
                4 => 14,
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                _ => 24,
            };
            if modifier == 1 {
                format!("\u{1b}[{final_number}~")
            } else {
                format!("\u{1b}[1;{modifier}{final_number}")
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leader_intercepts_terminal_command_and_double_leader_forwards_literal() {
        let mut mapper = InputMapper::default();
        let leader = Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(mapper.map(leader.clone(), true), Command::NoOp);
        assert_eq!(
            mapper.map(leader, true),
            Command::Insert("\u{2}".to_owned())
        );
        let leader = Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(mapper.map(leader, true), Command::NoOp);
        assert_eq!(
            mapper.map(
                Event::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
                true
            ),
            Command::Scroll(-10)
        );
    }

    #[test]
    fn terminal_leader_split_keys_are_disabled() {
        let mut mapper = InputMapper::default();
        for key in ['v', 'h'] {
            let leader = Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
            assert_eq!(mapper.map(leader, true), Command::NoOp);
            assert_eq!(
                mapper.map(
                    Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                    true
                ),
                Command::NoOp
            );
        }
    }

    #[test]
    fn terminal_leader_maps_session_commands() {
        let mut mapper = InputMapper::default();
        let leader = Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        let cases = [
            (KeyCode::Char('n'), Command::NewBottomTerminal),
            (KeyCode::Char('['), Command::PreviousBottomTerminal),
            (KeyCode::Char(']'), Command::NextBottomTerminal),
        ];
        for (key, expected) in cases {
            assert_eq!(mapper.map(leader.clone(), true), Command::NoOp);
            assert_eq!(
                mapper.map(Event::Key(KeyEvent::new(key, KeyModifiers::NONE)), true),
                expected
            );
        }
    }

    #[test]
    fn terminal_arrows_follow_application_cursor_mode() {
        let event = Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        let mut mapper = InputMapper::default();
        assert_eq!(
            mapper.map_with_application_cursor(event.clone(), true, false),
            Command::Insert("\x1b[A".to_owned())
        );
        assert_eq!(
            mapper.map_with_application_cursor(event, true, true),
            Command::Insert("\x1bOA".to_owned())
        );
    }

    #[test]
    fn editor_leader_maps_split_and_focus_commands() {
        let mut mapper = InputMapper::default();
        let cases = [
            (KeyCode::Char('v'), Command::Split(SplitDirection::Vertical)),
            (
                KeyCode::Char('h'),
                Command::Split(SplitDirection::Horizontal),
            ),
            (KeyCode::Left, Command::MoveFocus(FocusDirection::Left)),
            (KeyCode::Right, Command::MoveFocus(FocusDirection::Right)),
            (KeyCode::Up, Command::MoveFocus(FocusDirection::Up)),
            (KeyCode::Down, Command::MoveFocus(FocusDirection::Down)),
        ];
        for (key, expected) in cases {
            let leader = Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
            assert_eq!(mapper.map(leader, false), Command::NoOp);
            assert_eq!(
                mapper.map(Event::Key(KeyEvent::new(key, KeyModifiers::NONE)), false),
                expected
            );
        }
    }

    #[test]
    fn configured_binding_maps_through_semantic_command_layer() {
        let mut mapper = InputMapper::new(
            BTreeMap::from([("ctrl+k".to_owned(), "save".to_owned())]),
            "ctrl+b",
        );
        let event = Event::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(mapper.map(event, false), Command::Save);
    }

    #[test]
    fn portable_workbench_shortcuts_are_mapped() {
        let mut mapper = InputMapper::default();
        let terminal = Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        let settings = Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(
            mapper.map(terminal.clone(), false),
            Command::ToggleBottomTerminal
        );
        assert_eq!(mapper.map(settings.clone(), false), Command::OpenSettings);
        assert_eq!(mapper.map(terminal, true), Command::ToggleBottomTerminal);
        assert_eq!(mapper.map(settings, true), Command::OpenSettings);
    }

    #[test]
    fn configured_leader_replaces_the_default_and_doubles_to_terminal_control() {
        let mut mapper = InputMapper::new(BTreeMap::new(), "ctrl+a");
        let configured = Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        let default = Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(mapper.map(default, false), Command::NoOp);
        assert!(!mapper.leader_pending());
        assert_eq!(mapper.map(configured.clone(), true), Command::NoOp);
        assert_eq!(
            mapper.map(configured, true),
            Command::Insert("\u{1}".to_owned())
        );
    }

    #[test]
    fn pending_leader_cancels_on_timeout_paste_escape_and_global_shortcut() {
        let started = Instant::now();
        let leader = Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        let mut mapper = InputMapper::default();
        assert_eq!(mapper.map_at(leader.clone(), false, started), Command::NoOp);
        assert_eq!(
            mapper.map_at(
                Event::Key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)),
                false,
                started + LEADER_TIMEOUT
            ),
            Command::Insert("v".to_owned())
        );
        assert_eq!(mapper.map_at(leader.clone(), false, started), Command::NoOp);
        assert_eq!(
            mapper.map_at(Event::Paste("text".to_owned()), false, started),
            Command::Paste("text".to_owned())
        );
        assert_eq!(mapper.map_at(leader.clone(), false, started), Command::NoOp);
        assert_eq!(
            mapper.map_at(
                Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
                false,
                started
            ),
            Command::NoOp
        );
        assert_eq!(mapper.map_at(leader, false, started), Command::NoOp);
        assert_eq!(
            mapper.map_at(
                Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
                false,
                started
            ),
            Command::ToggleBottomTerminal
        );
        assert!(!mapper.leader_pending());
    }
}
