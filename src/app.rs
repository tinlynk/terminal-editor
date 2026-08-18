use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use portable_pty::PtySize;

use crate::{
    AppError, Result,
    command::Command,
    config::{ConfigScope, LoadedConfig},
    explorer::{Explorer, root_for_startup},
    file_io::{DocumentRegistry, SaveOutcome},
    input::InputMapper,
    layout::{Rect, SplitDirection},
    render,
    terminal::{TerminalGuard, install_panic_restoration_hook},
    terminal_emulator::MouseTrackingMode,
    terminal_session::TerminalSession,
    workspace::{Pane, TerminalId, Workspace},
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkbenchDrag {
    Explorer,
    BottomTerminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusOwner {
    Explorer,
    Workspace,
    BottomTerminal,
    Overlay,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overlay {
    FileDialog {
        directory: PathBuf,
        input: String,
        entries: Vec<PathBuf>,
        selected: usize,
        save_as: bool,
        error: Option<String>,
    },
    Settings {
        query: String,
        scope: ConfigScope,
        selected: usize,
    },
    Message {
        title: String,
        message: String,
    },
    ConfirmQuit,
    ConfirmTerminateAll,
    ConfirmTerminalClose {
        terminal_id: TerminalId,
        bottom: bool,
    },
    ConfirmDocumentClose,
    SaveConflict,
}

#[derive(Debug)]
pub struct Application {
    pub config: LoadedConfig,
    pub documents: DocumentRegistry,
    pub workspace: Workspace,
    pub explorer: Explorer,
    pub terminal_sessions: BTreeMap<TerminalId, TerminalSession>,
    pub bottom_terminal: Option<TerminalId>,
    pub bottom_terminal_visible: bool,
    pub focus_owner: FocusOwner,
    pub overlay: Option<Overlay>,
    pub clipboard: String,
    pub message: Option<String>,
    pending_terminal_control: Vec<u8>,
    running: bool,
    input: InputMapper,
    drag_split: Option<crate::layout::SplitterRect>,
    workbench_drag: Option<WorkbenchDrag>,
    selection_drag: bool,
    scrollbar_drag: bool,
    terminal_selection_drag: Option<TerminalId>,
    last_explorer_refresh: Instant,
    last_click: Option<(Instant, u16, u16)>,
    click_count: u8,
}

impl Application {
    pub fn new(startup_path: Option<PathBuf>) -> Result<Self> {
        let current_directory = env::current_dir()
            .map_err(|error| AppError::io("could not determine current directory", error))?;
        let explorer_root = root_for_startup(startup_path.as_deref(), &current_directory);
        let config = LoadedConfig::load(&explorer_root)?;
        let history_limit = config
            .effective
            .editor
            .undo_limit_mib
            .saturating_mul(1024 * 1024);
        let mut documents =
            DocumentRegistry::new(config.effective.editor.default_line_ending, history_limit);
        let workspace = match startup_path.as_deref() {
            Some(path) if path.is_file() => Workspace::with_editor(
                documents.open(path)?.0,
                config.effective.workspace.initial_split_ratio,
            ),
            Some(path) if !path.exists() => Workspace::with_editor(
                documents.new_at_path(path.to_owned())?,
                config.effective.workspace.initial_split_ratio,
            ),
            _ => Workspace::empty(config.effective.workspace.initial_split_ratio),
        };
        let explorer = Explorer::new(
            explorer_root,
            config.effective.explorer.visible,
            &config.effective.explorer.exclusions,
        )?;
        let overlay = startup_path
            .as_deref()
            .filter(|path| path.is_dir())
            .map(|path| file_dialog(path, false));
        let input = InputMapper::new(config.effective.keybindings.clone());
        Ok(Self {
            bottom_terminal_visible: config.effective.ui.bottom_terminal_visible,
            config,
            documents,
            workspace,
            explorer,
            terminal_sessions: BTreeMap::new(),
            bottom_terminal: None,
            focus_owner: if overlay.is_some() {
                FocusOwner::Overlay
            } else {
                FocusOwner::Workspace
            },
            overlay,
            clipboard: String::new(),
            message: None,
            pending_terminal_control: Vec::new(),
            running: true,
            input,
            drag_split: None,
            workbench_drag: None,
            selection_drag: false,
            scrollbar_drag: false,
            terminal_selection_drag: None,
            last_explorer_refresh: Instant::now(),
            last_click: None,
            click_count: 0,
        })
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        let directory = tempfile::tempdir().unwrap().keep();
        let config = LoadedConfig {
            effective: crate::config::Config::default(),
            user_path: directory.join("user.toml"),
            workspace_path: directory.join("workspace.toml"),
            diagnostics: Vec::new(),
            provenance: BTreeMap::from([(
                "editor".to_owned(),
                crate::config::ConfigScope::Default,
            )]),
        };
        let mut documents = DocumentRegistry::new(crate::config::LineEnding::Lf, 1024);
        let document_id = documents.new_untitled();
        Self {
            workspace: Workspace::with_editor(document_id, 0.5),
            explorer: Explorer::new(directory, true, &[".git".to_owned()]).unwrap(),
            config,
            documents,
            terminal_sessions: BTreeMap::new(),
            bottom_terminal: None,
            bottom_terminal_visible: false,
            focus_owner: FocusOwner::Workspace,
            overlay: None,
            clipboard: String::new(),
            message: None,
            pending_terminal_control: Vec::new(),
            running: true,
            input: InputMapper::default(),
            drag_split: None,
            workbench_drag: None,
            selection_drag: false,
            scrollbar_drag: false,
            terminal_selection_drag: None,
            last_explorer_refresh: Instant::now(),
            last_click: None,
            click_count: 0,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        install_panic_restoration_hook();
        let mut terminal = TerminalGuard::enter(self.config.effective.mouse.enabled)?;
        let mut redraw = true;
        while self.running {
            if redraw {
                terminal
                    .terminal_mut()
                    .draw(|frame| render::draw(frame, self))
                    .map_err(|error| AppError::io("could not draw terminal frame", error))?;
                redraw = false;
            }
            if event::poll(EVENT_POLL_INTERVAL)
                .map_err(|error| AppError::io("could not poll terminal input", error))?
            {
                let event = event::read()
                    .map_err(|error| AppError::io("could not read terminal input", error))?;
                self.handle_event(event)?;
                redraw = true;
            }
            redraw |= self.refresh_explorer_if_due(Instant::now());
            if !self.pending_terminal_control.is_empty() {
                let control = std::mem::take(&mut self.pending_terminal_control);
                terminal.write_control(&control)?;
            }
            for session in self.terminal_sessions.values_mut() {
                redraw |= session.drain_output()?;
                if let Some(clipboard) = session.take_child_clipboard_write() {
                    self.clipboard = clipboard;
                }
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> Result<()> {
        if self.overlay.is_some() {
            return self.handle_overlay_event(event);
        }
        if let Event::Resize(columns, rows) = event {
            return self.resize_terminals(columns, rows);
        }
        if let Event::Mouse(mouse) = event {
            return self.handle_mouse(mouse);
        }
        if self.focus_owner == FocusOwner::Explorer {
            return self.handle_explorer_event(event);
        }
        let terminal_focused = self.focus_owner == FocusOwner::BottomTerminal
            || matches!(self.workspace.focused_pane(), Pane::Terminal { .. });
        let command = self.input.map(event, terminal_focused);
        self.apply(command)
    }

    fn handle_explorer_event(&mut self, event: Event) -> Result<()> {
        let Event::Key(key) = event else {
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
            KeyCode::Esc | KeyCode::Left => self.focus_owner = FocusOwner::Workspace,
            KeyCode::Char('r') => {
                self.explorer.refresh()?;
                self.last_explorer_refresh = Instant::now();
            }
            _ => {}
        }
        Ok(())
    }

    fn resize_terminals(&mut self, columns: u16, rows: u16) -> Result<()> {
        let geometry = render::geometry(
            self,
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
        );
        let sizes = geometry
            .pane_layout
            .panes
            .iter()
            .filter_map(|(pane_id, area)| match self.workspace.panes.get(pane_id) {
                Some(Pane::Terminal { terminal_id }) => Some((
                    *terminal_id,
                    area.height.saturating_sub(2).max(1),
                    area.width.saturating_sub(2).max(1),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (terminal_id, terminal_rows, terminal_columns) in sizes {
            if let Some(session) = self.terminal_sessions.get_mut(&terminal_id) {
                session.resize_screen(terminal_rows, terminal_columns)?;
            }
        }
        if let (Some(area), Some(terminal_id)) = (geometry.bottom_terminal, self.bottom_terminal)
            && let Some(session) = self.terminal_sessions.get_mut(&terminal_id)
        {
            session.resize_screen(
                area.height.saturating_sub(2).max(1),
                area.width.saturating_sub(2).max(1),
            )?;
        }
        Ok(())
    }

    fn handle_overlay_event(&mut self, event: Event) -> Result<()> {
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
            && let Some(Overlay::Settings { scope, .. }) = self.overlay.as_ref()
        {
            let path = self.config.ensure_raw_config(*scope)?;
            self.open_path(&path, None)?;
            self.overlay = None;
            self.focus_owner = FocusOwner::Workspace;
            return Ok(());
        }
        if key.modifiers == KeyModifiers::CONTROL
            && key.code == KeyCode::Char('r')
            && let Some(Overlay::Settings {
                scope, selected, ..
            }) = self.overlay.as_ref()
        {
            self.config
                .reset_setting(*scope, SETTINGS_KEYS[*selected])?;
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
                }
                KeyCode::Char(character)
                    if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
                {
                    query.push(character);
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
            Overlay::ConfirmQuit
                if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('q') =>
            {
                self.request_quit();
            }
            Overlay::ConfirmTerminateAll if key.code == KeyCode::Char('t') => {
                for session in self.terminal_sessions.values_mut() {
                    session.begin_termination();
                }
                self.running = false;
            }
            Overlay::ConfirmTerminalClose {
                terminal_id,
                bottom,
            } if key.code == KeyCode::Char('t') => {
                let terminal_id = *terminal_id;
                let bottom = *bottom;
                if let Some(session) = self.terminal_sessions.get_mut(&terminal_id) {
                    session.begin_termination();
                }
                self.terminal_sessions.remove(&terminal_id);
                if bottom {
                    self.bottom_terminal = None;
                    self.bottom_terminal_visible = false;
                    self.focus_owner = FocusOwner::Workspace;
                } else {
                    self.workspace.close_focused();
                }
                self.overlay = None;
            }
            Overlay::ConfirmDocumentClose if key.code == KeyCode::Char('d') => {
                self.discard_focused_document();
                self.overlay = None;
                self.focus_owner = FocusOwner::Workspace;
            }
            Overlay::ConfirmDocumentClose if key.code == KeyCode::Char('s') => {
                self.save_focused()?;
                if self.overlay == Some(Overlay::ConfirmDocumentClose) {
                    self.close_focused_editor_view();
                    self.overlay = None;
                    self.focus_owner = FocusOwner::Workspace;
                }
            }
            Overlay::SaveConflict if key.code == KeyCode::Char('o') => {
                let document_id = self.focused_editor().map(|(id, _)| id);
                if let Some(document_id) = document_id {
                    self.documents.save(document_id, true)?;
                }
                self.overlay = None;
                self.focus_owner = FocusOwner::Workspace;
            }
            Overlay::Message { .. }
            | Overlay::ConfirmQuit
            | Overlay::ConfirmTerminateAll
            | Overlay::ConfirmTerminalClose { .. }
            | Overlay::ConfirmDocumentClose
            | Overlay::SaveConflict => {}
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        let (columns, rows) = crossterm::terminal::size()
            .map_err(|error| AppError::io("could not inspect terminal size", error))?;
        let geometry = render::geometry(
            self,
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
        );
        if self.forward_terminal_mouse(&geometry, mouse)? {
            return Ok(());
        }
        match mouse.kind {
            MouseEventKind::Down(_) => {
                let click_count = self.register_click(mouse.column, mouse.row);
                if let Some(area) = geometry.explorer
                    && explorer_divider_contains(area, mouse.column, mouse.row)
                {
                    self.workbench_drag = Some(WorkbenchDrag::Explorer);
                    return Ok(());
                }
                if let Some(area) = geometry.bottom_terminal
                    && bottom_divider_contains(area, mouse.column, mouse.row)
                {
                    self.workbench_drag = Some(WorkbenchDrag::BottomTerminal);
                    return Ok(());
                }
                if let Some(splitter) = geometry
                    .pane_layout
                    .splitters
                    .iter()
                    .find(|splitter| splitter.rect.contains(mouse.column, mouse.row))
                {
                    self.drag_split = Some(*splitter);
                    return Ok(());
                }
                if let Some((terminal_id, area, focus_owner)) =
                    self.terminal_at(&geometry, mouse.column, mouse.row)
                    && self
                        .terminal_sessions
                        .get(&terminal_id)
                        .is_some_and(|session| !session.mouse_reporting())
                {
                    let rows = usize::from(area.height.saturating_sub(2));
                    if let Some(session) = self.terminal_sessions.get_mut(&terminal_id) {
                        session.start_selection(
                            usize::from(mouse.row.saturating_sub(area.y.saturating_add(1))),
                            usize::from(mouse.column.saturating_sub(area.x.saturating_add(1))),
                            rows,
                        );
                    }
                    self.terminal_selection_drag = Some(terminal_id);
                    self.focus_owner = focus_owner;
                    if focus_owner == FocusOwner::Workspace {
                        self.workspace
                            .focus_at(&geometry.pane_layout, mouse.column, mouse.row);
                    }
                    return Ok(());
                }
                if geometry
                    .explorer
                    .is_some_and(|area| area.contains(mouse.column, mouse.row))
                {
                    self.focus_owner = FocusOwner::Explorer;
                    let explorer_area = geometry.explorer.unwrap();
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                        && self.toggle_open_editors_at(mouse.row, explorer_area)
                    {
                        return Ok(());
                    }
                    if self.activate_open_editor_at(mouse.row, explorer_area) {
                        return Ok(());
                    }
                    if let Some(index) = self.explorer_entry_at(mouse.row, explorer_area) {
                        self.explorer.selected = index;
                        if click_count >= 2
                            && let Some(path) = self.explorer.toggle_selected()?
                        {
                            self.open_path(&path, None)?;
                            self.focus_owner = FocusOwner::Workspace;
                        }
                    }
                } else if geometry
                    .bottom_terminal
                    .is_some_and(|area| area.contains(mouse.column, mouse.row))
                {
                    self.focus_owner = FocusOwner::BottomTerminal;
                } else if self
                    .workspace
                    .focus_at(&geometry.pane_layout, mouse.column, mouse.row)
                {
                    self.focus_owner = FocusOwner::Workspace;
                    if self.start_scrollbar_drag(&geometry.pane_layout, mouse.column, mouse.row) {
                        return Ok(());
                    }
                    self.place_editor_cursor(&geometry.pane_layout, mouse.column, mouse.row, true);
                    if let Pane::Editor { document_id, view } = self.workspace.focused_pane_mut() {
                        let document = self.documents.get(*document_id);
                        match click_count {
                            2 => view.select_word_at(document, view.cursor),
                            3.. => view.select_line_at(document, view.cursor),
                            _ => {
                                view.selection_anchor = Some(view.cursor);
                                self.selection_drag = true;
                            }
                        }
                    }
                }
            }
            MouseEventKind::Drag(_) => {
                if let Some(drag) = self.workbench_drag {
                    match drag {
                        WorkbenchDrag::Explorer => {
                            self.config.effective.explorer.width = mouse
                                .column
                                .saturating_add(1)
                                .clamp(12, columns.saturating_sub(20).max(12));
                        }
                        WorkbenchDrag::BottomTerminal => {
                            self.config.effective.ui.bottom_terminal_height = geometry
                                .status
                                .y
                                .saturating_sub(mouse.row)
                                .clamp(3, rows.saturating_sub(5).max(3));
                        }
                    }
                    self.resize_terminals(columns, rows)?;
                } else if let Some(splitter) = self.drag_split {
                    let ratio = match splitter.direction {
                        SplitDirection::Vertical => {
                            f32::from(mouse.column.saturating_sub(splitter.container.x))
                                / f32::from(splitter.container.width.max(1))
                        }
                        SplitDirection::Horizontal => {
                            f32::from(mouse.row.saturating_sub(splitter.container.y))
                                / f32::from(splitter.container.height.max(1))
                        }
                    };
                    self.workspace.resize_split(splitter.id, ratio);
                    self.resize_terminals(columns, rows)?;
                } else if let Some(terminal_id) = self.terminal_selection_drag
                    && let Some((_, area, _)) = self.terminal_at(&geometry, mouse.column, mouse.row)
                    && let Some(session) = self.terminal_sessions.get_mut(&terminal_id)
                {
                    session.update_selection(
                        usize::from(mouse.row.saturating_sub(area.y.saturating_add(1))),
                        usize::from(mouse.column.saturating_sub(area.x.saturating_add(1))),
                    );
                } else if self.selection_drag {
                    self.place_editor_cursor(&geometry.pane_layout, mouse.column, mouse.row, false);
                } else if self.scrollbar_drag {
                    self.update_scrollbar_drag(&geometry.pane_layout, mouse.row);
                }
            }
            MouseEventKind::Up(_) => {
                self.drag_split = None;
                self.workbench_drag = None;
                self.terminal_selection_drag = None;
                if self.selection_drag
                    && let Pane::Editor { view, .. } = self.workspace.focused_pane_mut()
                    && view.selection().is_none()
                {
                    view.clear_selection();
                }
                self.selection_drag = false;
                self.scrollbar_drag = false;
            }
            MouseEventKind::ScrollUp => self.scroll_at(&geometry, mouse.column, mouse.row, -3),
            MouseEventKind::ScrollDown => self.scroll_at(&geometry, mouse.column, mouse.row, 3),
            _ => {}
        }
        Ok(())
    }

    fn terminal_at(
        &self,
        geometry: &render::WorkbenchGeometry,
        column: u16,
        row: u16,
    ) -> Option<(TerminalId, Rect, FocusOwner)> {
        if let (Some(terminal_id), Some(area)) = (self.bottom_terminal, geometry.bottom_terminal)
            && terminal_content_contains(area, column, row)
        {
            return Some((terminal_id, area, FocusOwner::BottomTerminal));
        }
        geometry
            .pane_layout
            .panes
            .iter()
            .find_map(|(pane_id, area)| {
                let Pane::Terminal { terminal_id } = self.workspace.panes.get(pane_id)? else {
                    return None;
                };
                terminal_content_contains(*area, column, row).then_some((
                    *terminal_id,
                    *area,
                    FocusOwner::Workspace,
                ))
            })
    }

    fn scroll_at(
        &mut self,
        geometry: &render::WorkbenchGeometry,
        column: u16,
        row: u16,
        delta: isize,
    ) {
        if geometry
            .explorer
            .is_some_and(|area| area.contains(column, row))
        {
            self.explorer.move_selection(delta);
            return;
        }
        if let Some((terminal_id, _, _)) = self.terminal_at(geometry, column, row) {
            if let Some(session) = self.terminal_sessions.get_mut(&terminal_id) {
                session.scroll(-delta);
            }
            return;
        }
        let pane_id = geometry
            .pane_layout
            .panes
            .iter()
            .find_map(|(pane_id, area)| area.contains(column, row).then_some(*pane_id));
        if let Some(Pane::Editor { view, .. }) =
            pane_id.and_then(|pane_id| self.workspace.panes.get_mut(&pane_id))
        {
            view.viewport_line = view.viewport_line.saturating_add_signed(delta);
        }
    }

    fn explorer_entry_at(&self, row: u16, area: Rect) -> Option<usize> {
        let open_editor_rows = if self.explorer.open_editors_expanded {
            u16::try_from(self.open_document_ids().len()).unwrap_or(u16::MAX)
        } else {
            0
        };
        let first_entry_row = area.y.saturating_add(3).saturating_add(open_editor_rows);
        let index = usize::from(row.checked_sub(first_entry_row)?);
        (index < self.explorer.entries().len()).then_some(index)
    }

    fn toggle_open_editors_at(&mut self, row: u16, area: Rect) -> bool {
        if row != area.y.saturating_add(1) {
            return false;
        }
        self.explorer.open_editors_expanded = !self.explorer.open_editors_expanded;
        true
    }

    fn activate_open_editor_at(&mut self, row: u16, area: Rect) -> bool {
        if !self.explorer.open_editors_expanded {
            return false;
        }
        let Some(index) = row.checked_sub(area.y.saturating_add(2)).map(usize::from) else {
            return false;
        };
        let Some(document_id) = self.open_document_ids().get(index).copied() else {
            return false;
        };
        if let Some(pane_id) = self.workspace.panes.iter().find_map(|(pane_id, pane)| {
            matches!(pane, Pane::Editor { document_id: open_id, .. } if *open_id == document_id)
                .then_some(*pane_id)
        }) {
            self.workspace.focused = pane_id;
            self.focus_owner = FocusOwner::Workspace;
            return true;
        }
        let pane_id = if matches!(
            self.workspace.focused_pane(),
            Pane::Empty | Pane::Editor { .. }
        ) {
            Some(self.workspace.focused)
        } else {
            self.workspace
                .panes
                .iter()
                .find_map(|(pane_id, pane)| matches!(pane, Pane::Editor { .. }).then_some(*pane_id))
        };
        let Some(pane_id) = pane_id else {
            return false;
        };
        self.workspace.panes.insert(
            pane_id,
            Pane::Editor {
                document_id,
                view: crate::editor::EditorView::default(),
            },
        );
        self.workspace.focused = pane_id;
        self.focus_owner = FocusOwner::Workspace;
        true
    }

    pub fn open_document_ids(&self) -> Vec<crate::document::DocumentId> {
        self.documents
            .iter()
            .map(|(document_id, _)| document_id)
            .collect()
    }

    pub fn focused_document_id(&self) -> Option<crate::document::DocumentId> {
        match self.workspace.focused_pane() {
            Pane::Editor { document_id, .. } => Some(*document_id),
            Pane::Empty | Pane::Terminal { .. } => None,
        }
    }

    fn forward_terminal_mouse(
        &mut self,
        geometry: &render::WorkbenchGeometry,
        mouse: MouseEvent,
    ) -> Result<bool> {
        let bottom = geometry.bottom_terminal.and_then(|area| {
            self.bottom_terminal
                .filter(|_| terminal_content_contains(area, mouse.column, mouse.row))
                .map(|terminal_id| (terminal_id, area, FocusOwner::BottomTerminal))
        });
        let workspace = geometry
            .pane_layout
            .panes
            .iter()
            .find_map(|(pane_id, area)| {
                let Pane::Terminal { terminal_id } = self.workspace.panes.get(pane_id)? else {
                    return None;
                };
                terminal_content_contains(*area, mouse.column, mouse.row).then_some((
                    *terminal_id,
                    *area,
                    FocusOwner::Workspace,
                ))
            });
        let Some((terminal_id, area, focus_owner)) = bottom.or(workspace) else {
            return Ok(false);
        };
        let Some(session) = self.terminal_sessions.get_mut(&terminal_id) else {
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
        self.focus_owner = focus_owner;
        if focus_owner == FocusOwner::Workspace {
            self.workspace
                .focus_at(&geometry.pane_layout, mouse.column, mouse.row);
        }
        Ok(true)
    }

    fn register_click(&mut self, column: u16, row: u16) -> u8 {
        let now = Instant::now();
        self.click_count = match self.last_click {
            Some((last, last_column, last_row))
                if now.duration_since(last) <= MULTI_CLICK_INTERVAL
                    && last_column == column
                    && last_row == row =>
            {
                self.click_count.saturating_add(1).min(3)
            }
            _ => 1,
        };
        self.last_click = Some((now, column, row));
        self.click_count
    }

    fn place_editor_cursor(
        &mut self,
        layout: &crate::layout::CalculatedLayout,
        x: u16,
        y: u16,
        clear_selection: bool,
    ) {
        let focused = self.workspace.focused;
        let Some(area) = layout.panes.get(&focused) else {
            return;
        };
        let Pane::Editor { document_id, view } = self.workspace.focused_pane_mut() else {
            return;
        };
        let document = self.documents.get(*document_id);
        let row = usize::from(y.saturating_sub(area.y.saturating_add(1)));
        let line_number_width = if self.config.effective.editor.line_numbers {
            document.len_lines().to_string().len().max(2) + 1
        } else {
            0
        };
        let column = usize::from(x.saturating_sub(area.x.saturating_add(1)))
            .saturating_sub(line_number_width);
        view.cursor = view.position_from_viewport(document, row, column);
        if clear_selection {
            view.clear_selection();
        }
    }

    fn start_scrollbar_drag(
        &mut self,
        layout: &crate::layout::CalculatedLayout,
        x: u16,
        y: u16,
    ) -> bool {
        if !self.config.effective.editor.scrollbar {
            return false;
        }
        let Some(area) = layout.panes.get(&self.workspace.focused) else {
            return false;
        };
        if x != area.x.saturating_add(area.width.saturating_sub(2)) {
            return false;
        }
        self.scrollbar_drag = true;
        self.update_scrollbar_drag(layout, y);
        true
    }

    fn update_scrollbar_drag(&mut self, layout: &crate::layout::CalculatedLayout, y: u16) {
        let Some(area) = layout.panes.get(&self.workspace.focused) else {
            return;
        };
        let Pane::Editor { document_id, view } = self.workspace.focused_pane_mut() else {
            return;
        };
        let line_count = self.documents.get(*document_id).len_lines();
        let track = usize::from(area.height.saturating_sub(2).max(1));
        let offset = usize::from(y.saturating_sub(area.y.saturating_add(1))).min(track - 1);
        view.viewport_line = offset.saturating_mul(line_count.saturating_sub(1)) / track.max(1);
    }

    pub fn apply(&mut self, command: Command) -> Result<()> {
        let creates_document = match &command {
            Command::Insert(text) | Command::Paste(text) => !text.is_empty(),
            Command::PasteClipboard => !self.clipboard.is_empty(),
            Command::Newline | Command::Indent => true,
            _ => false,
        };
        if creates_document && self.focus_owner == FocusOwner::Workspace {
            self.ensure_focused_editor();
        }
        match command {
            Command::Quit => {
                if self
                    .documents
                    .iter()
                    .any(|(_, document)| document.is_modified())
                {
                    self.overlay = Some(Overlay::ConfirmQuit);
                    self.focus_owner = FocusOwner::Overlay;
                } else {
                    self.request_quit();
                }
            }
            Command::Save => self.save_focused()?,
            Command::SaveAs(path) => {
                if let Some((document_id, _)) = self.focused_editor() {
                    self.documents.save_as(document_id, &path)?;
                }
            }
            Command::OpenDialog => {
                self.overlay = Some(file_dialog(&self.explorer.root, false));
                self.focus_owner = FocusOwner::Overlay;
            }
            Command::Open(path) => self.open_path(&path, None)?,
            Command::OpenInSplit(path, direction) => self.open_path(&path, Some(direction))?,
            Command::ClosePane => self.request_close()?,
            Command::Split(direction) => {
                if self.focus_owner == FocusOwner::Workspace
                    && !matches!(self.workspace.focused_pane(), Pane::Terminal { .. })
                {
                    self.workspace.split_contextual(direction);
                }
            }
            Command::ToggleExplorer => {
                self.explorer.visible = !self.explorer.visible;
                if self.explorer.visible {
                    self.explorer.refresh()?;
                    self.last_explorer_refresh = Instant::now();
                    self.focus_owner = FocusOwner::Explorer;
                } else {
                    self.focus_owner = FocusOwner::Workspace;
                }
            }
            Command::ToggleBottomTerminal => self.toggle_bottom_terminal()?,
            Command::OpenSettings => {
                self.overlay = Some(Overlay::Settings {
                    query: String::new(),
                    scope: ConfigScope::User,
                    selected: 0,
                });
                self.focus_owner = FocusOwner::Overlay;
            }
            Command::Copy => self.copy_selection(false),
            Command::Cut => self.copy_selection(true),
            Command::PasteClipboard => {
                let clipboard = self.clipboard.clone();
                self.insert_text(&clipboard)?;
            }
            Command::Paste(text) => self.insert_text(&text)?,
            Command::Insert(text) => self.insert_typed_text(&text)?,
            Command::Undo => self.edit_focused(|document, view| {
                if let Some(cursor) = document.undo() {
                    view.cursor = cursor;
                    view.clear_selection();
                }
            }),
            Command::Redo => self.edit_focused(|document, view| {
                if let Some(cursor) = document.redo() {
                    view.cursor = cursor;
                    view.clear_selection();
                }
            }),
            Command::Newline => self.edit_focused(|document, view| view.newline(document)),
            Command::DeleteBackward => {
                self.edit_focused(|document, view| view.delete_backward(document));
            }
            Command::DeleteForward => {
                self.edit_focused(|document, view| view.delete_forward(document));
            }
            Command::Indent => {
                let indent = if self.config.effective.editor.insert_spaces {
                    " ".repeat(usize::from(self.config.effective.editor.indent_width))
                } else {
                    "\t".to_owned()
                };
                self.insert_text(&indent)?;
            }
            Command::Unindent => {
                let width = usize::from(self.config.effective.editor.indent_width);
                self.edit_focused(|document, view| {
                    let line_start = document.line_to_char(document.char_to_line(view.cursor));
                    let prefix = document.slice(line_start, view.cursor);
                    let remove = prefix
                        .chars()
                        .rev()
                        .take(width)
                        .take_while(|character| *character == ' ' || *character == '\t')
                        .count();
                    if remove > 0 {
                        view.cursor = document.delete(view.cursor - remove, view.cursor);
                    }
                });
            }
            Command::MoveLeft { selecting } => {
                self.view_focused(|document, view| view.move_left(document, selecting));
            }
            Command::MoveRight { selecting } => {
                self.view_focused(|document, view| view.move_right(document, selecting));
            }
            Command::MoveUp { selecting } => {
                self.view_focused(|document, view| view.move_vertical(document, -1, selecting));
            }
            Command::MoveDown { selecting } => {
                self.view_focused(|document, view| view.move_vertical(document, 1, selecting));
            }
            Command::Home { selecting } => {
                self.view_focused(|document, view| view.move_home(document, selecting));
            }
            Command::End { selecting } => {
                self.view_focused(|document, view| view.move_end(document, selecting));
            }
            Command::PageUp { selecting } => {
                self.view_focused(|document, view| view.page(document, 10, false, selecting));
            }
            Command::PageDown { selecting } => {
                self.view_focused(|document, view| view.page(document, 10, true, selecting));
            }
            Command::Scroll(delta) => {
                if self.focus_owner == FocusOwner::BottomTerminal {
                    if let Some(session) = self
                        .bottom_terminal
                        .and_then(|id| self.terminal_sessions.get_mut(&id))
                    {
                        session.scroll(-isize::from(delta));
                    }
                } else {
                    match self.workspace.focused_pane_mut() {
                        Pane::Empty => {}
                        Pane::Editor { view, .. } => {
                            view.viewport_line =
                                view.viewport_line.saturating_add_signed(isize::from(delta));
                        }
                        Pane::Terminal { terminal_id } => {
                            if let Some(session) = self.terminal_sessions.get_mut(terminal_id) {
                                session.scroll(-isize::from(delta));
                            }
                        }
                    }
                }
            }
            Command::ResizeSplit { id, ratio } => {
                self.workspace.resize_split(id, ratio);
            }
            Command::MoveFocus(direction) => {
                let (columns, rows) = crossterm::terminal::size()
                    .map_err(|error| AppError::io("could not inspect terminal size", error))?;
                let layout = render::geometry(
                    self,
                    Rect {
                        x: 0,
                        y: 0,
                        width: columns,
                        height: rows,
                    },
                );
                self.workspace.move_focus(direction, &layout.pane_layout);
            }
            Command::NoOp => {}
        }
        Ok(())
    }

    fn focused_editor(&self) -> Option<(crate::document::DocumentId, crate::layout::PaneId)> {
        match self.workspace.focused_pane() {
            Pane::Editor { document_id, .. } => Some((*document_id, self.workspace.focused)),
            Pane::Empty | Pane::Terminal { .. } => None,
        }
    }

    fn ensure_focused_editor(&mut self) {
        if !matches!(self.workspace.focused_pane(), Pane::Empty) {
            return;
        }
        let document_id = self.documents.new_untitled();
        self.workspace.panes.insert(
            self.workspace.focused,
            Pane::Editor {
                document_id,
                view: crate::editor::EditorView::default(),
            },
        );
    }

    fn request_quit(&mut self) {
        let has_live_terminal = self
            .terminal_sessions
            .values_mut()
            .any(|session| session.is_alive().unwrap_or(true));
        if !has_live_terminal {
            self.running = false;
        } else {
            self.overlay = Some(Overlay::ConfirmTerminateAll);
            self.focus_owner = FocusOwner::Overlay;
        }
    }

    fn refresh_explorer_if_due(&mut self, now: Instant) -> bool {
        let interval = self.config.effective.explorer.refresh_interval_seconds;
        if !self.explorer.visible
            || interval == 0
            || now.duration_since(self.last_explorer_refresh) < Duration::from_secs(interval)
        {
            return false;
        }
        self.last_explorer_refresh = now;
        if let Err(error) = self.explorer.refresh() {
            self.message = Some(format!("Explorer refresh failed: {error}"));
        }
        true
    }

    fn request_close(&mut self) -> Result<()> {
        if self.focus_owner == FocusOwner::BottomTerminal {
            if let Some(terminal_id) = self.bottom_terminal {
                self.overlay = Some(Overlay::ConfirmTerminalClose {
                    terminal_id,
                    bottom: true,
                });
                self.focus_owner = FocusOwner::Overlay;
            }
            return Ok(());
        }
        match self.workspace.focused_pane() {
            Pane::Empty => {
                self.workspace.close_focused();
            }
            Pane::Terminal { terminal_id } => {
                let terminal_id = *terminal_id;
                let is_alive = self
                    .terminal_sessions
                    .get_mut(&terminal_id)
                    .is_some_and(|session| session.is_alive().unwrap_or(true));
                if is_alive {
                    self.overlay = Some(Overlay::ConfirmTerminalClose {
                        terminal_id,
                        bottom: false,
                    });
                    self.focus_owner = FocusOwner::Overlay;
                } else {
                    self.terminal_sessions.remove(&terminal_id);
                    self.workspace.close_focused();
                }
            }
            Pane::Editor { document_id, .. } => {
                let document_id = *document_id;
                let visible_views = self
                    .workspace
                    .panes
                    .values()
                    .filter(|pane| {
                        matches!(pane, Pane::Editor { document_id: id, .. } if *id == document_id)
                    })
                    .count();
                if visible_views == 1 && self.documents.get(document_id).is_modified() {
                    self.overlay = Some(Overlay::ConfirmDocumentClose);
                    self.focus_owner = FocusOwner::Overlay;
                } else {
                    self.close_focused_editor_view();
                }
            }
        }
        Ok(())
    }

    fn discard_focused_document(&mut self) {
        self.close_focused_editor_view();
    }

    fn close_focused_editor_view(&mut self) {
        let Some((document_id, pane_id)) = self.focused_editor() else {
            return;
        };
        if self.workspace.panes.len() == 1 {
            self.workspace.panes.insert(pane_id, Pane::Empty);
            self.documents.remove(document_id);
            if let Some((next_document, _)) = self.documents.iter().next() {
                self.workspace.panes.insert(
                    pane_id,
                    Pane::Editor {
                        document_id: next_document,
                        view: crate::editor::EditorView::default(),
                    },
                );
            }
        } else {
            self.workspace.close_focused();
            self.unload_document_without_views(document_id);
        }
    }

    fn unload_document_without_views(&mut self, document_id: crate::document::DocumentId) {
        let still_visible = self.workspace.panes.values().any(|pane| {
            matches!(pane, Pane::Editor { document_id: open_id, .. } if *open_id == document_id)
        });
        if !still_visible {
            self.documents.remove(document_id);
        }
    }

    fn unload_pristine_untitled_without_views(&mut self, document_id: crate::document::DocumentId) {
        let document = self.documents.get(document_id);
        if document.path.is_none() && !document.is_modified() {
            self.unload_document_without_views(document_id);
        }
    }

    fn edit_focused(
        &mut self,
        operation: impl FnOnce(&mut crate::document::Document, &mut crate::editor::EditorView),
    ) {
        let focused_pane = self.workspace.focused;
        let (document_id, change) = {
            let (documents, workspace) = (&mut self.documents, &mut self.workspace);
            let Pane::Editor { document_id, view } = workspace.focused_pane_mut() else {
                return;
            };
            let document_id = *document_id;
            let document = documents.get_mut(document_id);
            operation(document, view);
            (document_id, document.last_change())
        };
        if let Some(change) = change {
            for (pane_id, pane) in &mut self.workspace.panes {
                if *pane_id == focused_pane {
                    continue;
                }
                let Pane::Editor {
                    document_id: other_document,
                    view,
                } = pane
                else {
                    continue;
                };
                if *other_document == document_id {
                    view.cursor = transform_position(view.cursor, change);
                    view.selection_anchor = view
                        .selection_anchor
                        .map(|position| transform_position(position, change));
                }
            }
        }
    }

    fn view_focused(
        &mut self,
        operation: impl FnOnce(&crate::document::Document, &mut crate::editor::EditorView),
    ) {
        let (documents, workspace) = (&self.documents, &mut self.workspace);
        let Pane::Editor { document_id, view } = workspace.focused_pane_mut() else {
            return;
        };
        operation(documents.get(*document_id), view);
    }

    fn insert_text(&mut self, text: &str) -> Result<()> {
        if self.focus_owner == FocusOwner::BottomTerminal {
            if let Some(id) = self.bottom_terminal
                && let Some(session) = self.terminal_sessions.get_mut(&id)
            {
                session.write(text.as_bytes())?;
            }
            return Ok(());
        }
        if let Pane::Terminal { terminal_id } = self.workspace.focused_pane() {
            if let Some(session) = self.terminal_sessions.get_mut(terminal_id) {
                session.write(text.as_bytes())?;
            }
            return Ok(());
        }
        self.edit_focused(|document, view| view.insert(document, text));
        Ok(())
    }

    fn insert_typed_text(&mut self, text: &str) -> Result<()> {
        if self.focus_owner == FocusOwner::BottomTerminal
            || matches!(self.workspace.focused_pane(), Pane::Terminal { .. })
        {
            return self.insert_text(text);
        }
        self.edit_focused(|document, view| view.insert_typed(document, text));
        Ok(())
    }

    fn copy_selection(&mut self, cut: bool) {
        let terminal_selection = if self.focus_owner == FocusOwner::BottomTerminal {
            self.bottom_terminal
                .and_then(|id| self.terminal_sessions.get(&id))
                .and_then(TerminalSession::selected_text)
        } else if let Pane::Terminal { terminal_id } = self.workspace.focused_pane() {
            self.terminal_sessions
                .get(terminal_id)
                .and_then(TerminalSession::selected_text)
        } else {
            None
        };
        if let Some(selected) = terminal_selection {
            self.clipboard = selected;
            if self.config.effective.clipboard.osc52
                && let Some(control) = crate::clipboard::osc52_copy(&self.clipboard)
            {
                self.pending_terminal_control = control;
            }
            return;
        }
        let selected = {
            let (documents, workspace) = (&self.documents, &self.workspace);
            let Pane::Editor { document_id, view } = workspace.focused_pane() else {
                return;
            };
            view.selected_text(documents.get(*document_id))
        };
        if let Some(selected) = selected {
            self.clipboard = selected;
            if self.config.effective.clipboard.osc52
                && let Some(control) = crate::clipboard::osc52_copy(&self.clipboard)
            {
                self.pending_terminal_control = control;
            }
            if cut {
                self.edit_focused(|document, view| {
                    if let Some(range) = view.selection() {
                        view.cursor = range.start;
                        document.delete(range.start, range.end);
                        view.clear_selection();
                    }
                });
            }
        }
    }

    fn save_focused(&mut self) -> Result<()> {
        let Some((document_id, _)) = self.focused_editor() else {
            return Ok(());
        };
        if self.documents.get(document_id).path.is_none() {
            self.overlay = Some(file_dialog(&self.explorer.root, true));
            self.focus_owner = FocusOwner::Overlay;
            return Ok(());
        }
        if matches!(
            self.documents.save(document_id, false)?,
            SaveOutcome::ExternalChange
        ) {
            self.overlay = Some(Overlay::SaveConflict);
            self.focus_owner = FocusOwner::Overlay;
        }
        Ok(())
    }

    fn open_path(&mut self, path: &Path, split: Option<SplitDirection>) -> Result<()> {
        let (document_id, _) = self.documents.open(path)?;
        if let Some(direction) = split {
            self.workspace.split_with_editor(document_id, direction);
            return Ok(());
        }
        if let Some((pane_id, _)) = self.workspace.panes.iter().find(|(_, pane)| {
            matches!(pane, Pane::Editor { document_id: open_id, .. } if *open_id == document_id)
        }) {
            self.workspace.focused = *pane_id;
        } else if matches!(self.workspace.focused_pane(), Pane::Terminal { .. }) {
            if let Some(pane_id) = self.workspace.panes.iter().find_map(|(pane_id, pane)| {
                matches!(pane, Pane::Editor { .. }).then_some(*pane_id)
            }) {
                let replaced_document = match self.workspace.panes.get(&pane_id) {
                    Some(Pane::Editor { document_id, .. }) => Some(*document_id),
                    _ => None,
                };
                self.workspace.panes.insert(
                    pane_id,
                    Pane::Editor {
                        document_id,
                        view: crate::editor::EditorView::default(),
                    },
                );
                self.workspace.focused = pane_id;
                if let Some(replaced_document) = replaced_document {
                    self.unload_pristine_untitled_without_views(replaced_document);
                }
            } else {
                self.workspace
                    .split_with_editor(document_id, SplitDirection::Vertical);
            }
        } else {
            let focused = self.workspace.focused;
            let replaced_document = match self.workspace.panes.get(&focused) {
                Some(Pane::Editor { document_id, .. }) => Some(*document_id),
                _ => None,
            };
            self.workspace.panes.insert(
                focused,
                Pane::Editor { document_id, view: crate::editor::EditorView::default() },
            );
            if let Some(replaced_document) = replaced_document {
                self.unload_pristine_untitled_without_views(replaced_document);
            }
        }
        Ok(())
    }

    fn submit_file_dialog(&mut self) -> Result<()> {
        let Some(Overlay::FileDialog { input, save_as, .. }) = self.overlay.as_ref() else {
            return Ok(());
        };
        let path = PathBuf::from(input);
        let save_as = *save_as;
        if path.is_dir() {
            self.overlay = Some(file_dialog(&path, save_as));
            return Ok(());
        }
        let result = if save_as {
            let document_id = self.focused_editor().map(|(id, _)| id);
            document_id.map_or(Ok(()), |id| self.documents.save_as(id, &path))
        } else {
            self.open_path(&path, None)
        };
        match result {
            Ok(()) => {
                self.overlay = None;
                self.focus_owner = FocusOwner::Workspace;
            }
            Err(open_error) => {
                if let Some(Overlay::FileDialog { error, .. }) = self.overlay.as_mut() {
                    *error = Some(open_error.to_string());
                }
            }
        }
        Ok(())
    }

    fn activate_file_dialog_selection(&mut self) -> Result<()> {
        let selected_path = match self.overlay.as_ref() {
            Some(Overlay::FileDialog {
                entries, selected, ..
            }) => entries.get(*selected).cloned(),
            _ => None,
        };
        if let Some(path) = selected_path
            && let Some(Overlay::FileDialog { input, .. }) = self.overlay.as_mut()
        {
            *input = path.display().to_string();
        }
        self.submit_file_dialog()
    }

    fn toggle_bottom_terminal(&mut self) -> Result<()> {
        if self.bottom_terminal.is_none() {
            let id = TerminalId(0);
            self.spawn_terminal(id, None)?;
            self.bottom_terminal = Some(id);
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

    fn spawn_terminal(&mut self, id: TerminalId, directory: Option<&Path>) -> Result<()> {
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
        let session = TerminalSession::spawn(
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
        )?;
        self.terminal_sessions.insert(id, session);
        Ok(())
    }

    pub fn status_text(&self) -> String {
        if let Some(message) = &self.message {
            return message.clone();
        }
        match self.workspace.focused_pane() {
            Pane::Empty => "NO EDITOR  Type to create a new file".to_owned(),
            Pane::Editor { document_id, view } => {
                let document = self.documents.get(*document_id);
                let line = document.char_to_line(view.cursor) + 1;
                let column = crate::editor::display_column(document, line - 1, view.cursor) + 1;
                format!(
                    "EDIT  Ln {line}, Col {column}  UTF-8  {:?}",
                    document.line_ending
                )
            }
            Pane::Terminal { .. } => "TERMINAL  Ctrl+B leader".to_owned(),
        }
    }
}

fn terminal_content_contains(area: Rect, column: u16, row: u16) -> bool {
    column > area.x
        && column < area.x.saturating_add(area.width.saturating_sub(1))
        && row > area.y
        && row < area.y.saturating_add(area.height.saturating_sub(1))
}

fn explorer_divider_contains(area: Rect, column: u16, row: u16) -> bool {
    row >= area.y
        && row < area.y.saturating_add(area.height)
        && (column == area.x.saturating_add(area.width.saturating_sub(1))
            || column == area.x.saturating_add(area.width))
}

fn bottom_divider_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && (row == area.y || row.saturating_add(1) == area.y)
}

fn global_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Quit
            | Command::Save
            | Command::OpenDialog
            | Command::ClosePane
            | Command::Split(_)
            | Command::OpenInSplit(_, _)
            | Command::ToggleExplorer
            | Command::ToggleBottomTerminal
            | Command::OpenSettings
    )
}

fn terminal_mouse_code(kind: MouseEventKind) -> Option<(u16, bool)> {
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

fn terminal_mouse_sequence(sgr: bool, code: u16, column: u16, row: u16, release: bool) -> Vec<u8> {
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

fn file_dialog(directory: &Path, save_as: bool) -> Overlay {
    let mut entries = std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    entries.sort();
    let input = if save_as {
        format!("{}{}", directory.display(), std::path::MAIN_SEPARATOR)
    } else {
        directory.display().to_string()
    };
    Overlay::FileDialog {
        directory: directory.to_owned(),
        input,
        entries,
        selected: 0,
        save_as,
        error: None,
    }
}

pub(crate) const SETTINGS_KEYS: [&str; 30] = [
    "editor.indent_width",
    "editor.insert_spaces",
    "editor.default_line_ending",
    "editor.undo_limit_mib",
    "editor.line_numbers",
    "editor.scrollbar",
    "workspace.minimum_pane_width",
    "workspace.minimum_pane_height",
    "workspace.initial_split_ratio",
    "explorer.visible",
    "explorer.width",
    "explorer.exclusions",
    "explorer.refresh_interval_seconds",
    "mouse.enabled",
    "clipboard.osc52",
    "terminal.shell",
    "terminal.arguments",
    "terminal.scrollback_lines",
    "terminal.allow_child_osc52_write",
    "terminal.leader",
    "ui.bottom_terminal_visible",
    "ui.bottom_terminal_height",
    "keybindings.ctrl+q",
    "keybindings.ctrl+s",
    "keybindings.ctrl+o",
    "keybindings.ctrl+w",
    "keybindings.ctrl+z",
    "keybindings.ctrl+y",
    "keybindings.ctrl+g",
    "keybindings.ctrl+t",
];

fn move_settings_selection(query: &str, selected: &mut usize, direction: isize) {
    let normalized = query.to_lowercase();
    let matches = SETTINGS_KEYS
        .iter()
        .enumerate()
        .filter_map(|(index, key)| key.to_lowercase().contains(&normalized).then_some(index))
        .collect::<Vec<_>>();
    let Some(position) = matches.iter().position(|index| index == selected) else {
        if let Some(first) = matches.first() {
            *selected = *first;
        }
        return;
    };
    let target = position
        .saturating_add_signed(direction)
        .min(matches.len().saturating_sub(1));
    *selected = matches[target];
}

fn transform_position(position: usize, change: crate::document::TextChange) -> usize {
    if position < change.start {
        position
    } else if position >= change.start.saturating_add(change.removed_chars) {
        position
            .saturating_sub(change.removed_chars)
            .saturating_add(change.inserted_chars)
    } else {
        change.start.saturating_add(change.inserted_chars)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn semantic_commands_edit_and_undo_document() {
        let mut application = Application::new_for_test();
        application.apply(Command::Insert("界".to_owned())).unwrap();
        let (id, _) = application.focused_editor().unwrap();
        assert_eq!(application.documents.get(id).text(), "界");
        application.apply(Command::Undo).unwrap();
        assert_eq!(application.documents.get(id).text(), "");
    }

    #[test]
    fn directory_startup_has_no_untitled_document() {
        let directory = tempfile::tempdir().unwrap();
        let application = Application::new(Some(directory.path().to_owned())).unwrap();

        assert!(matches!(application.workspace.focused_pane(), Pane::Empty));
        assert!(application.open_document_ids().is_empty());
        assert_eq!(application.documents.iter().count(), 0);
    }

    #[test]
    fn typing_in_an_empty_editor_creates_one_untitled_document() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = Application::new(Some(directory.path().to_owned())).unwrap();
        application.overlay = None;
        application.focus_owner = FocusOwner::Workspace;

        application.apply(Command::Insert("x".to_owned())).unwrap();

        let document_id = application.focused_document_id().unwrap();
        assert_eq!(application.open_document_ids(), vec![document_id]);
        assert_eq!(application.documents.get(document_id).text(), "x");
        assert!(application.documents.get(document_id).path.is_none());
    }

    #[test]
    fn modified_document_blocks_first_quit() {
        let mut application = Application::new_for_test();
        application.apply(Command::Insert("x".to_owned())).unwrap();
        application.apply(Command::Quit).unwrap();
        assert_eq!(application.overlay, Some(Overlay::ConfirmQuit));
        assert!(application.running);
    }

    #[test]
    fn discarding_the_only_modified_view_removes_the_document() {
        let mut application = Application::new_for_test();
        application
            .apply(Command::Insert("unsaved".to_owned()))
            .unwrap();
        let discarded = application.focused_document_id().unwrap();
        application.discard_focused_document();
        assert_ne!(application.focused_document_id(), Some(discarded));
        assert!(application.documents.iter().all(|(id, _)| id != discarded));
        assert_eq!(application.workspace.panes.len(), 1);
    }

    #[test]
    fn opening_from_terminal_focus_preserves_the_terminal_pane() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opened.txt");
        fs::write(&path, "text").unwrap();
        let mut application = Application::new_for_test();
        let (terminal_pane, _) = application
            .workspace
            .split_with_terminal(SplitDirection::Vertical);
        application.open_path(&path, None).unwrap();
        assert!(matches!(
            application.workspace.panes[&terminal_pane],
            Pane::Terminal { .. }
        ));
        assert_eq!(
            application
                .documents
                .get(application.focused_document_id().unwrap())
                .text(),
            "text"
        );
    }

    #[test]
    fn opening_a_file_replaces_and_unloads_a_pristine_untitled_document() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opened.txt");
        fs::write(&path, "text").unwrap();
        let mut application = Application::new_for_test();

        application.open_path(&path, None).unwrap();

        assert_eq!(application.open_document_ids().len(), 1);
        let document_id = application.focused_document_id().unwrap();
        assert_eq!(
            application.documents.get(document_id).path.as_deref(),
            Some(fs::canonicalize(path).unwrap().as_path())
        );
    }

    #[test]
    fn bottom_terminal_does_not_create_terminal_splits() {
        let mut application = Application::new_for_test();
        application.focus_owner = FocusOwner::BottomTerminal;
        let pane_count = application.workspace.panes.len();
        application
            .apply(Command::Split(SplitDirection::Horizontal))
            .unwrap();
        assert_eq!(application.workspace.panes.len(), pane_count);
        assert_eq!(application.focus_owner, FocusOwner::BottomTerminal);
    }

    #[test]
    fn terminal_input_does_not_create_an_untitled_document() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = Application::new(Some(directory.path().to_owned())).unwrap();
        application.overlay = None;
        application.focus_owner = FocusOwner::BottomTerminal;

        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('w'),
                KeyModifiers::CONTROL,
            )))
            .unwrap();

        assert!(matches!(application.workspace.focused_pane(), Pane::Empty));
        assert!(application.open_document_ids().is_empty());
    }

    #[test]
    fn terminal_mouse_sequences_follow_the_requested_encoding() {
        assert_eq!(
            terminal_mouse_sequence(true, 0, 2, 3, false),
            b"\x1b[<0;2;3M"
        );
        assert_eq!(
            terminal_mouse_sequence(false, 0, 2, 3, false),
            vec![0x1b, b'[', b'M', 32, 34, 35]
        );
    }

    #[test]
    fn shared_editor_views_transform_positions_for_edits_and_undo() {
        let mut application = Application::new_for_test();
        let original = application.workspace.focused;
        application
            .apply(Command::Split(SplitDirection::Vertical))
            .unwrap();
        application.apply(Command::Insert("界".to_owned())).unwrap();
        let Pane::Editor { view, .. } = &application.workspace.panes[&original] else {
            panic!("expected editor pane");
        };
        assert_eq!(view.cursor, 1);
        application.apply(Command::Undo).unwrap();
        let Pane::Editor { view, .. } = &application.workspace.panes[&original] else {
            panic!("expected editor pane");
        };
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn editor_leader_chord_creates_an_editor_split() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('b'),
                KeyModifiers::CONTROL,
            )))
            .unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('v'),
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert_eq!(application.workspace.panes.len(), 2);
        assert!(application.workspace.panes.values().all(|pane| {
            matches!(pane, Pane::Editor { document_id: open_id, .. } if *open_id == document_id)
        }));
    }

    #[test]
    fn terminal_mouse_events_use_sgr_codes() {
        assert_eq!(
            terminal_mouse_code(MouseEventKind::Down(MouseButton::Left)),
            Some((0, false))
        );
        assert_eq!(
            terminal_mouse_code(MouseEventKind::Drag(MouseButton::Right)),
            Some((34, false))
        );
        assert_eq!(
            terminal_mouse_code(MouseEventKind::ScrollDown),
            Some((65, false))
        );
        assert_eq!(
            terminal_mouse_code(MouseEventKind::Up(MouseButton::Left)),
            Some((3, true))
        );
    }

    #[test]
    fn explorer_rows_account_for_open_editors_and_headers() {
        let mut application = Application::new_for_test();
        fs::write(application.explorer.root.join("README.md"), "readme").unwrap();
        fs::create_dir(application.explorer.root.join("target")).unwrap();
        application.explorer.refresh().unwrap();
        let area = Rect {
            x: 0,
            y: 1,
            width: 24,
            height: 20,
        };
        let readme_index = application
            .explorer
            .entries()
            .iter()
            .position(|entry| entry.path.ends_with("README.md"))
            .unwrap();
        let readme_row =
            area.y + 3 + application.documents.iter().count() as u16 + readme_index as u16;
        assert_eq!(
            application.explorer_entry_at(readme_row, area),
            Some(readme_index)
        );
    }

    #[test]
    fn collapsing_open_editors_updates_explorer_row_hit_testing() {
        let mut application = Application::new_for_test();
        fs::write(application.explorer.root.join("README.md"), "readme").unwrap();
        application.explorer.refresh().unwrap();
        let area = Rect {
            x: 0,
            y: 1,
            width: 24,
            height: 20,
        };
        let readme_index = application
            .explorer
            .entries()
            .iter()
            .position(|entry| entry.path.ends_with("README.md"))
            .unwrap();

        assert!(application.toggle_open_editors_at(area.y + 1, area));
        assert!(!application.explorer.open_editors_expanded);
        assert!(!application.activate_open_editor_at(area.y + 2, area));
        assert_eq!(
            application.explorer_entry_at(area.y + 3 + readme_index as u16, area),
            Some(readme_index)
        );
        assert!(!application.toggle_open_editors_at(area.y + 2, area));
    }

    #[test]
    fn open_editor_rows_switch_to_the_matching_document_pane() {
        let mut application = Application::new_for_test();
        let first_document = application.focused_document_id().unwrap();
        let second_document = application.documents.new_untitled();
        let area = Rect {
            x: 0,
            y: 1,
            width: 24,
            height: 20,
        };
        assert!(application.activate_open_editor_at(area.y + 2, area));
        assert_eq!(application.focused_document_id(), Some(first_document));
        assert_eq!(application.focus_owner, FocusOwner::Workspace);
        assert!(application.activate_open_editor_at(area.y + 3, area));
        assert_eq!(application.focused_document_id(), Some(second_document));
        assert!(application.activate_open_editor_at(area.y + 2, area));
        assert_eq!(application.focused_document_id(), Some(first_document));
    }

    #[test]
    fn control_w_closes_open_editors_one_at_a_time() {
        let mut application = Application::new_for_test();
        let first_document = application.focused_document_id().unwrap();
        let second_document = application.documents.new_untitled();
        let area = Rect {
            x: 0,
            y: 1,
            width: 24,
            height: 20,
        };
        assert!(application.activate_open_editor_at(area.y + 3, area));
        assert_eq!(application.focused_document_id(), Some(second_document));

        for expected_count in [1, 0] {
            application
                .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Char('w'),
                    KeyModifiers::CONTROL,
                )))
                .unwrap();
            assert_eq!(application.open_document_ids().len(), expected_count);
        }

        assert_ne!(application.focused_document_id(), Some(first_document));
        assert!(matches!(application.workspace.focused_pane(), Pane::Empty));
    }

    #[test]
    fn global_shortcuts_remain_available_while_explorer_is_focused() {
        let mut application = Application::new_for_test();
        application.focus_owner = FocusOwner::Explorer;
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('e'),
                KeyModifiers::CONTROL,
            )))
            .unwrap();
        assert!(!application.explorer.visible);
        assert_eq!(application.focus_owner, FocusOwner::Workspace);
    }

    #[test]
    fn explorer_auto_refresh_detects_new_files_when_due() {
        let mut application = Application::new_for_test();
        application
            .config
            .effective
            .explorer
            .refresh_interval_seconds = 5;
        let path = application.explorer.root.join("new.txt");
        fs::write(&path, "new").unwrap();
        let now = Instant::now();
        application.last_explorer_refresh = now - Duration::from_secs(5);

        assert!(application.refresh_explorer_if_due(now));
        assert!(
            application
                .explorer
                .entries()
                .iter()
                .any(|entry| entry.path == path)
        );
    }

    #[test]
    fn zero_explorer_refresh_interval_disables_polling() {
        let mut application = Application::new_for_test();
        application
            .config
            .effective
            .explorer
            .refresh_interval_seconds = 0;
        let path = application.explorer.root.join("new.txt");
        fs::write(&path, "new").unwrap();
        let now = Instant::now();
        application.last_explorer_refresh = now - Duration::from_secs(5);

        assert!(!application.refresh_explorer_if_due(now));
        assert!(
            application
                .explorer
                .entries()
                .iter()
                .all(|entry| entry.path != path)
        );
    }

    #[test]
    fn workbench_dividers_include_their_visible_borders() {
        let area = Rect {
            x: 0,
            y: 2,
            width: 24,
            height: 10,
        };
        assert!(explorer_divider_contains(area, 23, 5));
        assert!(explorer_divider_contains(area, 24, 5));
        assert!(bottom_divider_contains(area, 5, 2));
        assert!(bottom_divider_contains(area, 5, 1));
    }

    #[test]
    fn file_dialog_navigates_into_folders_and_opens_selected_files() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let file = nested.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let canonical_file = fs::canonicalize(&file).unwrap();
        let mut application = Application::new_for_test();
        application.overlay = Some(file_dialog(directory.path(), false));
        application.activate_file_dialog_selection().unwrap();
        let Some(Overlay::FileDialog {
            directory: current,
            entries,
            ..
        }) = application.overlay.as_ref()
        else {
            panic!("expected file dialog");
        };
        assert_eq!(current, &nested);
        assert_eq!(entries, &vec![file.clone()]);
        application.activate_file_dialog_selection().unwrap();
        assert!(application.overlay.is_none());
        let document_id = application.focused_document_id().unwrap();
        assert_eq!(
            application.documents.get(document_id).path.as_deref(),
            Some(canonical_file.as_path())
        );
    }

    #[test]
    fn file_dialog_mouse_wheel_moves_the_selection() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..8 {
            fs::write(directory.path().join(format!("{index}.txt")), "x").unwrap();
        }
        let mut application = Application::new_for_test();
        application.overlay = Some(file_dialog(directory.path(), false));
        application
            .handle_overlay_event(Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }))
            .unwrap();
        let Some(Overlay::FileDialog { selected, .. }) = application.overlay else {
            panic!("expected file dialog");
        };
        assert_eq!(selected, 3);
    }

    #[test]
    fn save_as_enter_submits_the_typed_filename() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("saved.txt");
        let mut application = Application::new_for_test();
        application
            .apply(Command::Insert("saved content".to_owned()))
            .unwrap();
        application.overlay = Some(file_dialog(directory.path(), true));
        application.focus_owner = FocusOwner::Overlay;
        let Some(Overlay::FileDialog { input, .. }) = application.overlay.as_mut() else {
            panic!("expected Save As dialog");
        };
        assert!(input.ends_with(std::path::MAIN_SEPARATOR));
        input.push_str("saved.txt");
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert!(application.overlay.is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved content");
        assert_eq!(application.open_document_ids().len(), 1);
        assert!(
            application
                .documents
                .iter()
                .all(|(_, document)| document.path.is_some())
        );
        assert_eq!(
            application
                .documents
                .get(application.focused_document_id().unwrap())
                .path
                .as_deref(),
            Some(fs::canonicalize(path).unwrap().as_path())
        );
    }

    #[test]
    fn control_w_closes_the_final_saved_editor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("saved.txt");
        let mut application = Application::new_for_test();
        application
            .apply(Command::Insert("saved".to_owned()))
            .unwrap();
        let document_id = application.focused_document_id().unwrap();
        application.documents.save_as(document_id, &path).unwrap();

        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('w'),
                KeyModifiers::CONTROL,
            )))
            .unwrap();

        assert!(matches!(application.workspace.focused_pane(), Pane::Empty));
        assert!(application.open_document_ids().is_empty());
        assert_eq!(application.documents.iter().count(), 0);
    }
}
