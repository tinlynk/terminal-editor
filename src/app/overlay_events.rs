use std::path::Path;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::{AppError, Result, config::ConfigScope, layout::Rect, render};

use super::{
    Application, FocusOwner, Overlay, SETTINGS_KEYS, file_dialog, move_settings_selection,
};

impl Application {
    pub(super) fn handle_overlay_event(&mut self, event: Event) -> Result<()> {
        if let Event::Mouse(mouse) = event {
            let click_count = matches!(mouse.kind, MouseEventKind::Down(_))
                .then(|| self.register_click(mouse.column, mouse.row));
            let click_area = if click_count.is_some() {
                let (columns, rows) = crossterm::terminal::size()
                    .map_err(|error| AppError::io("could not inspect terminal size", error))?;
                self.overlay.as_ref().map(|overlay| {
                    render::overlay_area(
                        Rect {
                            x: 0,
                            y: 0,
                            width: columns,
                            height: rows,
                        },
                        overlay,
                    )
                })
            } else {
                None
            };
            let mut clicked_entry = false;
            if let Some(Overlay::FileDialog {
                entries, selected, ..
            }) = self.overlay.as_mut()
            {
                match mouse.kind {
                    MouseEventKind::ScrollUp => *selected = selected.saturating_sub(3),
                    MouseEventKind::ScrollDown => {
                        *selected = selected
                            .saturating_add(3)
                            .min(entries.len().saturating_sub(1));
                    }
                    MouseEventKind::Down(_) => {
                        let area = click_area.expect("mouse-down overlay area must exist");
                        let visible = render::file_dialog_visible_rows(area);
                        let start =
                            render::file_dialog_view_start(*selected, entries.len(), visible);
                        if let Some(offset) = mouse.row.checked_sub(area.y.saturating_add(3))
                            && usize::from(offset) < visible
                        {
                            let index = start.saturating_add(usize::from(offset));
                            if index < entries.len() {
                                *selected = index;
                                clicked_entry = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if clicked_entry && click_count.is_some_and(|count| count >= 2) {
                return self.activate_file_dialog_selection();
            }
            return Ok(());
        }
        let Event::Key(key) = event else {
            return Ok(());
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Ok(());
        }
        if key.code == KeyCode::Esc {
            if matches!(
                self.overlay,
                Some(Overlay::FileDialog { save_as: true, .. })
                    | Some(Overlay::SaveConflict)
                    | Some(Overlay::ConfirmSaveAsOverwrite { .. })
            ) {
                self.pending_close_document = None;
            }
            self.overlay = None;
            self.focus_owner = FocusOwner::Workspace;
            return Ok(());
        }
        if key.code == KeyCode::Enter && matches!(self.overlay, Some(Overlay::FileDialog { .. })) {
            let save_as = matches!(
                self.overlay,
                Some(Overlay::FileDialog { save_as: true, .. })
            );
            return if save_as || key.modifiers.contains(KeyModifiers::CONTROL) {
                self.submit_file_dialog()
            } else {
                self.activate_file_dialog_selection()
            };
        }
        if key.code == KeyCode::Enter
            && let Some(Overlay::Settings {
                scope, selected, ..
            }) = self.overlay.as_ref()
        {
            if SETTINGS_KEYS[*selected] == "theme.active" {
                let scope = *scope;
                self.rescan_themes(true)?;
                let selected = self
                    .theme_registry
                    .entries()
                    .position(|(id, _)| id == self.config.effective.theme.active)
                    .unwrap_or(0);
                self.overlay = Some(Overlay::ThemePicker { scope, selected });
                return Ok(());
            }
            let path = self.config.ensure_raw_config(*scope)?;
            self.open_path(&path, None)?;
            self.overlay = None;
            self.focus_owner = FocusOwner::Workspace;
            return Ok(());
        }
        if key.code == KeyCode::Enter
            && let Some(Overlay::ThemePicker { scope, selected }) = self.overlay.as_ref()
        {
            let scope = *scope;
            let theme_id = self
                .theme_registry
                .entries()
                .nth(*selected)
                .map(|(id, _)| id.to_owned());
            if let Some(theme_id) = theme_id {
                self.select_theme(scope, &theme_id)?;
                self.overlay = None;
                self.focus_owner = FocusOwner::Workspace;
            }
            return Ok(());
        }
        if matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
            && matches!(self.overlay, Some(Overlay::ThemePicker { .. }))
        {
            self.rescan_themes(true)?;
            let count = self.theme_registry.entries().count();
            if let Some(Overlay::ThemePicker { selected, .. }) = self.overlay.as_mut() {
                *selected = (*selected).min(count.saturating_sub(1));
            }
            return Ok(());
        }
        if key.modifiers == KeyModifiers::CONTROL
            && key.code == KeyCode::Char('r')
            && let Some(Overlay::Settings {
                scope, selected, ..
            }) = self.overlay.as_ref()
        {
            let previous_mouse = self.config.effective.mouse.enabled;
            self.config
                .reset_setting(*scope, SETTINGS_KEYS[*selected])?;
            self.apply_effective_config(previous_mouse)?;
            return Ok(());
        }
        match self.overlay.as_mut().expect("overlay must exist") {
            Overlay::FileDialog {
                directory,
                input,
                entries,
                selected,
                ..
            } => match key.code {
                KeyCode::Backspace if *input == directory.display().to_string() => {
                    let parent = directory.parent().map(Path::to_owned);
                    let save_as = matches!(
                        self.overlay,
                        Some(Overlay::FileDialog { save_as: true, .. })
                    );
                    if let Some(parent) = parent {
                        self.overlay = Some(file_dialog(&parent, save_as));
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    *selected = selected
                        .saturating_add(1)
                        .min(entries.len().saturating_sub(1));
                }
                KeyCode::Tab => {
                    if let Some(path) = entries.get(*selected) {
                        *input = path.display().to_string();
                    }
                }
                KeyCode::Right => return self.activate_file_dialog_selection(),
                KeyCode::Left => {
                    let parent = directory.parent().map(Path::to_owned);
                    let save_as = matches!(
                        self.overlay,
                        Some(Overlay::FileDialog { save_as: true, .. })
                    );
                    if let Some(parent) = parent {
                        self.overlay = Some(file_dialog(&parent, save_as));
                    }
                }
                KeyCode::Char(character)
                    if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
                {
                    input.push(character);
                }
                _ => {}
            },
            Overlay::Settings {
                query,
                scope,
                selected,
            } => match key.code {
                KeyCode::Backspace => {
                    query.pop();
                    move_settings_selection(query, selected, 0);
                }
                KeyCode::Char(character)
                    if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
                {
                    query.push(character);
                    move_settings_selection(query, selected, 0);
                }
                KeyCode::Tab => {
                    *scope = match scope {
                        ConfigScope::User => ConfigScope::Workspace,
                        ConfigScope::Workspace | ConfigScope::Default => ConfigScope::User,
                    };
                }
                KeyCode::Up => move_settings_selection(query, selected, -1),
                KeyCode::Down => move_settings_selection(query, selected, 1),
                _ => {}
            },
            Overlay::ThemePicker { selected, .. } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    *selected = selected
                        .saturating_add(1)
                        .min(self.theme_registry.entries().count().saturating_sub(1));
                }
                _ => {}
            },
            Overlay::ConfirmQuit
                if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('q') =>
            {
                self.request_quit();
            }
            Overlay::ConfirmTerminateAll if key.code == KeyCode::Char('t') => {
                if let Some(session) = self.bottom_terminal.as_mut() {
                    session.begin_termination();
                }
                self.running = false;
            }
            Overlay::ConfirmTerminalClose if key.code == KeyCode::Char('t') => {
                if let Some(session) = self.bottom_terminal.as_mut() {
                    session.begin_termination();
                }
                self.bottom_terminal = None;
                self.bottom_terminal_visible = false;
                self.focus_owner = FocusOwner::Workspace;
                self.overlay = None;
            }
            Overlay::ConfirmDocumentClose if key.code == KeyCode::Char('d') => {
                self.discard_focused_document();
                self.overlay = None;
                self.focus_owner = FocusOwner::Workspace;
            }
            Overlay::ConfirmDocumentClose if key.code == KeyCode::Char('s') => {
                self.pending_close_document = self.focused_editor().map(|(id, _)| id);
                self.save_focused()?;
            }
            Overlay::SaveConflict if key.code == KeyCode::Char('o') => {
                let document_id = self.focused_editor().map(|(id, _)| id);
                if let Some(document_id) = document_id {
                    self.documents.save(document_id, true)?;
                    self.finish_successful_save(document_id)?;
                }
                self.overlay = None;
                self.focus_owner = FocusOwner::Workspace;
            }
            Overlay::ConfirmSaveAsOverwrite { document_id, path }
                if key.code == KeyCode::Char('o') =>
            {
                let document_id = *document_id;
                let path = path.clone();
                self.documents.save_as(document_id, &path, true)?;
                self.overlay = None;
                self.focus_owner = FocusOwner::Workspace;
                self.finish_successful_save(document_id)?;
            }
            Overlay::Message { .. }
            | Overlay::ConfirmQuit
            | Overlay::ConfirmTerminateAll
            | Overlay::ConfirmTerminalClose
            | Overlay::ConfirmDocumentClose
            | Overlay::SaveConflict
            | Overlay::ConfirmSaveAsOverwrite { .. } => {}
        }
        Ok(())
    }
}
