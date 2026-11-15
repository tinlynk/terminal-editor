use std::{
    env,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use portable_pty::PtySize;

use crate::{
    AppError, Result, layout::Rect, render, terminal_emulator::MouseTrackingMode,
    terminal_session::TerminalSession, workspace::Pane,
};

use super::{Application, FocusOwner};

const MAX_BOTTOM_TERMINALS: usize = 8;

impl Application {
    pub(super) fn resize_terminals(&mut self, columns: u16, rows: u16) -> Result<()> {
        let geometry = render::geometry(
            self,
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
        );
        let editor_geometry = geometry
            .pane_layout
            .panes
            .iter()
            .filter_map(|(pane_id, area)| {
                let Pane::Editor { document_id, .. } = self.workspace.panes.get(pane_id)? else {
                    return None;
                };
                Some((
                    *pane_id,
                    *document_id,
                    crate::render::editor_content_geometry(
                        self,
                        *area,
                        self.documents.get(*document_id),
                    ),
                ))
            })
            .collect::<Vec<_>>();
        for (pane_id, document_id, content) in editor_geometry {
            let Some(Pane::Editor { view, .. }) = self.workspace.panes.get_mut(&pane_id) else {
                continue;
            };
            let document = self.documents.get(document_id);
            if self.config.effective.editor.soft_wrap {
                view.ensure_visible_wrapped(document, content.height, content.width);
            } else {
                view.clamp_viewport(document, content.height);
                view.viewport_wrap_segment = 0;
            }
        }
        if let Some(area) = geometry.terminal {
            for session in &mut self.bottom_terminals {
                session.resize_screen(
                    area.height.saturating_sub(2).max(1),
                    area.width.saturating_sub(2).max(1),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn terminal_at(
        &self,
        geometry: &render::WorkbenchGeometry,
        column: u16,
        row: u16,
    ) -> Option<Rect> {
        if let (true, Some(area)) = (!self.bottom_terminals.is_empty(), geometry.terminal)
            && terminal_content_contains(area, column, row)
        {
            return Some(area);
        }
        None
    }

    pub(super) fn forward_terminal_mouse(
        &mut self,
        geometry: &render::WorkbenchGeometry,
        mouse: MouseEvent,
    ) -> Result<bool> {
        let Some(area) = geometry.terminal.filter(|area| {
            !self.bottom_terminals.is_empty()
                && terminal_content_contains(*area, mouse.column, mouse.row)
        }) else {
            return Ok(false);
        };
        let Some(session) = self.active_bottom_terminal_mut() else {
            return Ok(false);
        };
        if !session.mouse_reporting() {
            return Ok(false);
        }
        let modes = session.modes();
        let mouse_tracking = modes.mouse_tracking();
        if (matches!(mouse.kind, MouseEventKind::Drag(_))
            && mouse_tracking == MouseTrackingMode::Normal)
            || (matches!(mouse.kind, MouseEventKind::Moved)
                && mouse_tracking != MouseTrackingMode::AnyEvent)
        {
            return Ok(false);
        }
        let Some((button, release)) = terminal_mouse_code(mouse.kind) else {
            return Ok(false);
        };
        let mut code = button;
        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
            code += 4;
        }
        if mouse.modifiers.contains(KeyModifiers::ALT) {
            code += 8;
        }
        if mouse.modifiers.contains(KeyModifiers::CONTROL) {
            code += 16;
        }
        let column = mouse.column.saturating_sub(area.x).max(1);
        let row = mouse.row.saturating_sub(area.y).max(1);
        session.write(&terminal_mouse_sequence(
            modes.mouse_sgr,
            code,
            column,
            row,
            release,
        ))?;
        self.focus_owner = FocusOwner::BottomTerminal;
        Ok(true)
    }

    pub(super) fn toggle_bottom_terminal(&mut self) -> Result<()> {
        if self.bottom_terminals.is_empty() {
            self.bottom_terminals.push(self.spawn_terminal(None)?);
            self.active_bottom_terminal = 0;
        }
        self.bottom_terminal_visible = !self.bottom_terminal_visible;
        self.focus_owner = if self.bottom_terminal_visible {
            FocusOwner::BottomTerminal
        } else {
            FocusOwner::Workspace
        };
        if self.bottom_terminal_visible {
            let (columns, rows) = crossterm::terminal::size()
                .map_err(|error| AppError::io("could not inspect terminal size", error))?;
            self.resize_terminals(columns, rows)?;
        }
        Ok(())
    }

    pub(super) fn close_active_bottom_terminal(&mut self) {
        if self.bottom_terminals.is_empty() {
            return;
        }
        let mut session = self.bottom_terminals.remove(self.active_bottom_terminal);
        session.begin_termination();
        self.active_bottom_terminal = self
            .active_bottom_terminal
            .min(self.bottom_terminals.len().saturating_sub(1));
        if self.bottom_terminals.is_empty() {
            self.bottom_terminal_visible = false;
            self.focus_owner = FocusOwner::Workspace;
        }
    }

    pub(super) fn new_bottom_terminal(&mut self) -> Result<()> {
        if self.bottom_terminals.len() >= MAX_BOTTOM_TERMINALS {
            self.message = Some(format!("Terminal limit reached ({MAX_BOTTOM_TERMINALS})"));
            return Ok(());
        }
        self.bottom_terminals.push(self.spawn_terminal(None)?);
        self.active_bottom_terminal = self.bottom_terminals.len().saturating_sub(1);
        self.bottom_terminal_visible = true;
        self.focus_owner = FocusOwner::BottomTerminal;
        let (columns, rows) = crossterm::terminal::size()
            .map_err(|error| AppError::io("could not inspect terminal size", error))?;
        self.resize_terminals(columns, rows)
    }

    pub(super) fn cycle_bottom_terminal(&mut self, delta: isize) {
        let count = self.bottom_terminals.len();
        if count == 0 {
            return;
        }
        self.active_bottom_terminal =
            (self.active_bottom_terminal as isize + delta).rem_euclid(count as isize) as usize;
        self.bottom_terminal_visible = true;
        self.focus_owner = FocusOwner::BottomTerminal;
    }

    pub(super) fn spawn_terminal(&self, directory: Option<&Path>) -> Result<TerminalSession> {
        let shell = self
            .config
            .effective
            .terminal
            .shell
            .clone()
            .or_else(|| env::var_os("SHELL").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        let working_directory =
            directory.map_or_else(|| self.explorer.root.clone(), Path::to_owned);
        TerminalSession::spawn(
            &shell,
            &self.config.effective.terminal.arguments,
            &working_directory,
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
            self.config.effective.terminal.scrollback_lines,
            self.config.effective.terminal.allow_child_osc52_write,
        )
    }
}

fn terminal_content_contains(area: Rect, column: u16, row: u16) -> bool {
    column > area.x
        && column < area.x.saturating_add(area.width.saturating_sub(1))
        && row > area.y
        && row < area.y.saturating_add(area.height.saturating_sub(1))
}

pub(super) fn terminal_mouse_code(kind: MouseEventKind) -> Option<(u16, bool)> {
    let button = |button| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    match kind {
        MouseEventKind::Down(value) => Some((button(value), false)),
        MouseEventKind::Up(_) => Some((3, true)),
        MouseEventKind::Drag(value) => Some((button(value) + 32, false)),
        MouseEventKind::Moved => Some((35, false)),
        MouseEventKind::ScrollUp => Some((64, false)),
        MouseEventKind::ScrollDown => Some((65, false)),
        _ => None,
    }
}

pub(super) fn terminal_mouse_sequence(
    sgr: bool,
    code: u16,
    column: u16,
    row: u16,
    release: bool,
) -> Vec<u8> {
    if sgr {
        return format!(
            "\x1b[<{code};{column};{row}{}",
            if release { 'm' } else { 'M' }
        )
        .into_bytes();
    }
    [
        0x1b,
        b'[',
        b'M',
        u8::try_from(code.saturating_add(32).min(255)).unwrap_or(255),
        u8::try_from(column.saturating_add(32).min(255)).unwrap_or(255),
        u8::try_from(row.saturating_add(32).min(255)).unwrap_or(255),
    ]
    .to_vec()
}
