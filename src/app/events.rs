use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEventKind};

use crate::Result;

use super::{Application, FocusOwner, global_command};

impl Application {
    pub(super) fn handle_event(&mut self, event: Event) -> Result<()> {
        if self.overlay.is_some() {
            self.input.cancel_leader();
            return self.handle_overlay_event(event);
        }
        if let Event::Resize(columns, rows) = event {
            self.input.cancel_leader();
            return self.resize_terminals(columns, rows);
        }
        if let Event::Mouse(mouse) = event {
            self.input.cancel_leader();
            return self.handle_mouse(mouse);
        }
        if self.focus_owner == FocusOwner::Explorer {
            return self.handle_explorer_event(event);
        }
        let terminal_focused = self.focus_owner == FocusOwner::BottomTerminal;
        let application_cursor = terminal_focused
            && self
                .active_bottom_terminal()
                .is_some_and(|session| session.modes().application_cursor);
        let command =
            self.input
                .map_with_application_cursor(event, terminal_focused, application_cursor);
        self.apply(command)
    }

    fn handle_explorer_event(&mut self, event: Event) -> Result<()> {
        let Event::Key(key) = event else {
            self.input.cancel_leader();
            return Ok(());
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Ok(());
        }
        let command = self.input.map(Event::Key(key), false);
        if global_command(&command) {
            return self.apply(command);
        }
        match key.code {
            KeyCode::Up => self.explorer.move_selection(-1),
            KeyCode::Down => self.explorer.move_selection(1),
            KeyCode::Enter | KeyCode::Right => {
                if let Some(path) = self.explorer.toggle_selected()? {
                    self.open_path(&path, None)?;
                    self.focus_owner = FocusOwner::Workspace;
                }
            }
            KeyCode::Left => {
                if (self.explorer.root_header_selected && self.explorer.root_expanded)
                    || self
                        .explorer
                        .selected_entry()
                        .is_some_and(|entry| entry.is_directory && entry.expanded)
                {
                    self.explorer.toggle_selected()?;
                }
            }
            KeyCode::Esc => self.focus_owner = FocusOwner::Workspace,
            KeyCode::Backspace | KeyCode::Delete
                if key.modifiers.is_empty() && key.kind == KeyEventKind::Press =>
            {
                self.open_explorer_delete();
            }
            KeyCode::Char('r') => {
                self.explorer.refresh()?;
                self.last_explorer_refresh = Instant::now();
            }
            _ => {}
        }
        Ok(())
    }
}
