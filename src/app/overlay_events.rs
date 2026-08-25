use std::path::Path;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::{
    AppError, Result, config::ConfigScope, file_io::remove_recovery_path, input::key_sequence,
    layout::Rect, render,
};

use super::{
    Application, FocusOwner, Overlay, SETTINGS_KEYS, explorer_create_dialog, file_dialog,
    move_settings_selection,
};

impl Application {
    fn help_maximum_scroll(&self) -> Result<usize> {
        let (columns, rows) = crossterm::terminal::size()
            .map_err(|error| AppError::io("could not inspect terminal size", error))?;
        let area = render::overlay_area(
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
            self.overlay.as_ref().expect("help overlay exists"),
        );
        Ok(render::help_line_count(self).saturating_sub(usize::from(area.height.saturating_sub(2))))
    }

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
            if matches!(self.overlay, Some(Overlay::Help { .. })) {
                let maximum = self.help_maximum_scroll()?;
                if let Some(Overlay::Help { scroll }) = self.overlay.as_mut() {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => *scroll = scroll.saturating_sub(3),
                        MouseEventKind::ScrollDown => {
                            *scroll = scroll.saturating_add(3).min(maximum)
                        }
                        _ => {}
                    }
                }
                return Ok(());
            }
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
            // Extract query before mutable borrow to avoid conflict
            let query: String = self.overlay.as_ref().and_then(|o| {
                if let Overlay::Settings { query, .. } = o {
                    Some(query.to_lowercase())
                } else {
                    None
                }
            }).unwrap_or_default();

            if let Some(Overlay::Settings {
                selected,
                editing,
                ..
            }) = self.overlay.as_mut()
            {
                match mouse.kind {
                    MouseEventKind::ScrollUp if editing.is_none() => {
                        move_settings_selection(&query, selected, -3);
                        return Ok(());
                    }
                    MouseEventKind::ScrollDown if editing.is_none() => {
                        move_settings_selection(&query, selected, 3);
                        return Ok(());
                    }
                    _ => {}
                }
                if let Some(area) = click_area {
                    if let MouseEventKind::Down(_) = mouse.kind
                        && editing.is_none()
                    {
                        let content_start = area.y + 2;
                        let clicked_line = mouse.row.saturating_sub(content_start);
                        if clicked_line < 12 {
                            let visible_keys: Vec<&str> = SETTINGS_KEYS
                                .iter()
                                .filter(|key| key.to_lowercase().contains(&query))
                                .copied()
                                .collect();
                            let visible_count = 12.min(visible_keys.len());
                            let start = (*selected)
                                .saturating_sub(10)
                                .min(visible_keys.len().saturating_sub(visible_count));
                            if let Some(index) = visible_keys.get(start.saturating_add(clicked_line as usize))
                                && let Some(setting_index) = SETTINGS_KEYS.iter().position(|key| *key == *index)
                            {
                                *selected = setting_index;
                            }
                        }
                    }
                    return Ok(());
                }
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
            if let Some(Overlay::ExternalFileChange {
                document_id,
                version,
                ..
            }) = self.overlay.as_ref()
            {
                self.deferred_external_changes
                    .insert(*document_id, version.clone());
            }
            if matches!(
                self.overlay,
                Some(Overlay::FileDialog { save_as: true, .. })
                    | Some(Overlay::SaveConflict)
                    | Some(Overlay::ConfirmSaveAsOverwrite { .. })
                    | Some(Overlay::ConfirmDocumentClose)
            ) {
                self.pending_close_document = None;
                self.quit_in_progress = false;
            }
            if let Some(Overlay::Settings { editing, .. }) = self.overlay.as_mut()
                && editing.is_some()
            {
                *editing = None;
                return Ok(());
            }
            self.overlay = None;
            self.focus_owner = FocusOwner::Workspace;
            return Ok(());
        }
        if key.code == KeyCode::Char('r')
            && let Some((path, record)) = self.overlay.as_ref().and_then(|overlay| {
                let Overlay::Recovery { path, record } = overlay else {
                    return None;
                };
                Some((path.clone(), record.clone()))
            })
        {
            self.restore_recovery_record(record)?;
            remove_recovery_path(&path)?;
            self.overlay = None;
            self.focus_owner = FocusOwner::Workspace;
            self.show_next_recovery();
            return Ok(());
        }
        if key.code == KeyCode::Char('d')
            && let Some(path) = self.overlay.as_ref().and_then(|overlay| {
                let Overlay::Recovery { path, .. } = overlay else {
                    return None;
                };
                Some(path.clone())
            })
        {
            remove_recovery_path(&path)?;
            self.overlay = None;
            self.focus_owner = FocusOwner::Workspace;
            self.show_next_recovery();
            return Ok(());
        }
        if key.code == KeyCode::Char('k')
            && let Some(document_id) = self.overlay.as_ref().and_then(|overlay| {
                let Overlay::ExternalFileChange { document_id, .. } = overlay else {
                    return None;
                };
                Some(*document_id)
            })
        {
            self.documents.accept_external_file_state(document_id)?;
            self.deferred_external_changes.remove(&document_id);
            self.overlay = None;
            self.focus_owner = FocusOwner::Workspace;
            return Ok(());
        }
        if key.code == KeyCode::Char('r')
            && let Some((document_id, missing)) = self.overlay.as_ref().and_then(|overlay| {
                let Overlay::ExternalFileChange {
                    document_id,
                    missing,
                    ..
                } = overlay else {
                    return None;
                };
                Some((*document_id, *missing))
            })
        {
            if missing {
                self.documents.accept_external_file_state(document_id)?;
                self.message = Some("The missing file was kept open with local contents".to_owned());
            } else {
                self.documents.reload(document_id, true)?;
            }
            self.deferred_external_changes.remove(&document_id);
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
        if key.code == KeyCode::Enter && matches!(self.overlay, Some(Overlay::ExplorerCreate { .. })) {
            return self.submit_explorer_create();
        }
        if key.code == KeyCode::Char('o')
            && let Some((path, split)) = self.overlay.as_ref().and_then(|overlay| {
                let Overlay::ConfirmUnsafeOpen { path, split, .. } = overlay else {
                    return None;
                };
                Some((path.clone(), *split))
            })
        {
            super::document_lifecycle::open_confirmed_path(self, &path, split)?;
            self.overlay = None;
            self.focus_owner = FocusOwner::Workspace;
            return Ok(());
        }
        if matches!(self.overlay, Some(Overlay::SearchReplace { .. })) {
            if key.modifiers == (KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                && key.code == KeyCode::Char('r')
            {
                self.replace_all_search_matches();
                return Ok(());
            }
            if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('r') {
                self.replace_search_match();
                return Ok(());
            }
            match key.code {
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.select_search_match(false);
                }
                KeyCode::Enter => self.select_search_match(true),
                KeyCode::Tab => {
                    if let Some(Overlay::SearchReplace {
                        editing_replacement,
                        ..
                    }) = self.overlay.as_mut()
                    {
                        *editing_replacement = !*editing_replacement;
                    }
                }
                KeyCode::Backspace => {
                    let mut query_changed = false;
                    if let Some(Overlay::SearchReplace {
                        query,
                        replacement,
                        editing_replacement,
                        current_match,
                        ..
                    }) = self.overlay.as_mut()
                    {
                        if *editing_replacement {
                            replacement.pop();
                        } else {
                            query.pop();
                            *current_match = None;
                            query_changed = true;
                        }
                    }
                    if query_changed {
                        self.select_search_match(true);
                    }
                }
                KeyCode::Char(character)
                    if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
                {
                    let mut query_changed = false;
                    if let Some(Overlay::SearchReplace {
                        query,
                        replacement,
                        editing_replacement,
                        current_match,
                        ..
                    }) = self.overlay.as_mut()
                    {
                        if *editing_replacement {
                            replacement.push(character);
                        } else {
                            query.push(character);
                            *current_match = None;
                            query_changed = true;
                        }
                    }
                    if query_changed {
                        self.select_search_match(true);
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        if matches!(self.overlay, Some(Overlay::Help { .. })) {
            let maximum = self.help_maximum_scroll()?;
            if let Some(Overlay::Help { scroll }) = self.overlay.as_mut() {
                match key.code {
                    KeyCode::Up => *scroll = scroll.saturating_sub(1),
                    KeyCode::Down => *scroll = scroll.saturating_add(1).min(maximum),
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                    KeyCode::PageDown => *scroll = scroll.saturating_add(10).min(maximum),
                    _ => {}
                }
            }
            return Ok(());
        }
        if key.code == KeyCode::Enter
            && let Some(Overlay::Settings {
                scope, selected, editing, new_value, ..
            }) = self.overlay.as_mut()
        {
            // If editing a value, confirm it
            if let Some(editing_selected) = editing {
                let key = SETTINGS_KEYS[*editing_selected];
                let value = new_value.clone();
                let previous_mouse = self.config.effective.mouse.enabled;
                let saved = match self.config.set_setting_from_string(*scope, key, &value) {
                    Ok(()) => true,
                    Err(error) => {
                        self.message = Some(format!("Invalid value: {error}"));
                        false
                    }
                };
                *editing = None;
                *new_value = String::new();
                if saved {
                    self.apply_effective_config(previous_mouse)?;
                }
                return Ok(());
            }
            // Normal Enter: start editing the selected setting
            {
                let key = SETTINGS_KEYS[*selected];
                // theme.active opens theme picker (but we keep it as editable)
                if key == "theme.active" {
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
                // Start editing: populate new_value with current value
                *editing = Some(*selected);
                let current_value = self.config.effective.get_setting_value(key);
                *new_value = current_value;
            }
            return Ok(());
        }
        if let Some((scope, current_key, replacement_key)) = self.overlay.as_ref().and_then(|overlay| {
            let Overlay::Settings {
                scope,
                editing: Some(setting_index),
                ..
            } = overlay else {
                return None;
            };
            SETTINGS_KEYS[*setting_index]
                .strip_prefix("keybindings.")
                .map(|current_key| (*scope, current_key.to_owned()))
        }).and_then(|(scope, current_key)| {
            key_sequence(key).map(|replacement_key| (scope, current_key, replacement_key))
        }) {
            let previous_mouse = self.config.effective.mouse.enabled;
            match self.config.rebind_keybinding(scope, &current_key, &replacement_key) {
                Ok(()) => self.apply_effective_config(previous_mouse)?,
                Err(error) => self.message = Some(format!("Invalid keybinding: {error}")),
            }
            if let Some(Overlay::Settings { editing, new_value, .. }) = self.overlay.as_mut() {
                *editing = None;
                *new_value = String::new();
            }
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
                        self.overlay = Some(file_dialog(&parent, save_as, &self.explorer));
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
                        self.overlay = Some(file_dialog(&parent, save_as, &self.explorer));
                    }
                }
                KeyCode::Char(character)
                    if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
                {
                    input.push(character);
                }
                _ => {}
            },
            Overlay::ExplorerCreate { directory, name, entries, selected, create_directory, .. } => match key.code {
                KeyCode::Backspace => { name.pop(); }
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = selected.saturating_add(1).min(entries.len().saturating_sub(1)),
                KeyCode::Right => {
                    if let Some(path) = entries.get(*selected) { self.overlay = Some(explorer_create_dialog(path, *create_directory)); }
                }
                KeyCode::Left => {
                    if let Some(parent) = directory.parent().filter(|parent| parent.starts_with(&self.explorer.root)) { self.overlay = Some(explorer_create_dialog(parent, *create_directory)); }
                }
                KeyCode::Char(character) if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) => name.push(character),
                _ => {}
            },
            Overlay::Settings {
                query,
                scope,
                selected,
                editing,
                new_value,
            } => {
                if editing.is_some() {
                    // Editing mode: update new_value
                    match key.code {
                        KeyCode::Up => cycle_setting_option(
                            SETTINGS_KEYS[editing.expect("editing state exists")],
                            new_value,
                            -1,
                        ),
                        KeyCode::Down => cycle_setting_option(
                            SETTINGS_KEYS[editing.expect("editing state exists")],
                            new_value,
                            1,
                        ),
                        KeyCode::Backspace => {
                            new_value.pop();
                        }
                        KeyCode::Char(character)
                            if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
                                && setting_accepts_character(
                                    SETTINGS_KEYS[editing.expect("editing state exists")],
                                    new_value,
                                    character,
                                ) =>
                        {
                            new_value.push(character);
                        }
                        _ => {}
                    }
                } else {
                    // Search mode: update query
                    match key.code {
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
                    }
                }
            }
            Overlay::ThemePicker { selected, .. } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    *selected = selected
                        .saturating_add(1)
                        .min(self.theme_registry.entries().count().saturating_sub(1));
                }
                _ => {}
            },
            Overlay::SearchReplace { .. } => {}
            Overlay::ConfirmQuit
                if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('q') =>
            {
                self.request_quit();
            }
            Overlay::ConfirmTerminateAll if key.code == KeyCode::Char('t') => {
                for session in &mut self.bottom_terminals {
                    session.begin_termination();
                }
                self.running = false;
            }
            Overlay::ConfirmTerminalClose if key.code == KeyCode::Char('t') => {
                self.close_active_bottom_terminal();
                self.overlay = None;
            }
            Overlay::ConfirmDocumentClose if key.code == KeyCode::Char('d') => {
                self.discard_focused_document();
                self.overlay = None;
                self.focus_owner = FocusOwner::Workspace;
                if self.quit_in_progress {
                    self.continue_quit_sequence()?;
                }
            }
            Overlay::ConfirmDocumentClose if key.code == KeyCode::Char('s') => {
                self.pending_close_document = self.focused_editor().map(|(id, _)| id);
                self.save_focused()?;
            }
            Overlay::ConfirmUnsafeOpen { .. } | Overlay::Recovery { .. } | Overlay::ExternalFileChange { .. } => {}
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
            | Overlay::Help { .. }
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

fn setting_accepts_character(key: &str, value: &str, character: char) -> bool {
    const INTEGER_SETTINGS: &[&str] = &[
        "editor.indent_width",
        "editor.undo_limit_mib",
        "editor.syntax_highlighting_max_file_size_kib",
        "editor.autosave_interval_seconds",
        "workspace.minimum_pane_width",
        "workspace.minimum_pane_height",
        "workspace.vertical_divider_thickness",
        "workspace.horizontal_divider_thickness",
        "explorer.width",
        "explorer.divider_thickness",
        "explorer.refresh_interval_seconds",
        "terminal.scrollback_lines",
        "ui.bottom_terminal_height",
        "ui.terminal_divider_thickness",
    ];
    if INTEGER_SETTINGS.contains(&key) {
        return character.is_ascii_digit() || (character == '-' && value.is_empty());
    }
    if key == "workspace.initial_split_ratio" {
        return character.is_ascii_digit() || (character == '.' && !value.contains('.'));
    }
    true
}

pub(crate) fn setting_options(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "editor.insert_spaces" | "editor.line_numbers" | "editor.scrollbar"
        | "editor.soft_wrap" | "editor.syntax_highlighting"
        | "editor.check_modified_files_on_quit" | "explorer.visible"
        | "mouse.enabled" | "clipboard.osc52"
        | "terminal.allow_child_osc52_write" | "ui.bottom_terminal_visible" => {
            Some(&["true", "false"])
        }
        "editor.default_line_ending" => Some(&["lf", "crlf"]),
        "editor.header_style" => Some(&["tabs", "breadcrumbs"]),
        _ => None,
    }
}

fn cycle_setting_option(key: &str, value: &mut String, direction: isize) {
    let Some(options) = setting_options(key) else {
        return;
    };
    let index = options.iter().position(|option| *option == value).unwrap_or(0);
    let next = index.saturating_add_signed(direction).min(options.len().saturating_sub(1));
    *value = options[next].to_owned();
}
