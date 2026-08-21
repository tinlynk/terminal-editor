use std::time::Instant;

use crate::{Result, config::ConfigScope, input::InputMapper, theme::ThemeRegistry};

use super::{Application, FocusOwner};
use crate::workspace::Pane;

impl Application {
    pub(super) fn reload_config_after_raw_save(&mut self) -> Result<()> {
        let previous_mouse = self.config.effective.mouse.enabled;
        let mut reloaded = self.config.clone();
        reloaded.reload()?;
        if reloaded
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.starts_with("could not parse "))
        {
            self.config.diagnostics = reloaded.diagnostics;
            self.message = Some(
                "Configuration contains invalid TOML; the previous runtime settings remain active"
                    .to_owned(),
            );
            return Ok(());
        }
        self.config = reloaded;
        self.apply_effective_config(previous_mouse)
    }

    pub(super) fn apply_effective_config(&mut self, previous_mouse: bool) -> Result<()> {
        self.input = InputMapper::new(
            self.config.effective.keybindings.clone(),
            &self.config.effective.terminal.leader,
        );
        self.explorer.visible = self.config.effective.explorer.visible;
        self.explorer
            .set_exclusions(&self.config.effective.explorer.exclusions)?;
        if !self.explorer.visible && self.focus_owner == FocusOwner::Explorer {
            self.focus_owner = FocusOwner::Workspace;
        }
        self.last_explorer_refresh = Instant::now();
        self.workspace
            .set_initial_split_ratio(self.config.effective.workspace.initial_split_ratio);
        let history_limit = self
            .config
            .effective
            .editor
            .undo_limit_mib
            .saturating_mul(1024 * 1024);
        self.documents.apply_editor_defaults(
            self.config.effective.editor.default_line_ending,
            history_limit,
        );
        for pane in self.workspace.panes.values_mut() {
            if let Pane::Editor { view, .. } = pane {
                if self.config.effective.editor.soft_wrap {
                    view.viewport_column = 0;
                } else {
                    view.viewport_wrap_segment = 0;
                }
            }
        }
        for view in self.saved_editor_views.values_mut() {
            if self.config.effective.editor.soft_wrap {
                view.viewport_column = 0;
            } else {
                view.viewport_wrap_segment = 0;
            }
        }
        if let Some(session) = self.bottom_terminal.as_mut() {
            session.apply_runtime_config(
                self.config.effective.terminal.scrollback_lines,
                self.config.effective.terminal.allow_child_osc52_write,
            );
        }
        if previous_mouse != self.config.effective.mouse.enabled {
            self.pending_mouse_capture = Some(self.config.effective.mouse.enabled);
        }
        let bottom_terminal_visible = self.config.effective.ui.bottom_terminal_visible;
        if bottom_terminal_visible && self.bottom_terminal.is_none() {
            self.bottom_terminal = Some(self.spawn_terminal(None)?);
        }
        self.bottom_terminal_visible = bottom_terminal_visible;
        if !self.bottom_terminal_visible && self.focus_owner == FocusOwner::BottomTerminal {
            self.focus_owner = FocusOwner::Workspace;
        }
        self.apply_configured_theme(false);
        Ok(())
    }

    pub(super) fn rescan_themes(&mut self, retain_last_valid: bool) -> Result<()> {
        self.theme_registry = ThemeRegistry::load(self.theme_registry.directory().to_owned())?;
        self.apply_configured_theme(retain_last_valid);
        Ok(())
    }

    pub(super) fn select_theme(&mut self, scope: ConfigScope, theme_id: &str) -> Result<()> {
        self.config.set_theme_active(scope, theme_id)?;
        self.apply_configured_theme(false);
        Ok(())
    }

    fn apply_configured_theme(&mut self, retain_last_valid: bool) {
        let requested = &self.config.effective.theme.active;
        if let Some(entry) = self.theme_registry.get(requested) {
            self.theme = entry.theme.clone();
            if let Some(diagnostic) = entry.diagnostics.first() {
                self.message = Some(format!("Theme `{requested}`: {diagnostic}"));
            }
            return;
        }
        if retain_last_valid && self.theme.id == *requested {
            self.message = Some(format!(
                "Theme `{requested}` could not be reloaded; the previous valid version remains active"
            ));
            return;
        }
        let (theme, message) = self.theme_registry.resolve(requested);
        self.theme = theme.clone();
        self.message = message;
    }
}
