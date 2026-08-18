use std::collections::BTreeMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{
    command::Command,
    layout::{FocusDirection, SplitDirection},
};

#[derive(Clone, Debug, Default)]
pub struct InputMapper {
    leader_pending: bool,
    bindings: BTreeMap<String, String>,
}

impl InputMapper {
    pub fn new(bindings: BTreeMap<String, String>) -> Self {
        Self {
            leader_pending: false,
            bindings,
        }
    }

    pub fn map(&mut self, event: Event, terminal_focused: bool) -> Command {
        match event {
            Event::Paste(text) => Command::Paste(text),
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.map_key(key, terminal_focused)
            }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {
                Command::NoOp
            }
            _ => Command::NoOp,
        }
    }

    fn map_key(&mut self, key: KeyEvent, terminal_focused: bool) -> Command {
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('t') {
            return Command::ToggleBottomTerminal;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('g') {
            return Command::OpenSettings;
        }
        let leader = key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('b');
        if self.leader_pending {
            self.leader_pending = false;
            return self.map_leader_key(key, terminal_focused);
        }
        if leader {
            self.leader_pending = true;
            return Command::NoOp;
        }
        if terminal_focused {
            return Command::Insert(key_to_terminal_bytes(key));
        }
        if let Some(command) = key_sequence(key)
            .and_then(|sequence| self.bindings.get(&sequence))
            .and_then(|command| named_command(command))
        {
            return command;
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

    fn map_leader_key(&self, key: KeyEvent, terminal_focused: bool) -> Command {
        if terminal_focused {
            match key.code {
                KeyCode::Char('b') if key.modifiers == KeyModifiers::CONTROL => {
                    Command::Insert("\u{2}".to_owned())
                }
                KeyCode::Char('q') => Command::Quit,
                KeyCode::Char('w') => Command::ClosePane,
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
        "toggle-terminal" => Some(Command::ToggleBottomTerminal),
        "toggle-explorer" => Some(Command::ToggleExplorer),
        _ => None,
    }
}

fn key_sequence(key: KeyEvent) -> Option<String> {
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

fn key_to_terminal_bytes(key: KeyEvent) -> String {
    match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let value =
                u32::from(character.to_ascii_lowercase()).saturating_sub(u32::from('a')) + 1;
            char::from_u32(value).map_or_else(String::new, |value| value.to_string())
        }
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Enter => "\r".to_owned(),
        KeyCode::Tab => "\t".to_owned(),
        KeyCode::Backspace => "\u{7f}".to_owned(),
        KeyCode::Esc => "\u{1b}".to_owned(),
        KeyCode::Up => "\u{1b}[A".to_owned(),
        KeyCode::Down => "\u{1b}[B".to_owned(),
        KeyCode::Right => "\u{1b}[C".to_owned(),
        KeyCode::Left => "\u{1b}[D".to_owned(),
        KeyCode::Home => "\u{1b}[H".to_owned(),
        KeyCode::End => "\u{1b}[F".to_owned(),
        KeyCode::Delete => "\u{1b}[3~".to_owned(),
        KeyCode::PageUp => "\u{1b}[5~".to_owned(),
        KeyCode::PageDown => "\u{1b}[6~".to_owned(),
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
        let mut mapper =
            InputMapper::new(BTreeMap::from([("ctrl+k".to_owned(), "save".to_owned())]));
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
}
