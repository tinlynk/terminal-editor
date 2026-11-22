use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyModifiers, MouseButton, MouseEventKind};
use unicode_width::UnicodeWidthStr;

use crate::{
    AppError, Result,
    command::Command,
    config::{ConfigScope, LoadedConfig},
    document::DocumentId,
    explorer::{Explorer, ExplorerDeleteTarget, root_for_startup},
    file_io::{
        DocumentRegistry, ExternalFileState, ExternalFileVersion, RecoveryRecord,
        load_recovery_records, open_file_warning, recovery_directory, remove_recovery_record,
        write_recovery_record,
    },
    input::InputMapper,
    layout::{PaneId, Rect, SplitDirection},
    render,
    syntax::{SyntaxCache, SyntaxPalette, SyntaxSpan},
    terminal::{TerminalGuard, install_panic_restoration_hook},
    terminal_session::TerminalSession,
    theme::{Theme, ThemeRegistry},
    workspace::{Pane, Workspace},
};

mod config_runtime;
mod document_lifecycle;
mod events;
mod explorer_delete;
mod overlay_events;
mod terminal_runtime;

#[cfg(test)]
use crossterm::event::KeyCode;
#[cfg(test)]
use crossterm::event::MouseEvent;
#[cfg(test)]
use terminal_runtime::{terminal_mouse_code, terminal_mouse_sequence};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkbenchDrag {
    Explorer,
    BottomTerminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EditorVisibilityAnchor {
    pane_id: PaneId,
    document_id: DocumentId,
    revision: u64,
    cursor: usize,
    area: Rect,
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
        editing: Option<usize>,
        new_value: String,
    },
    ThemePicker {
        scope: ConfigScope,
        selected: usize,
    },
    Help {
        scroll: usize,
    },
    SearchReplace {
        document_id: crate::document::DocumentId,
        query: String,
        replacement: String,
        editing_replacement: bool,
        current_match: Option<std::ops::Range<usize>>,
    },
    ExplorerCreate {
        directory: PathBuf,
        name: String,
        entries: Vec<PathBuf>,
        selected: usize,
        create_directory: bool,
        error: Option<String>,
    },
    ConfirmExplorerDelete {
        target: ExplorerDeleteTarget,
        modified_documents: usize,
    },
    ExplorerDeleteError {
        message: String,
    },
    Message {
        title: String,
        message: String,
    },
    ConfirmQuit,
    ConfirmTerminateAll,
    ConfirmTerminalClose,
    ConfirmDocumentClose,
    ConfirmUnsafeOpen {
        path: PathBuf,
        split: Option<SplitDirection>,
        warning: &'static str,
    },
    SaveConflict,
    Recovery {
        path: PathBuf,
        record: RecoveryRecord,
    },
    ExternalFileChange {
        document_id: DocumentId,
        missing: bool,
        version: ExternalFileVersion,
    },
    ConfirmSaveAsOverwrite {
        document_id: crate::document::DocumentId,
        path: PathBuf,
    },
}

#[derive(Debug)]
pub struct Application {
    pub config: LoadedConfig,
    pub documents: DocumentRegistry,
    pub workspace: Workspace,
    pub explorer: Explorer,
    pub bottom_terminals: Vec<TerminalSession>,
    active_bottom_terminal: usize,
    pub bottom_terminal_visible: bool,
    pub focus_owner: FocusOwner,
    pub overlay: Option<Overlay>,
    pub theme_registry: ThemeRegistry,
    pub theme: Theme,
    pub clipboard: String,
    pub message: Option<String>,
    saved_editor_views: BTreeMap<crate::document::DocumentId, crate::editor::EditorView>,
    pending_terminal_control: Vec<u8>,
    pending_close_document: Option<crate::document::DocumentId>,
    quit_in_progress: bool,
    pending_mouse_capture: Option<bool>,
    running: bool,
    input: InputMapper,
    drag_split: Option<crate::layout::SplitterRect>,
    workbench_drag: Option<WorkbenchDrag>,
    horizontal_scrollbar_drag_offset: Option<u16>,
    selection_drag: bool,
    scrollbar_drag: bool,
    terminal_selection_drag: bool,
    title_control_hover: Option<render::TitleControlAction>,
    last_explorer_refresh: Instant,
    last_click: Option<(Instant, u16, u16)>,
    click_count: u8,
    syntax_caches: RefCell<BTreeMap<crate::document::DocumentId, SyntaxCache>>,
    pending_startup_warning: Option<(PathBuf, &'static str)>,
    recovery_directory: PathBuf,
    recovery_records: VecDeque<(PathBuf, RecoveryRecord)>,
    deferred_external_changes: BTreeMap<DocumentId, ExternalFileVersion>,
    last_autosave: Instant,
    last_external_file_check: Instant,
    external_file_poll_cursor: usize,
}

fn terminal_paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + if bracketed { 12 } else { 0 });
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    bytes.extend_from_slice(text.as_bytes());
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    bytes
}

fn startup_workspace(
    startup_path: Option<&Path>,
    documents: &mut DocumentRegistry,
    initial_split_ratio: f32,
) -> Result<(Workspace, Option<(PathBuf, &'static str)>)> {
    let workspace = match startup_path {
        Some(path) if path.is_file() => {
            if let Some(warning) = open_file_warning(path)? {
                return Ok((
                    Workspace::empty(initial_split_ratio),
                    Some((path.to_owned(), warning.description())),
                ));
            }
            Workspace::with_editor(documents.open(path)?.0, initial_split_ratio)
        }
        Some(path) if !path.exists() => {
            Workspace::with_editor(documents.new_at_path(path.to_owned())?, initial_split_ratio)
        }
        _ => Workspace::empty(initial_split_ratio),
    };
    Ok((workspace, None))
}

impl Application {
    pub(crate) fn active_bottom_terminal(&self) -> Option<&TerminalSession> {
        self.bottom_terminals.get(self.active_bottom_terminal)
    }

    pub(crate) fn active_bottom_terminal_mut(&mut self) -> Option<&mut TerminalSession> {
        self.bottom_terminals.get_mut(self.active_bottom_terminal)
    }

    pub(crate) fn active_bottom_terminal_index(&self) -> usize {
        self.active_bottom_terminal
    }

    pub(crate) fn select_bottom_terminal(&mut self, index: usize) {
        if index < self.bottom_terminals.len() {
            self.active_bottom_terminal = index;
            self.bottom_terminal_visible = true;
            self.focus_owner = FocusOwner::BottomTerminal;
        }
    }

    pub fn new(startup_path: Option<PathBuf>) -> Result<Self> {
        let current_directory = env::current_dir()
            .map_err(|error| AppError::io("could not determine current directory", error))?;
        let explorer_root = root_for_startup(startup_path.as_deref(), &current_directory);
        let config = LoadedConfig::load(&explorer_root)?;
        let recovery_directory = recovery_directory(&config.user_path, &explorer_root)?;
        let recovery_records = load_recovery_records(&recovery_directory).unwrap_or_default();
        let theme_registry = ThemeRegistry::load_for_config(&config.user_path)?;
        let (theme, theme_message) = theme_registry.resolve(&config.effective.theme.active);
        let theme = theme.clone();
        let history_limit = config
            .effective
            .editor
            .undo_limit_mib
            .saturating_mul(1024 * 1024);
        let mut documents =
            DocumentRegistry::new(config.effective.editor.default_line_ending, history_limit);
        let (workspace, pending_startup_warning) = startup_workspace(
            startup_path.as_deref(),
            &mut documents,
            config.effective.workspace.initial_split_ratio,
        )?;
        let explorer = Explorer::new(
            explorer_root,
            config.effective.explorer.visible,
            &config.effective.explorer.exclusions,
        )?;
        let overlay = startup_path
            .as_deref()
            .filter(|path| path.is_dir())
            .map(|path| file_dialog(path, false, &explorer));
        let input = InputMapper::new(
            config.effective.keybindings.clone(),
            &config.effective.terminal.leader,
        );
        let mut application = Self {
            bottom_terminal_visible: config.effective.ui.bottom_terminal_visible,
            config,
            documents,
            workspace,
            explorer,
            bottom_terminals: Vec::new(),
            active_bottom_terminal: 0,
            focus_owner: if overlay.is_some() {
                FocusOwner::Overlay
            } else {
                FocusOwner::Workspace
            },
            overlay,
            theme_registry,
            theme,
            clipboard: String::new(),
            message: theme_message,
            saved_editor_views: BTreeMap::new(),
            pending_terminal_control: Vec::new(),
            pending_close_document: None,
            quit_in_progress: false,
            pending_mouse_capture: None,
            running: true,
            input,
            drag_split: None,
            workbench_drag: None,
            horizontal_scrollbar_drag_offset: None,
            selection_drag: false,
            scrollbar_drag: false,
            terminal_selection_drag: false,
            title_control_hover: None,
            last_explorer_refresh: Instant::now(),
            last_click: None,
            click_count: 0,
            syntax_caches: RefCell::new(BTreeMap::new()),
            pending_startup_warning,
            recovery_directory,
            recovery_records: recovery_records.into(),
            deferred_external_changes: BTreeMap::new(),
            last_autosave: Instant::now(),
            last_external_file_check: Instant::now(),
            external_file_poll_cursor: 0,
        };
        if !application.recovery_records.is_empty() {
            application.overlay = None;
        }
        application.show_next_recovery();
        if application.bottom_terminal_visible {
            application
                .bottom_terminals
                .push(application.spawn_terminal(None)?);
        }
        Ok(application)
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
        let theme_registry = ThemeRegistry::load(directory.join("themes")).unwrap();
        let theme = theme_registry.get("default").unwrap().theme.clone();
        let mut documents = DocumentRegistry::new(crate::config::LineEnding::Lf, 1024);
        let document_id = documents.new_untitled();
        Self {
            workspace: Workspace::with_editor(document_id, 0.5),
            explorer: Explorer::new(directory.clone(), true, &[".git".to_owned()]).unwrap(),
            config,
            documents,
            bottom_terminals: Vec::new(),
            active_bottom_terminal: 0,
            bottom_terminal_visible: false,
            focus_owner: FocusOwner::Workspace,
            overlay: None,
            theme_registry,
            theme,
            clipboard: String::new(),
            message: None,
            saved_editor_views: BTreeMap::new(),
            pending_terminal_control: Vec::new(),
            pending_close_document: None,
            quit_in_progress: false,
            pending_mouse_capture: None,
            running: true,
            input: InputMapper::default(),
            drag_split: None,
            workbench_drag: None,
            horizontal_scrollbar_drag_offset: None,
            selection_drag: false,
            scrollbar_drag: false,
            terminal_selection_drag: false,
            title_control_hover: None,
            last_explorer_refresh: Instant::now(),
            last_click: None,
            click_count: 0,
            syntax_caches: RefCell::new(BTreeMap::new()),
            pending_startup_warning: None,
            recovery_directory: directory.join("recovery"),
            recovery_records: VecDeque::new(),
            deferred_external_changes: BTreeMap::new(),
            last_autosave: Instant::now(),
            last_external_file_check: Instant::now(),
            external_file_poll_cursor: 0,
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
                let (columns, rows) = match &event {
                    Event::Resize(columns, rows) => (*columns, *rows),
                    _ => crossterm::terminal::size()
                        .map_err(|error| AppError::io("could not inspect terminal size", error))?,
                };
                self.handle_event_and_sync_editor_viewport(event, columns, rows)?;
                redraw = true;
            }
            redraw |= self.refresh_explorer_if_due(Instant::now());
            redraw |= self.refresh_document_lifecycle(Instant::now());
            redraw |= self.input.expire_leader(Instant::now());
            if !self.pending_terminal_control.is_empty() {
                let control = std::mem::take(&mut self.pending_terminal_control);
                terminal.write_control(&control)?;
            }
            if let Some(enabled) = self.pending_mouse_capture.take() {
                terminal.set_mouse_capture(enabled)?;
            }
            for session in &mut self.bottom_terminals {
                redraw |= session.drain_output()?;
                if let Some(clipboard) = session.take_child_clipboard_write() {
                    self.clipboard = clipboard;
                }
            }
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
        self.title_control_hover = geometry.title_controls.action_at(mouse.column, mouse.row);
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(area) = geometry.terminal
            && let Some(index) = render::terminal_tab_at(self, area, mouse.column, mouse.row)
        {
            self.select_bottom_terminal(index);
            return Ok(());
        }
        if self.forward_terminal_mouse(&geometry, mouse)? {
            return Ok(());
        }
        if self.route_mouse_scroll(&geometry, mouse) {
            return Ok(());
        }
        match mouse.kind {
            MouseEventKind::Down(_) => {
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                    && let Some(action) = geometry.title_controls.action_at(mouse.column, mouse.row)
                {
                    self.apply(title_control_command(action))?;
                    return Ok(());
                }
                let click_count = self.register_click(mouse.column, mouse.row);
                if let Some(area) = geometry.explorer
                    && explorer_divider_contains(
                        area,
                        geometry.explorer_divider,
                        mouse.column,
                        mouse.row,
                    )
                {
                    self.workbench_drag = Some(WorkbenchDrag::Explorer);
                    return Ok(());
                }
                if let Some(area) = geometry.terminal
                    && bottom_divider_contains(
                        area,
                        geometry.terminal_divider,
                        mouse.column,
                        mouse.row,
                    )
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
                if let Some(area) = self.terminal_at(&geometry, mouse.column, mouse.row)
                    && self
                        .active_bottom_terminal()
                        .is_some_and(|session| !session.mouse_reporting())
                {
                    let rows = usize::from(area.height.saturating_sub(2));
                    if let Some(session) = self.active_bottom_terminal_mut() {
                        session.start_selection(
                            usize::from(mouse.row.saturating_sub(area.y.saturating_add(1))),
                            usize::from(mouse.column.saturating_sub(area.x.saturating_add(1))),
                            rows,
                        );
                    }
                    self.terminal_selection_drag = true;
                    self.focus_owner = FocusOwner::BottomTerminal;
                    return Ok(());
                }
                if geometry
                    .explorer
                    .is_some_and(|area| area.contains(mouse.column, mouse.row))
                {
                    self.focus_owner = FocusOwner::Explorer;
                    let explorer_area = geometry.explorer.unwrap();
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                        && let Some(action) = render::explorer_root_action_at(
                            explorer_area,
                            self.open_document_ids().len(),
                            self.explorer.open_editors_expanded,
                            mouse.column,
                            mouse.row,
                        )
                    {
                        match action {
                            render::ExplorerRootAction::NewFile => self.open_explorer_create(false),
                            render::ExplorerRootAction::NewFolder => {
                                self.open_explorer_create(true)
                            }
                            render::ExplorerRootAction::Refresh => {
                                self.explorer.refresh()?;
                                self.last_explorer_refresh = Instant::now();
                            }
                        }
                        return Ok(());
                    }
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                        && self.toggle_open_editors_at(mouse.row, explorer_area)
                    {
                        self.explorer.open_editors_header_selected = true;
                        self.explorer.root_header_selected = false;
                        return Ok(());
                    }
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                        && self.toggle_workspace_root_at(mouse.row, explorer_area)
                    {
                        self.explorer.open_editors_header_selected = false;
                        self.explorer.root_header_selected = true;
                        return Ok(());
                    }
                    if self.activate_open_editor_at(mouse.row, explorer_area) {
                        return Ok(());
                    }
                    if let Some(index) = self.explorer_entry_at(mouse.row, explorer_area) {
                        self.explorer.open_editors_header_selected = false;
                        self.explorer.root_header_selected = false;
                        self.explorer.selected = index;
                        let entry = self.explorer.entries().get(index).cloned();
                        if let Some(entry) = entry {
                            if entry.is_directory && !entry.is_symlink {
                                let icon = if entry.expanded {
                                    self.theme.icons.folder_open.as_str()
                                } else {
                                    self.theme.icons.folder_closed.as_str()
                                };
                                let icon_col = explorer_area.x
                                    + 1
                                    + u16::try_from((entry.depth + 1) * 2).unwrap_or(0);
                                let icon_width =
                                    u16::try_from(UnicodeWidthStr::width(icon)).unwrap_or(1);
                                let icon_end = icon_col.saturating_add(icon_width);
                                let is_icon_click = mouse.column <= icon_end;
                                if is_icon_click || click_count >= 2 {
                                    self.explorer.toggle_selected()?;
                                }
                            } else if click_count >= 2
                                && let Some(path) = self.explorer.toggle_selected()?
                            {
                                self.open_path(&path, None)?;
                                self.focus_owner = FocusOwner::Workspace;
                            }
                        }
                    }
                } else if geometry
                    .terminal
                    .is_some_and(|area| area.contains(mouse.column, mouse.row))
                {
                    self.focus_owner = FocusOwner::BottomTerminal;
                } else if self
                    .workspace
                    .focus_at(&geometry.pane_layout, mouse.column, mouse.row)
                {
                    self.focus_owner = FocusOwner::Workspace;
                    let focused_id = self.workspace.focused;
                    if let Some(pane_area) = geometry.pane_layout.panes.get(&focused_id)
                        && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                        && let Some(selected_doc_id) =
                            render::editor_tab_at(self, *pane_area, mouse.column, mouse.row)
                    {
                        self.switch_editor_document(focused_id, selected_doc_id);
                        return Ok(());
                    }
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                        && self.start_horizontal_scrollbar_drag(
                            &geometry.pane_layout,
                            mouse.column,
                            mouse.row,
                        )
                    {
                        return Ok(());
                    }
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
                } else if self.terminal_selection_drag
                    && let Some(area) = self.terminal_at(&geometry, mouse.column, mouse.row)
                    && let Some(session) = self.active_bottom_terminal_mut()
                {
                    session.update_selection(
                        usize::from(mouse.row.saturating_sub(area.y.saturating_add(1))),
                        usize::from(mouse.column.saturating_sub(area.x.saturating_add(1))),
                    );
                } else if self.selection_drag {
                    self.place_editor_cursor(&geometry.pane_layout, mouse.column, mouse.row, false);
                } else if self.horizontal_scrollbar_drag_offset.is_some() {
                    self.update_horizontal_scrollbar_drag(&geometry.pane_layout, mouse.column);
                } else if self.scrollbar_drag {
                    self.update_scrollbar_drag(&geometry.pane_layout, mouse.row);
                }
            }
            MouseEventKind::Up(_) => {
                self.drag_split = None;
                self.workbench_drag = None;
                self.terminal_selection_drag = false;
                if self.selection_drag
                    && let Pane::Editor { view, .. } = self.workspace.focused_pane_mut()
                    && view.selection().is_none()
                {
                    view.clear_selection();
                }
                self.selection_drag = false;
                self.horizontal_scrollbar_drag_offset = None;
                self.scrollbar_drag = false;
            }
            _ => {}
        }
        Ok(())
    }

    fn route_mouse_scroll(
        &mut self,
        geometry: &render::WorkbenchGeometry,
        mouse: crossterm::event::MouseEvent,
    ) -> bool {
        let horizontal_delta = match mouse.kind {
            MouseEventKind::ScrollUp if mouse.modifiers.contains(KeyModifiers::SHIFT) => Some(-3),
            MouseEventKind::ScrollDown if mouse.modifiers.contains(KeyModifiers::SHIFT) => Some(3),
            MouseEventKind::ScrollLeft => Some(-3),
            MouseEventKind::ScrollRight => Some(3),
            _ => None,
        };
        if let Some(delta) = horizontal_delta {
            if geometry
                .explorer
                .is_some_and(|area| area.contains(mouse.column, mouse.row))
            {
                self.explorer.scroll_horizontal(delta);
                return true;
            }
            self.scroll_horizontal_at(geometry, mouse.column, mouse.row, delta);
            return true;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_at(geometry, mouse.column, mouse.row, -3);
                true
            }
            MouseEventKind::ScrollDown => {
                self.scroll_at(geometry, mouse.column, mouse.row, 3);
                true
            }
            _ => false,
        }
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
            self.explorer.scroll_vertical(delta);
            return;
        }
        if self.terminal_at(geometry, column, row).is_some() {
            if let Some(session) = self.active_bottom_terminal_mut() {
                session.scroll(-delta);
            }
            return;
        }
        let pane = geometry
            .pane_layout
            .panes
            .iter()
            .find_map(|(pane_id, area)| area.contains(column, row).then_some((*pane_id, *area)));
        let Some((pane_id, area)) = pane else {
            return;
        };
        let Some(Pane::Editor { document_id, .. }) = self.workspace.panes.get(&pane_id) else {
            return;
        };
        let document_id = *document_id;
        let document = self.documents.get(document_id);
        let content = render::editor_content_geometry(self, area, document);
        let Some(Pane::Editor { view, .. }) = self.workspace.panes.get_mut(&pane_id) else {
            return;
        };
        if self.config.effective.editor.soft_wrap {
            view.scroll_vertical_wrapped(document, content.width, content.height, delta);
        } else {
            view.scroll_vertical(document, content.height, delta);
        }
    }

    fn scroll_horizontal_at(
        &mut self,
        geometry: &render::WorkbenchGeometry,
        column: u16,
        row: u16,
        delta: isize,
    ) {
        if self.config.effective.editor.soft_wrap {
            return;
        }
        let Some((pane_id, area)) = geometry
            .pane_layout
            .panes
            .iter()
            .find_map(|(pane_id, area)| area.contains(column, row).then_some((*pane_id, *area)))
        else {
            return;
        };
        let Some(Pane::Editor { document_id, .. }) = self.workspace.panes.get(&pane_id) else {
            return;
        };
        let document_id = *document_id;
        let document = self.documents.get(document_id);
        let content = render::editor_content_geometry(self, area, document);
        let Some(Pane::Editor { view, .. }) = self.workspace.panes.get_mut(&pane_id) else {
            return;
        };
        view.scroll_horizontal(document, content.width, delta);
    }

    fn explorer_entry_at(&self, row: u16, area: Rect) -> Option<usize> {
        if !self.explorer.root_expanded {
            return None;
        }
        let rows = render::explorer_rows(
            area,
            self.open_document_ids().len(),
            self.explorer.open_editors_expanded,
        );
        let index = self
            .explorer
            .viewport_row
            .saturating_add(usize::from(row.checked_sub(rows.entries_start)?));
        (index < self.explorer.entries().len()).then_some(index)
    }

    fn toggle_open_editors_at(&mut self, row: u16, area: Rect) -> bool {
        let rows = render::explorer_rows(
            area,
            self.open_document_ids().len(),
            self.explorer.open_editors_expanded,
        );
        if row != rows.header {
            return false;
        }
        self.explorer.open_editors_expanded = !self.explorer.open_editors_expanded;
        true
    }

    fn toggle_workspace_root_at(&mut self, row: u16, area: Rect) -> bool {
        let rows = render::explorer_rows(
            area,
            self.open_document_ids().len(),
            self.explorer.open_editors_expanded,
        );
        if row != rows.root {
            return false;
        }
        self.explorer.root_expanded = !self.explorer.root_expanded;
        self.explorer.viewport_row = 0;
        true
    }

    fn activate_open_editor_at(&mut self, row: u16, area: Rect) -> bool {
        if !self.explorer.open_editors_expanded {
            return false;
        }
        let rows = render::explorer_rows(area, self.open_document_ids().len(), true);
        let Some(index) = row.checked_sub(rows.open_editors_start).map(usize::from) else {
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
        self.switch_editor_document(pane_id, document_id);
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
            Pane::Empty => None,
        }
    }

    pub fn syntax_spans(
        &self,
        document_id: crate::document::DocumentId,
        document: &crate::document::Document,
        line_index: usize,
    ) -> Vec<SyntaxSpan> {
        let editor = &self.config.effective.editor;
        let maximum_bytes = editor
            .syntax_highlighting_max_file_size_kib
            .saturating_mul(1024);
        if !editor.syntax_highlighting || document.len_bytes() > maximum_bytes {
            return Vec::new();
        }
        let palette = SyntaxPalette {
            foreground: self.theme.colors.foreground,
            comment: self.theme.colors.syntax_comment,
            string: self.theme.colors.syntax_string,
            number: self.theme.colors.syntax_number,
            keyword: self.theme.colors.syntax_keyword,
            type_name: self.theme.colors.syntax_type,
            function: self.theme.colors.syntax_function,
            variable: self.theme.colors.syntax_variable,
        };
        self.syntax_caches
            .borrow_mut()
            .entry(document_id)
            .or_default()
            .highlight_line(
                document.path.as_deref(),
                document.revision(),
                palette,
                line_index,
                |line| document.line(line),
            )
    }

    fn reload_document(&mut self, document_id: DocumentId) -> Result<()> {
        self.syntax_caches.borrow_mut().remove(&document_id);
        self.documents.reload(document_id, true)
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
        let Pane::Editor { document_id, .. } = self.workspace.focused_pane() else {
            return;
        };
        let document_id = *document_id;
        let document = self.documents.get(document_id);
        let content = render::editor_content_geometry(self, *area, document);
        let Pane::Editor { view, .. } = self.workspace.focused_pane_mut() else {
            return;
        };
        if self.config.effective.editor.soft_wrap {
            view.ensure_visible_wrapped(document, content.height, content.width);
        } else {
            view.clamp_viewport(document, content.height);
        }
        let row = usize::from(y.saturating_sub(area.y.saturating_add(1)));
        let column = usize::from(x.saturating_sub(area.x.saturating_add(1)))
            .saturating_sub(content.line_number_width);
        view.cursor = if self.config.effective.editor.soft_wrap {
            view.position_from_wrapped_viewport(document, row, column, content.width)
        } else {
            view.position_from_viewport(document, row, column)
        };
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
        let Pane::Editor { document_id, .. } = self.workspace.focused_pane() else {
            return false;
        };
        if !render::editor_content_geometry(self, *area, self.documents.get(*document_id))
            .scrollbar_visible
        {
            return false;
        }
        if x != area.x.saturating_add(area.width.saturating_sub(2)) {
            return false;
        }
        self.scrollbar_drag = true;
        self.update_scrollbar_drag(layout, y);
        true
    }

    fn start_horizontal_scrollbar_drag(
        &mut self,
        layout: &crate::layout::CalculatedLayout,
        x: u16,
        y: u16,
    ) -> bool {
        let Some(area) = layout.panes.get(&self.workspace.focused) else {
            return false;
        };
        let Pane::Editor { document_id, view } = self.workspace.focused_pane() else {
            return false;
        };
        let document = self.documents.get(*document_id);
        let content = render::editor_content_geometry(self, *area, document);
        let Some(scrollbar) =
            render::horizontal_scrollbar_geometry(*area, content, document, view.viewport_column)
        else {
            return false;
        };
        if y != scrollbar.row
            || x < scrollbar.track_x
            || x >= scrollbar.track_x.saturating_add(scrollbar.track_width)
        {
            return false;
        }
        let thumb_end = scrollbar.thumb_x.saturating_add(scrollbar.thumb_width);
        self.horizontal_scrollbar_drag_offset = Some(if x >= scrollbar.thumb_x && x < thumb_end {
            x.saturating_sub(scrollbar.thumb_x)
        } else {
            scrollbar.thumb_width / 2
        });
        self.update_horizontal_scrollbar_drag(layout, x);
        true
    }

    fn update_horizontal_scrollbar_drag(
        &mut self,
        layout: &crate::layout::CalculatedLayout,
        x: u16,
    ) {
        let Some(grab_offset) = self.horizontal_scrollbar_drag_offset else {
            return;
        };
        let Some(area) = layout.panes.get(&self.workspace.focused) else {
            return;
        };
        let Pane::Editor { document_id, view } = self.workspace.focused_pane() else {
            return;
        };
        let document_id = *document_id;
        let document = self.documents.get(document_id);
        let content = render::editor_content_geometry(self, *area, document);
        let Some(scrollbar) =
            render::horizontal_scrollbar_geometry(*area, content, document, view.viewport_column)
        else {
            return;
        };
        let travel = scrollbar.track_width.saturating_sub(scrollbar.thumb_width);
        let thumb_column = x
            .saturating_sub(scrollbar.track_x)
            .saturating_sub(grab_offset)
            .min(travel);
        let viewport_column = usize::from(thumb_column).saturating_mul(scrollbar.maximum)
            / usize::from(travel).max(1);
        let Pane::Editor { view, .. } = self.workspace.focused_pane_mut() else {
            return;
        };
        view.viewport_column = viewport_column;
    }

    fn update_scrollbar_drag(&mut self, layout: &crate::layout::CalculatedLayout, y: u16) {
        let Some(area) = layout.panes.get(&self.workspace.focused) else {
            return;
        };
        let Pane::Editor { document_id, .. } = self.workspace.focused_pane() else {
            return;
        };
        let document_id = *document_id;
        let document = self.documents.get(document_id);
        let content = render::editor_content_geometry(self, *area, document);
        let track = content.height.max(1);
        let offset = usize::from(y.saturating_sub(area.y.saturating_add(1))).min(track - 1);
        let Pane::Editor { view, .. } = self.workspace.focused_pane_mut() else {
            return;
        };
        if self.config.effective.editor.soft_wrap {
            let maximum = crate::editor::maximum_wrapped_viewport(document, content.width, track);
            if document.len_lines() == 1 {
                view.viewport_line = 0;
                view.viewport_wrap_segment =
                    offset.saturating_mul(maximum.segment) / track.saturating_sub(1).max(1);
            } else {
                view.viewport_line =
                    offset.saturating_mul(maximum.line) / track.saturating_sub(1).max(1);
                view.viewport_wrap_segment = 0;
            }
            view.viewport_column = 0;
        } else {
            let maximum = crate::editor::EditorView::maximum_viewport_line(document, track);
            view.viewport_line = offset.saturating_mul(maximum) / track.saturating_sub(1).max(1);
        }
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
                if self.config.effective.editor.check_modified_files_on_quit
                    && self
                        .documents
                        .iter()
                        .any(|(_, document)| document.is_modified())
                {
                    self.quit_in_progress = true;
                    self.continue_quit_sequence()?;
                } else {
                    self.quit_in_progress = false;
                    self.request_quit();
                }
            }
            Command::Save => self.save_focused()?,
            Command::SaveAs(path) => {
                if let Some((document_id, _)) = self.focused_editor() {
                    self.save_document_as(document_id, &path)?;
                }
            }
            Command::OpenDialog => {
                self.overlay = Some(file_dialog(&self.explorer.root, false, &self.explorer));
                self.focus_owner = FocusOwner::Overlay;
            }
            Command::Open(path) => self.open_path(&path, None)?,
            Command::OpenInSplit(path, direction) => self.open_path(&path, Some(direction))?,
            Command::ClosePane => self.request_close()?,
            Command::Split(direction) => {
                if self.focus_owner == FocusOwner::Workspace {
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
            Command::NewBottomTerminal => self.new_bottom_terminal()?,
            Command::PreviousBottomTerminal => self.cycle_bottom_terminal(-1),
            Command::NextBottomTerminal => self.cycle_bottom_terminal(1),
            Command::OpenSettings => {
                self.overlay = Some(Overlay::Settings {
                    query: String::new(),
                    scope: ConfigScope::User,
                    selected: 0,
                    editing: None,
                    new_value: String::new(),
                });
                self.focus_owner = FocusOwner::Overlay;
            }
            Command::OpenHelp => {
                self.overlay = Some(Overlay::Help { scroll: 0 });
                self.focus_owner = FocusOwner::Overlay;
            }
            Command::OpenSearchReplace => self.open_search_replace(),
            Command::Copy => self.copy_selection(false),
            Command::Cut => self.copy_selection(true),
            Command::PasteClipboard => {
                let clipboard = self.clipboard.clone();
                self.insert_text(&clipboard)?;
            }
            Command::Paste(text) => self.paste_text(&text)?,
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
                if self.config.effective.editor.soft_wrap {
                    let width = self
                        .focused_editor_content_geometry()?
                        .map_or(1, |value| value.width);
                    self.view_focused(|document, view| {
                        view.move_vertical_wrapped(document, width, -1, selecting);
                    });
                } else {
                    self.view_focused(|document, view| view.move_vertical(document, -1, selecting));
                }
            }
            Command::MoveDown { selecting } => {
                if self.config.effective.editor.soft_wrap {
                    let width = self
                        .focused_editor_content_geometry()?
                        .map_or(1, |value| value.width);
                    self.view_focused(|document, view| {
                        view.move_vertical_wrapped(document, width, 1, selecting);
                    });
                } else {
                    self.view_focused(|document, view| view.move_vertical(document, 1, selecting));
                }
            }
            Command::Home { selecting } => {
                self.view_focused(|document, view| view.move_home(document, selecting));
            }
            Command::End { selecting } => {
                self.view_focused(|document, view| view.move_end(document, selecting));
            }
            Command::PageUp { selecting } => {
                if self.config.effective.editor.soft_wrap {
                    let content = self.focused_editor_content_geometry()?.unwrap_or(
                        render::EditorContentGeometry {
                            height: 10,
                            horizontal_scrollbar_visible: false,
                            line_number_width: 0,
                            scrollbar_visible: false,
                            width: 1,
                        },
                    );
                    self.view_focused(|document, view| {
                        view.page_wrapped(
                            document,
                            content.width,
                            content.height,
                            false,
                            selecting,
                        );
                    });
                } else {
                    self.view_focused(|document, view| view.page(document, 10, false, selecting));
                }
            }
            Command::PageDown { selecting } => {
                if self.config.effective.editor.soft_wrap {
                    let content = self.focused_editor_content_geometry()?.unwrap_or(
                        render::EditorContentGeometry {
                            height: 10,
                            horizontal_scrollbar_visible: false,
                            line_number_width: 0,
                            scrollbar_visible: false,
                            width: 1,
                        },
                    );
                    self.view_focused(|document, view| {
                        view.page_wrapped(document, content.width, content.height, true, selecting);
                    });
                } else {
                    self.view_focused(|document, view| view.page(document, 10, true, selecting));
                }
            }
            Command::Scroll(delta) => {
                if self.focus_owner == FocusOwner::BottomTerminal {
                    if let Some(session) = self.active_bottom_terminal_mut() {
                        session.scroll(-isize::from(delta));
                    }
                } else {
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
                    let height = geometry
                        .pane_layout
                        .panes
                        .get(&self.workspace.focused)
                        .map_or(1, |area| usize::from(area.height.saturating_sub(2)));
                    let wrapped_width = if self.config.effective.editor.soft_wrap {
                        let Pane::Editor { document_id, .. } = self.workspace.focused_pane() else {
                            return Ok(());
                        };
                        geometry
                            .pane_layout
                            .panes
                            .get(&self.workspace.focused)
                            .map_or(1, |area| {
                                render::editor_content_geometry(
                                    self,
                                    *area,
                                    self.documents.get(*document_id),
                                )
                                .width
                            })
                    } else {
                        1
                    };
                    match self.workspace.focused_pane_mut() {
                        Pane::Empty => {}
                        Pane::Editor { document_id, view } => {
                            let document = self.documents.get(*document_id);
                            if self.config.effective.editor.soft_wrap {
                                view.scroll_vertical_wrapped(
                                    document,
                                    wrapped_width,
                                    height,
                                    isize::from(delta),
                                );
                            } else {
                                view.scroll_vertical(document, height, isize::from(delta));
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
            Pane::Empty => None,
        }
    }

    fn open_search_replace(&mut self) {
        let Some((document_id, _)) = self.focused_editor() else {
            return;
        };
        let query = match self.workspace.focused_pane() {
            Pane::Editor { view, .. } => view
                .selected_text(self.documents.get(document_id))
                .unwrap_or_default(),
            Pane::Empty => String::new(),
        };
        self.overlay = Some(Overlay::SearchReplace {
            document_id,
            query,
            replacement: String::new(),
            editing_replacement: false,
            current_match: None,
        });
        self.focus_owner = FocusOwner::Overlay;
        self.select_search_match(true);
    }

    fn select_search_match(&mut self, forward: bool) {
        let Some((document_id, query, current_match)) = self.overlay.as_ref().and_then(|overlay| {
            let Overlay::SearchReplace {
                document_id,
                query,
                current_match,
                ..
            } = overlay
            else {
                return None;
            };
            Some((*document_id, query.clone(), current_match.clone()))
        }) else {
            return;
        };
        let matches = literal_matches(&self.documents.get(document_id).text(), &query);
        let cursor = match self.workspace.focused_pane() {
            Pane::Editor { view, .. } => view.cursor,
            Pane::Empty => 0,
        };
        let matched = if forward {
            let start = current_match.as_ref().map_or(cursor, |range| range.end);
            matches
                .iter()
                .find(|range| range.start >= start)
                .cloned()
                .or_else(|| matches.first().cloned())
        } else {
            let end = current_match.as_ref().map_or(cursor, |range| range.start);
            matches
                .iter()
                .rev()
                .find(|range| range.end <= end)
                .cloned()
                .or_else(|| matches.last().cloned())
        };
        if let Some(range) = matched.as_ref()
            && let Pane::Editor {
                document_id: active_id,
                view,
            } = self.workspace.focused_pane_mut()
            && *active_id == document_id
        {
            view.selection_anchor = Some(range.start);
            view.cursor = range.end;
        }
        if let Some(Overlay::SearchReplace { current_match, .. }) = self.overlay.as_mut() {
            *current_match = matched;
        }
    }

    fn replace_search_match(&mut self) {
        let Some((document_id, query, replacement, current_match)) =
            self.overlay.as_ref().and_then(|overlay| {
                let Overlay::SearchReplace {
                    document_id,
                    query,
                    replacement,
                    current_match,
                    ..
                } = overlay
                else {
                    return None;
                };
                Some((
                    *document_id,
                    query.clone(),
                    replacement.clone(),
                    current_match.clone(),
                ))
            })
        else {
            return;
        };
        let Some(range) = current_match else {
            return;
        };
        if self
            .documents
            .get(document_id)
            .slice(range.start, range.end)
            != query
        {
            self.select_search_match(true);
            return;
        }
        self.edit_focused(|document, view| {
            let end = document.replace(range.start, range.end, &replacement);
            view.selection_anchor = Some(range.start);
            view.cursor = end;
        });
        if let Some(Overlay::SearchReplace { current_match, .. }) = self.overlay.as_mut() {
            *current_match = None;
        }
        self.select_search_match(true);
    }

    fn replace_all_search_matches(&mut self) {
        let Some((document_id, query, replacement)) = self.overlay.as_ref().and_then(|overlay| {
            let Overlay::SearchReplace {
                document_id,
                query,
                replacement,
                ..
            } = overlay
            else {
                return None;
            };
            Some((*document_id, query.clone(), replacement.clone()))
        }) else {
            return;
        };
        let matches = literal_matches(&self.documents.get(document_id).text(), &query);
        for range in matches.into_iter().rev() {
            self.edit_focused(|document, view| {
                view.cursor = document.replace(range.start, range.end, &replacement);
                view.clear_selection();
            });
        }
        if let Some(Overlay::SearchReplace { current_match, .. }) = self.overlay.as_mut() {
            *current_match = None;
        }
        self.select_search_match(true);
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
            .bottom_terminals
            .iter_mut()
            .any(|session| session.is_alive().unwrap_or(true));
        if !has_live_terminal {
            self.running = false;
        } else {
            self.overlay = Some(Overlay::ConfirmTerminateAll);
            self.focus_owner = FocusOwner::Overlay;
        }
    }

    fn continue_quit_sequence(&mut self) -> Result<()> {
        let Some(document_id) = self
            .documents
            .iter()
            .find_map(|(id, document)| document.is_modified().then_some(id))
        else {
            self.quit_in_progress = false;
            self.request_quit();
            return Ok(());
        };
        if let Some((pane_id, _)) = self.workspace.panes.iter().find(|(_, pane)| {
            matches!(pane, Pane::Editor { document_id: open_id, .. } if *open_id == document_id)
        }) {
            self.workspace.focused = *pane_id;
        } else {
            self.switch_editor_document(self.workspace.focused, document_id);
        }
        self.focus_owner = FocusOwner::Workspace;
        self.request_close()
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

    fn refresh_document_lifecycle(&mut self, now: Instant) -> bool {
        let mut redraw = false;
        let autosave_interval = self.config.effective.editor.autosave_interval_seconds;
        let autosave_interval = u64::try_from(autosave_interval).unwrap_or(0);
        if autosave_interval > 0
            && now.duration_since(self.last_autosave) >= Duration::from_secs(autosave_interval)
        {
            self.last_autosave = now;
            for (document_id, modified) in self
                .documents
                .iter()
                .map(|(id, document)| (id, document.is_modified()))
                .collect::<Vec<_>>()
            {
                let result = if modified {
                    write_recovery_record(
                        &self.recovery_directory,
                        document_id,
                        self.documents.get(document_id),
                    )
                    .map(|_| ())
                } else {
                    remove_recovery_record(
                        &self.recovery_directory,
                        document_id,
                        self.documents.get(document_id),
                    )
                };
                if let Err(error) = result {
                    self.message = Some(format!("Autosave recovery failed: {error}"));
                }
            }
        }
        if self.overlay.is_some()
            || now.duration_since(self.last_external_file_check) < Duration::from_secs(2)
        {
            return redraw;
        }
        self.last_external_file_check = now;
        const EXTERNAL_FILE_POLL_BATCH: usize = 8;
        let named_documents = self
            .documents
            .iter()
            .filter(|(_, document)| document.path.is_some())
            .map(|(id, document)| (id, document.is_modified()))
            .collect::<Vec<_>>();
        if named_documents.is_empty() {
            return redraw;
        }
        let start = self.external_file_poll_cursor % named_documents.len();
        for index in 0..named_documents.len().min(EXTERNAL_FILE_POLL_BATCH) {
            let (document_id, modified) = named_documents[(start + index) % named_documents.len()];
            let (state, version) = match self.documents.external_file_state_and_version(document_id)
            {
                Ok(state) => state,
                Err(error) => {
                    self.message = Some(format!("Could not inspect open file: {error}"));
                    continue;
                }
            };
            if self.deferred_external_changes.get(&document_id) == version.as_ref() {
                continue;
            }
            self.deferred_external_changes.remove(&document_id);
            match state {
                ExternalFileState::Unchanged => {}
                ExternalFileState::Changed if !modified => {
                    if let Err(error) = self.reload_document(document_id) {
                        self.message = Some(format!("Could not reload changed file: {error}"));
                    } else {
                        self.message = Some("Reloaded file changed outside the editor".to_owned());
                        redraw = true;
                    }
                }
                ExternalFileState::Changed | ExternalFileState::Missing => {
                    let Some(version) = version else {
                        continue;
                    };
                    self.overlay = Some(Overlay::ExternalFileChange {
                        document_id,
                        missing: state == ExternalFileState::Missing,
                        version,
                    });
                    self.focus_owner = FocusOwner::Overlay;
                    self.external_file_poll_cursor = (start + index + 1) % named_documents.len();
                    return true;
                }
            }
        }
        self.external_file_poll_cursor =
            (start + named_documents.len().min(EXTERNAL_FILE_POLL_BATCH)) % named_documents.len();
        redraw
    }

    fn show_next_recovery(&mut self) {
        if self.overlay.is_some() {
            return;
        }
        if let Some((path, record)) = self.recovery_records.pop_front() {
            self.overlay = Some(Overlay::Recovery { path, record });
            self.focus_owner = FocusOwner::Overlay;
            return;
        }
        if let Some((path, warning)) = self.pending_startup_warning.take() {
            self.overlay = Some(Overlay::ConfirmUnsafeOpen {
                path,
                split: None,
                warning,
            });
            self.focus_owner = FocusOwner::Overlay;
        }
    }

    fn restore_recovery_record(&mut self, record: RecoveryRecord) -> Result<()> {
        let document_id = match record.path {
            Some(path) => self.documents.new_at_path(path)?,
            None => self.documents.new_untitled(),
        };
        self.documents.get_mut(document_id).encoding = record.encoding;
        self.documents.get_mut(document_id).insert(0, &record.text);
        let pane_id = self.workspace.focused;
        self.switch_editor_document(pane_id, document_id);
        Ok(())
    }

    fn remove_recovery_for_document(&mut self, document_id: DocumentId) -> Result<()> {
        remove_recovery_record(
            &self.recovery_directory,
            document_id,
            self.documents.get(document_id),
        )
    }

    fn request_close(&mut self) -> Result<()> {
        document_lifecycle::request_close(self)
    }

    fn discard_focused_document(&mut self) {
        self.close_focused_editor_view();
    }

    fn close_focused_editor_view(&mut self) {
        document_lifecycle::close_focused_editor_view(self);
    }

    fn unload_document_without_views(&mut self, document_id: crate::document::DocumentId) {
        let still_visible = self.workspace.panes.values().any(|pane| {
            matches!(pane, Pane::Editor { document_id: open_id, .. } if *open_id == document_id)
        });
        if !still_visible {
            self.deferred_external_changes.remove(&document_id);
            if let Err(error) = self.remove_recovery_for_document(document_id) {
                self.message = Some(format!("Could not remove recovery data: {error}"));
            }
            self.documents.remove(document_id);
            self.saved_editor_views.remove(&document_id);
        }
    }

    fn switch_editor_document(
        &mut self,
        pane_id: crate::layout::PaneId,
        document_id: crate::document::DocumentId,
    ) {
        let current = self
            .workspace
            .panes
            .get(&pane_id)
            .and_then(|pane| match pane {
                Pane::Editor { document_id, view } => Some((*document_id, view.clone())),
                Pane::Empty => None,
            });
        if current
            .as_ref()
            .is_some_and(|(current_id, _)| *current_id == document_id)
        {
            return;
        }
        if let Some((current_id, view)) = current {
            let remains_visible = self.workspace.panes.iter().any(|(other_id, pane)| {
                *other_id != pane_id
                    && matches!(pane, Pane::Editor { document_id, .. } if *document_id == current_id)
            });
            if !remains_visible {
                self.saved_editor_views.insert(current_id, view);
            }
        }
        let view = self
            .saved_editor_views
            .remove(&document_id)
            .unwrap_or_default();
        self.workspace
            .panes
            .insert(pane_id, Pane::Editor { document_id, view });
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
            if let Some(view) = self.saved_editor_views.get_mut(&document_id) {
                view.cursor = transform_position(view.cursor, change);
                view.selection_anchor = view
                    .selection_anchor
                    .map(|position| transform_position(position, change));
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
            if let Some(session) = self.active_bottom_terminal_mut() {
                session.write(text.as_bytes())?;
            }
            return Ok(());
        }
        self.edit_focused(|document, view| view.insert(document, text));
        Ok(())
    }

    fn paste_text(&mut self, text: &str) -> Result<()> {
        if self.focus_owner != FocusOwner::BottomTerminal {
            return self.insert_text(text);
        }
        let bracketed = self
            .active_bottom_terminal()
            .is_some_and(|session| session.modes().bracketed_paste);
        let bytes = terminal_paste_bytes(text, bracketed);
        if let Some(session) = self.active_bottom_terminal_mut() {
            session.write(&bytes)?;
        }
        Ok(())
    }

    fn insert_typed_text(&mut self, text: &str) -> Result<()> {
        if self.focus_owner == FocusOwner::BottomTerminal {
            return self.insert_text(text);
        }
        self.edit_focused(|document, view| view.insert_typed(document, text));
        Ok(())
    }

    fn copy_selection(&mut self, cut: bool) {
        let terminal_selection = if self.focus_owner == FocusOwner::BottomTerminal {
            self.active_bottom_terminal()
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
        document_lifecycle::save_focused(self)
    }

    fn save_document_as(
        &mut self,
        document_id: crate::document::DocumentId,
        path: &Path,
    ) -> Result<()> {
        document_lifecycle::save_document_as(self, document_id, path)
    }

    fn finish_successful_save(&mut self, document_id: crate::document::DocumentId) -> Result<()> {
        document_lifecycle::finish_successful_save(self, document_id)
    }

    fn open_path(&mut self, path: &Path, split: Option<SplitDirection>) -> Result<()> {
        document_lifecycle::open_path(self, path, split)
    }

    fn submit_file_dialog(&mut self) -> Result<()> {
        let Some(Overlay::FileDialog { input, save_as, .. }) = self.overlay.as_ref() else {
            return Ok(());
        };
        let path = PathBuf::from(input);
        let save_as = *save_as;
        if path.is_dir() {
            self.overlay = Some(file_dialog(&path, save_as, &self.explorer));
            return Ok(());
        }
        let result = if save_as {
            let document_id = self.focused_editor().map(|(id, _)| id);
            document_id.map_or(Ok(()), |id| self.save_document_as(id, &path))
        } else {
            self.open_path(&path, None)
        };
        match result {
            Ok(()) => {
                if matches!(self.overlay, Some(Overlay::FileDialog { .. })) {
                    self.overlay = None;
                    self.focus_owner = FocusOwner::Workspace;
                }
            }
            Err(open_error) => {
                if let Some(Overlay::FileDialog { error, .. }) = self.overlay.as_mut() {
                    *error = Some(open_error.to_string());
                }
            }
        }
        Ok(())
    }

    fn open_explorer_create(&mut self, create_directory: bool) {
        let directory = self
            .explorer
            .selected_entry()
            .map(|entry| {
                if entry.is_directory {
                    entry.path.clone()
                } else {
                    entry
                        .path
                        .parent()
                        .unwrap_or(&self.explorer.root)
                        .to_owned()
                }
            })
            .unwrap_or_else(|| self.explorer.root.clone());
        self.overlay = Some(explorer_create_dialog(&directory, create_directory));
        self.focus_owner = FocusOwner::Overlay;
    }

    fn submit_explorer_create(&mut self) -> Result<()> {
        let Some(Overlay::ExplorerCreate {
            directory,
            name,
            create_directory,
            ..
        }) = self.overlay.as_ref()
        else {
            return Ok(());
        };
        let directory = directory.clone();
        let name = name.trim();
        if name.is_empty() || Path::new(name).components().count() != 1 {
            if let Some(Overlay::ExplorerCreate { error, .. }) = self.overlay.as_mut() {
                *error = Some("Enter a single file or folder name".to_owned());
            }
            return Ok(());
        }
        let path = directory.join(name);
        if !path.starts_with(&self.explorer.root) || path.exists() {
            if let Some(Overlay::ExplorerCreate { error, .. }) = self.overlay.as_mut() {
                *error = Some("The destination is unavailable".to_owned());
            }
            return Ok(());
        }
        let result = if *create_directory {
            fs::create_dir(&path)
        } else {
            fs::File::create(&path).map(|_| ())
        };
        if let Err(error) = result {
            if let Some(Overlay::ExplorerCreate { error: message, .. }) = self.overlay.as_mut() {
                *message = Some(error.to_string());
            }
            return Ok(());
        }
        self.explorer.refresh()?;
        self.explorer.selected = self
            .explorer
            .entries()
            .iter()
            .position(|entry| entry.path == path)
            .unwrap_or(self.explorer.selected);
        self.overlay = None;
        self.focus_owner = FocusOwner::Explorer;
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

    pub fn git_branch(&self) -> Option<String> {
        detect_git_branch(&self.explorer.root)
    }

    pub fn status_text(&self) -> String {
        if let Some(message) = &self.message {
            return message.clone();
        }
        if self.input.leader_pending() {
            return format!("Leader {} pending", self.config.effective.terminal.leader);
        }
        if let Some(action) = self.title_control_hover {
            return action.status_hint().to_owned();
        }
        if self.focus_owner == FocusOwner::BottomTerminal {
            return format!("Terminal  {} leader", self.config.effective.terminal.leader);
        }
        match self.workspace.focused_pane() {
            Pane::Empty => "No editor  Type to create a new file".to_owned(),
            Pane::Editor { document_id, view } => {
                let document = self.documents.get(*document_id);
                let line = document.char_to_line(view.cursor) + 1;
                let column = crate::editor::display_column(document, line - 1, view.cursor) + 1;
                let line_ending = match document.line_ending {
                    crate::config::LineEnding::Lf => "LF",
                    crate::config::LineEnding::Crlf => "CRLF",
                };
                let encoding = document.encoding.label();
                let horizontal_offset =
                    if !self.config.effective.editor.soft_wrap && view.viewport_column > 0 {
                        format!("  HScroll {}", view.viewport_column)
                    } else {
                        String::new()
                    };
                format!(
                    "Edit  Ln {line}, Col {column}  {encoding}  {line_ending}{horizontal_offset}"
                )
            }
        }
    }

    fn ensure_focused_editor_visible(&mut self, columns: u16, rows: u16) {
        if self.focus_owner != FocusOwner::Workspace {
            return;
        }
        let geometry = render::geometry(
            self,
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
        );
        let focused = self.workspace.focused;
        let Some(area) = geometry.pane_layout.panes.get(&focused).copied() else {
            return;
        };
        let Pane::Editor { document_id, .. } = self.workspace.focused_pane() else {
            return;
        };
        let document_id = *document_id;
        let document = self.documents.get(document_id);
        let content = render::editor_content_geometry(self, area, document);
        let (documents, workspace) = (&self.documents, &mut self.workspace);
        let Pane::Editor { document_id, view } = workspace.focused_pane_mut() else {
            return;
        };
        if self.config.effective.editor.soft_wrap {
            view.ensure_visible_wrapped(documents.get(*document_id), content.height, content.width);
        } else {
            view.viewport_wrap_segment = 0;
            view.ensure_visible(
                documents.get(*document_id),
                content.height,
                content.width.saturating_sub(1).max(1),
            );
            view.clamp_horizontal(documents.get(*document_id), content.width);
        }
    }

    fn focused_editor_content_geometry(&self) -> Result<Option<render::EditorContentGeometry>> {
        let Pane::Editor { document_id, .. } = self.workspace.focused_pane() else {
            return Ok(None);
        };
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
        Ok(geometry
            .pane_layout
            .panes
            .get(&self.workspace.focused)
            .map(|area| {
                render::editor_content_geometry(self, *area, self.documents.get(*document_id))
            }))
    }

    fn handle_event_and_sync_editor_viewport(
        &mut self,
        event: Event,
        columns: u16,
        rows: u16,
    ) -> Result<()> {
        let before = self.editor_visibility_anchor(columns, rows);
        let resized = matches!(event, Event::Resize(_, _));
        let preserves_viewport = self.event_preserves_editor_viewport(&event);
        self.handle_event(event)?;
        let preserves_viewport = preserves_viewport
            || self.scrollbar_drag
            || self.horizontal_scrollbar_drag_offset.is_some();
        self.sync_editor_viewport_after_event(before, columns, rows, resized, preserves_viewport);
        self.clamp_editor_horizontal_offsets(columns, rows);
        Ok(())
    }

    fn clamp_editor_horizontal_offsets(&mut self, columns: u16, rows: u16) {
        let geometry = render::geometry(
            self,
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
        );
        let panes = geometry
            .pane_layout
            .panes
            .iter()
            .filter_map(|(pane_id, area)| {
                let Pane::Editor { document_id, .. } = self.workspace.panes.get(pane_id)? else {
                    return None;
                };
                let content =
                    render::editor_content_geometry(self, *area, self.documents.get(*document_id));
                Some((*pane_id, *document_id, content.width))
            })
            .collect::<Vec<_>>();
        for (pane_id, document_id, width) in panes {
            let Some(Pane::Editor { view, .. }) = self.workspace.panes.get_mut(&pane_id) else {
                continue;
            };
            if self.config.effective.editor.soft_wrap {
                view.viewport_column = 0;
            } else {
                view.clamp_horizontal(self.documents.get(document_id), width);
            }
        }
    }

    fn event_preserves_editor_viewport(&self, event: &Event) -> bool {
        self.scrollbar_drag
            || self.horizontal_scrollbar_drag_offset.is_some()
            || matches!(
                event,
                Event::Mouse(mouse)
                    if matches!(
                        mouse.kind,
                        MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight
                    )
            )
    }

    fn sync_editor_viewport_after_event(
        &mut self,
        before: Option<EditorVisibilityAnchor>,
        columns: u16,
        rows: u16,
        resized: bool,
        preserves_viewport: bool,
    ) {
        if !preserves_viewport
            && (resized || before != self.editor_visibility_anchor(columns, rows))
        {
            self.ensure_focused_editor_visible(columns, rows);
        }
    }

    fn editor_visibility_anchor(&self, columns: u16, rows: u16) -> Option<EditorVisibilityAnchor> {
        if self.focus_owner != FocusOwner::Workspace {
            return None;
        }
        let pane_id = self.workspace.focused;
        let Pane::Editor { document_id, view } = self.workspace.focused_pane() else {
            return None;
        };
        let geometry = render::geometry(
            self,
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
        );
        let area = geometry.pane_layout.panes.get(&pane_id).copied()?;
        Some(EditorVisibilityAnchor {
            pane_id,
            document_id: *document_id,
            revision: self.documents.get(*document_id).revision(),
            cursor: view.cursor,
            area,
        })
    }
}

fn explorer_divider_contains(area: Rect, divider: Option<Rect>, column: u16, row: u16) -> bool {
    (row >= area.y
        && row < area.y.saturating_add(area.height)
        && column == area.x.saturating_add(area.width.saturating_sub(1)))
        || divider.is_some_and(|d| d.contains(column, row))
}

fn bottom_divider_contains(area: Rect, divider: Option<Rect>, column: u16, row: u16) -> bool {
    (column >= area.x && column < area.x.saturating_add(area.width) && row == area.y)
        || divider.is_some_and(|d| d.contains(column, row))
}

fn title_control_command(action: render::TitleControlAction) -> Command {
    match action {
        render::TitleControlAction::ToggleLeftPanel => Command::ToggleExplorer,
        render::TitleControlAction::ToggleBottomPanel => Command::ToggleBottomTerminal,
        render::TitleControlAction::OpenSettings => Command::OpenSettings,
    }
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
            | Command::OpenHelp
            | Command::OpenSearchReplace
    )
}

fn literal_matches(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    text.match_indices(query)
        .map(|(byte_start, matched)| {
            let start = text[..byte_start].chars().count();
            start..start + matched.chars().count()
        })
        .collect()
}

fn file_dialog(directory: &Path, save_as: bool, explorer: &Explorer) -> Overlay {
    let directory = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_owned());
    let mut entries = std::fs::read_dir(&directory)
        .map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .filter(|entry| !explorer.is_excluded(&entry.path()))
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    entries.sort_by_key(|path| {
        path.file_name().map_or_else(String::new, |name| {
            name.to_string_lossy().to_ascii_lowercase()
        })
    });
    let input = if save_as {
        format!("{}{}", directory.display(), std::path::MAIN_SEPARATOR)
    } else {
        directory.display().to_string()
    };
    Overlay::FileDialog {
        directory,
        input,
        entries,
        selected: 0,
        save_as,
        error: None,
    }
}

pub(super) fn explorer_create_dialog(directory: &Path, create_directory: bool) -> Overlay {
    let mut entries = fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .filter_map(|entry| entry.file_type().ok()?.is_dir().then_some(entry.path()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    entries.sort();
    Overlay::ExplorerCreate {
        directory: directory.to_owned(),
        name: String::new(),
        entries,
        selected: 0,
        create_directory,
        error: None,
    }
}

pub(crate) const SETTINGS_KEYS: [&str; 41] = [
    "editor.indent_width",
    "editor.insert_spaces",
    "editor.default_line_ending",
    "editor.undo_limit_mib",
    "editor.line_numbers",
    "editor.scrollbar",
    "editor.soft_wrap",
    "editor.syntax_highlighting",
    "editor.syntax_highlighting_max_file_size_kib",
    "editor.autosave_interval_seconds",
    "editor.header_style",
    "editor.check_modified_files_on_quit",
    "workspace.minimum_pane_width",
    "workspace.minimum_pane_height",
    "workspace.initial_split_ratio",
    "workspace.vertical_divider_thickness",
    "workspace.horizontal_divider_thickness",
    "explorer.visible",
    "explorer.width",
    "explorer.exclusions",
    "explorer.refresh_interval_seconds",
    "explorer.divider_thickness",
    "mouse.enabled",
    "clipboard.osc52",
    "terminal.shell",
    "terminal.arguments",
    "terminal.scrollback_lines",
    "terminal.allow_child_osc52_write",
    "terminal.leader",
    "ui.bottom_terminal_visible",
    "ui.bottom_terminal_height",
    "ui.terminal_divider_thickness",
    "theme.active",
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

pub fn detect_git_branch(start_path: &Path) -> Option<String> {
    let git_path = start_path.join(".git");
    if git_path.is_dir() {
        read_git_head(&git_path.join("HEAD"))
    } else if git_path.is_file()
        && let Ok(content) = fs::read_to_string(&git_path)
        && let Some(gitdir_str) = content.trim().strip_prefix("gitdir:")
    {
        let gitdir = gitdir_str.trim();
        read_git_head(&start_path.join(gitdir).join("HEAD"))
    } else {
        None
    }
}

fn read_git_head(head_path: &Path) -> Option<String> {
    let text = fs::read_to_string(head_path).ok()?;
    let trimmed = text.trim();
    if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
        Some(branch.to_owned())
    } else if let Some(branch) = trimmed.strip_prefix("ref: ") {
        Some(branch.to_owned())
    } else if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(trimmed[..7.min(trimmed.len())].to_owned())
    } else if !trimmed.is_empty() {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn application_with_startup_file(path: &Path) -> Application {
        application_with_startup_path(path)
    }

    fn application_with_startup_path(path: &Path) -> Application {
        let mut application = Application::new_for_test();
        application.documents = DocumentRegistry::new(crate::config::LineEnding::Lf, 1024);
        let (workspace, warning) = startup_workspace(
            Some(path),
            &mut application.documents,
            application.config.effective.workspace.initial_split_ratio,
        )
        .unwrap();
        application.workspace = workspace;
        application.pending_startup_warning = warning;
        application
    }

    #[test]
    fn git_branch_detection_reads_head_branch_and_commit() {
        let directory = tempfile::tempdir().unwrap();
        let git_dir = directory.path().join(".git");
        fs::create_dir(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature-modern-ui\n").unwrap();

        let branch = detect_git_branch(directory.path());
        assert_eq!(branch, Some("feature-modern-ui".to_owned()));

        fs::write(
            git_dir.join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        let detached = detect_git_branch(directory.path());
        assert_eq!(detached, Some("0123456".to_owned()));
    }

    #[test]
    fn status_bar_omits_git_branch_outside_a_repository() {
        let application = Application::new_for_test();

        assert_eq!(application.git_branch(), None);
    }

    #[test]
    fn git_branch_detection_ignores_a_parent_repository() {
        let directory = tempfile::tempdir().unwrap();
        let git_dir = directory.path().join(".git");
        let workspace = directory.path().join("workspace");
        fs::create_dir(&git_dir).unwrap();
        fs::create_dir(&workspace).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        assert_eq!(detect_git_branch(&workspace), None);
    }

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
        let application = application_with_startup_path(directory.path());

        assert!(matches!(application.workspace.focused_pane(), Pane::Empty));
        assert!(application.open_document_ids().is_empty());
        assert_eq!(application.documents.iter().count(), 0);
    }

    #[test]
    fn typing_in_an_empty_editor_creates_one_untitled_document() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = application_with_startup_path(directory.path());
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
        assert_eq!(application.overlay, Some(Overlay::ConfirmDocumentClose));
        assert!(application.running);
    }

    #[test]
    fn quitting_checks_each_modified_document_before_exiting() {
        let mut application = Application::new_for_test();
        let first_document = application.focused_document_id().unwrap();
        application
            .apply(Command::Insert("first".to_owned()))
            .unwrap();
        let second_document = application.documents.new_untitled();
        application.switch_editor_document(application.workspace.focused, second_document);
        application
            .apply(Command::Insert("second".to_owned()))
            .unwrap();
        application.switch_editor_document(application.workspace.focused, first_document);
        let path = application.explorer.root.join("first.txt");

        application.apply(Command::Quit).unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        let Some(Overlay::FileDialog { input, .. }) = application.overlay.as_mut() else {
            panic!("expected Save As dialog");
        };
        *input = path.display().to_string();
        application.submit_file_dialog().unwrap();

        assert_eq!(application.focused_document_id(), Some(second_document));
        assert_eq!(application.overlay, Some(Overlay::ConfirmDocumentClose));
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert!(!application.running);
    }

    #[test]
    fn quitting_skips_modified_file_checks_when_disabled() {
        let mut application = Application::new_for_test();
        application
            .config
            .effective
            .editor
            .check_modified_files_on_quit = false;
        application.apply(Command::Insert("x".to_owned())).unwrap();

        application.apply(Command::Quit).unwrap();

        assert!(application.overlay.is_none());
        assert!(!application.running);
    }

    #[test]
    fn escaping_a_quit_document_confirmation_cancels_the_quit_sequence() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .apply(Command::Insert("unsaved".to_owned()))
            .unwrap();

        application.apply(Command::Quit).unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert!(application.running);
        assert!(application.overlay.is_none());
        assert!(application.documents.get(document_id).is_modified());
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
    fn search_replace_selects_cycles_and_replaces_document_matches() {
        let mut application = Application::new_for_test();
        application
            .apply(Command::Insert("red blue red".to_owned()))
            .unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('f'),
                KeyModifiers::CONTROL,
            )))
            .unwrap();
        assert!(matches!(
            application.overlay,
            Some(Overlay::SearchReplace { .. })
        ));

        for character in "red".chars() {
            application
                .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Char(character),
                    KeyModifiers::NONE,
                )))
                .unwrap();
        }
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.selection(), Some(0..3));

        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )))
            .unwrap();
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.selection(), Some(9..12));

        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Tab,
                KeyModifiers::NONE,
            )))
            .unwrap();
        for character in "green".chars() {
            application
                .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Char(character),
                    KeyModifiers::NONE,
                )))
                .unwrap();
        }
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL,
            )))
            .unwrap();
        let document_id = application.focused_document_id().unwrap();
        assert_eq!(
            application.documents.get(document_id).text(),
            "red blue green"
        );
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            )))
            .unwrap();

        assert_eq!(
            application.documents.get(document_id).text(),
            "green blue green"
        );
    }

    #[test]
    fn opening_text_files_does_not_require_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opened.txt");
        fs::write(&path, "text").unwrap();
        let mut application = Application::new_for_test();

        application.open_path(&path, None).unwrap();

        assert!(application.overlay.is_none());
        assert_eq!(application.open_document_ids().len(), 1);
    }

    #[test]
    fn syntax_highlighting_can_be_disabled() {
        let mut application = Application::new_for_test();
        let path = application.explorer.root.join("main.rs");
        let document_id = application.documents.new_at_path(path).unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(0, "fn main() {};");
        application.config.effective.editor.syntax_highlighting = false;
        let document = application.documents.get(document_id);

        assert!(
            application
                .syntax_spans(document_id, document, 0)
                .is_empty()
        );
    }

    #[test]
    fn oversized_documents_skip_syntax_highlighting() {
        let mut application = Application::new_for_test();
        let path = application.explorer.root.join("main.rs");
        let document_id = application.documents.new_at_path(path).unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(0, "fn main() {};");
        application
            .config
            .effective
            .editor
            .syntax_highlighting_max_file_size_kib = 0;
        let document = application.documents.get(document_id);

        assert!(
            application
                .syntax_spans(document_id, document, 0)
                .is_empty()
        );
    }

    #[test]
    fn reloading_document_invalidates_cached_syntax() {
        let mut application = Application::new_for_test();
        let path = application.explorer.root.join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        let (document_id, _) = application.documents.open(&path).unwrap();
        let document = application.documents.get(document_id);
        let before = application.syntax_spans(document_id, document, 0);

        fs::write(&path, "// changed externally\n").unwrap();
        application.reload_document(document_id).unwrap();
        let document = application.documents.get(document_id);
        let after = application.syntax_spans(document_id, document, 0);

        assert_ne!(before, after);
    }

    #[test]
    fn flagged_startup_file_is_opened_only_after_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binary.dat");
        fs::write(&path, b"binary\0data").unwrap();
        let mut application = application_with_startup_file(&path);
        application.show_next_recovery();

        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmUnsafeOpen { .. })
        ));
        assert!(application.open_document_ids().is_empty());

        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('o'),
                KeyModifiers::NONE,
            )))
            .unwrap();

        let document_id = application.focused_document_id().unwrap();
        assert_eq!(
            application.documents.get(document_id).path.as_deref(),
            Some(fs::canonicalize(path).unwrap().as_path())
        );
    }

    #[test]
    fn flagged_startup_file_can_be_canceled_without_opening_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binary.dat");
        fs::write(&path, b"binary\0data").unwrap();
        let mut application = application_with_startup_file(&path);
        application.show_next_recovery();

        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert!(application.overlay.is_none());
        assert!(application.open_document_ids().is_empty());
        assert!(matches!(application.workspace.focused_pane(), Pane::Empty));
    }

    #[test]
    fn startup_warning_waits_until_recovery_prompts_are_dismissed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binary.dat");
        fs::write(&path, b"binary\0data").unwrap();
        let mut application = application_with_startup_file(&path);
        application.recovery_records.push_back((
            directory.path().join("recovery.json"),
            RecoveryRecord {
                format_version: 1,
                path: None,
                revision: 0,
                captured_at_unix_seconds: 0,
                encoding: crate::document::EncodingState::Utf8,
                text: String::new(),
            },
        ));

        application.show_next_recovery();
        assert!(matches!(
            application.overlay,
            Some(Overlay::Recovery { .. })
        ));
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmUnsafeOpen { .. })
        ));
    }

    #[test]
    fn unsafe_file_open_can_be_cancelled_or_confirmed_in_a_split() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binary.dat");
        fs::write(&path, b"binary\0data").unwrap();
        let mut application = Application::new_for_test();

        application
            .open_path(&path, Some(SplitDirection::Vertical))
            .unwrap();
        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmUnsafeOpen { .. })
        ));
        assert_eq!(application.open_document_ids().len(), 1);

        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert!(application.overlay.is_none());
        assert_eq!(application.open_document_ids().len(), 1);

        application
            .open_path(&path, Some(SplitDirection::Vertical))
            .unwrap();
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('o'),
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert!(application.overlay.is_none());
        assert_eq!(application.open_document_ids().len(), 2);
        assert_eq!(application.workspace.panes.len(), 2);
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
    fn hiding_the_bottom_terminal_preserves_the_single_session() {
        let mut application = Application::new_for_test();
        application
            .bottom_terminals
            .push(application.spawn_terminal(None).unwrap());
        application.bottom_terminal_visible = true;
        application.focus_owner = FocusOwner::BottomTerminal;
        let session = application.active_bottom_terminal().unwrap() as *const TerminalSession;

        application.toggle_bottom_terminal().unwrap();

        assert!(!application.bottom_terminal_visible);
        assert_eq!(application.focus_owner, FocusOwner::Workspace);
        assert_eq!(
            application.active_bottom_terminal().unwrap() as *const TerminalSession,
            session
        );
        assert_eq!(application.workspace.panes.len(), 1);
    }

    #[test]
    fn confirmed_terminal_close_removes_the_only_session_without_changing_editors() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .bottom_terminals
            .push(application.spawn_terminal(None).unwrap());
        application.bottom_terminal_visible = true;
        application.focus_owner = FocusOwner::BottomTerminal;

        application.request_close().unwrap();
        assert_eq!(application.overlay, Some(Overlay::ConfirmTerminalClose));
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('t'),
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert!(application.bottom_terminals.is_empty());
        assert!(!application.bottom_terminal_visible);
        assert_eq!(application.focused_document_id(), Some(document_id));
        assert_eq!(application.workspace.panes.len(), 1);
    }

    #[test]
    fn terminal_sessions_cycle_wrap_and_close_independently() {
        let mut application = Application::new_for_test();
        application
            .bottom_terminals
            .push(application.spawn_terminal(None).unwrap());
        application
            .bottom_terminals
            .push(application.spawn_terminal(None).unwrap());
        application.bottom_terminal_visible = true;

        application.cycle_bottom_terminal(-1);
        assert_eq!(application.active_bottom_terminal, 1);
        application.cycle_bottom_terminal(1);
        assert_eq!(application.active_bottom_terminal, 0);

        application.close_active_bottom_terminal();
        assert_eq!(application.bottom_terminals.len(), 1);
        assert_eq!(application.active_bottom_terminal, 0);
        assert!(application.bottom_terminal_visible);
    }

    #[test]
    fn terminal_tab_hit_testing_selects_the_matching_session() {
        let mut application = Application::new_for_test();
        application
            .bottom_terminals
            .push(application.spawn_terminal(None).unwrap());
        application
            .bottom_terminals
            .push(application.spawn_terminal(None).unwrap());
        let area = Rect {
            x: 0,
            y: 10,
            width: 80,
            height: 8,
        };

        let second_tab_column = (1..area.width)
            .find(|column| render::terminal_tab_at(&application, area, *column, area.y) == Some(1))
            .unwrap();
        let second_tab =
            render::terminal_tab_at(&application, area, second_tab_column, area.y).unwrap();
        application.select_bottom_terminal(second_tab);

        assert_eq!(second_tab, 1);
        assert_eq!(application.active_bottom_terminal, 1);
        assert_eq!(application.focus_owner, FocusOwner::BottomTerminal);
    }

    #[test]
    fn terminal_session_limit_prevents_new_session_creation() {
        let mut application = Application::new_for_test();
        for _ in 0..8 {
            application
                .bottom_terminals
                .push(application.spawn_terminal(None).unwrap());
        }

        application.new_bottom_terminal().unwrap();

        assert_eq!(application.bottom_terminals.len(), 8);
        assert_eq!(
            application.message.as_deref(),
            Some("Terminal limit reached (8)")
        );
    }

    #[test]
    fn terminal_input_does_not_create_an_untitled_document() {
        let directory = tempfile::tempdir().unwrap();
        let mut application = application_with_startup_path(directory.path());
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
    fn terminal_paste_wraps_data_only_when_bracketed_paste_is_enabled() {
        assert_eq!(
            terminal_paste_bytes("line\n", true),
            b"\x1b[200~line\n\x1b[201~"
        );
        assert_eq!(terminal_paste_bytes("line\n", false), b"line\n");
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
    fn raw_configuration_reload_updates_runtime_input_and_explorer_settings() {
        let mut application = Application::new_for_test();
        fs::write(
            &application.config.user_path,
            "[terminal]\nleader = \"ctrl+a\"\n[explorer]\nrefresh_interval_seconds = 9\n[editor]\nsoft_wrap = true\n",
        )
        .unwrap();
        application.reload_config_after_raw_save().unwrap();
        assert_eq!(
            application
                .config
                .effective
                .explorer
                .refresh_interval_seconds,
            9
        );
        assert!(application.config.effective.editor.soft_wrap);

        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('a'),
                KeyModifiers::CONTROL,
            )))
            .unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('h'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert_eq!(application.workspace.panes.len(), 2);
    }

    #[test]
    fn invalid_raw_configuration_keeps_the_last_valid_runtime_settings() {
        let mut application = Application::new_for_test();
        fs::write(
            &application.config.user_path,
            "[terminal]\nleader = \"ctrl+a\"\n",
        )
        .unwrap();
        application.reload_config_after_raw_save().unwrap();
        fs::write(&application.config.user_path, "[terminal\n").unwrap();
        application.reload_config_after_raw_save().unwrap();

        assert_eq!(application.config.effective.terminal.leader, "ctrl+a");
        assert!(application.message.as_deref().is_some_and(|message| {
            message.contains("previous runtime settings remain active")
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
            area.y + 4 + application.documents.iter().count() as u16 + readme_index as u16;
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
            application.explorer_entry_at(area.y + 4 + readme_index as u16, area),
            Some(readme_index)
        );
        assert!(!application.toggle_open_editors_at(area.y + 2, area));
    }

    #[test]
    fn explorer_folder_single_click_on_icon_toggles_and_double_click_on_name_toggles() {
        let mut application = Application::new_for_test();
        let sub_dir = application.explorer.root.join("subdir");
        fs::create_dir(&sub_dir).unwrap();
        fs::write(sub_dir.join("file.txt"), "hello").unwrap();
        application.explorer.refresh().unwrap();

        let area = Rect {
            x: 0,
            y: 1,
            width: 24,
            height: 20,
        };
        let subdir_index = application
            .explorer
            .entries()
            .iter()
            .position(|entry| entry.path == sub_dir)
            .unwrap();

        let row = area.y + 4 + application.documents.iter().count() as u16 + subdir_index as u16;
        assert_eq!(application.explorer_entry_at(row, area), Some(subdir_index));

        // Subdir is initially collapsed
        assert!(!application.explorer.entries()[subdir_index].expanded);

        let icon_col = area.x + 1 + 2; // depth 0 -> indent width 2 -> icon at x + 3
        let name_col = area.x + 1 + 6; // folder name starts at x + 5..6

        // Single click on folder icon: toggles expansion (expands)
        let entry = application.explorer.entries()[subdir_index].clone();
        assert!(entry.is_directory);
        application.explorer.selected = subdir_index;
        let is_icon_click = icon_col <= area.x + 1 + 2 + 1;
        assert!(is_icon_click);
        application.explorer.toggle_selected().unwrap();
        assert!(application.explorer.entries()[subdir_index].expanded);

        // Single click on folder icon again: toggles expansion (collapses)
        application.explorer.toggle_selected().unwrap();
        assert!(!application.explorer.entries()[subdir_index].expanded);

        // Single click on folder name: does not toggle
        let is_name_icon_click = name_col <= area.x + 1 + 2 + 1;
        assert!(!is_name_icon_click);
        // Only click_count >= 2 toggles on name
        let click_count_single = 1;
        if is_name_icon_click || click_count_single >= 2 {
            application.explorer.toggle_selected().unwrap();
        }
        assert!(!application.explorer.entries()[subdir_index].expanded);

        // Double click on folder name: toggles expansion (expands)
        let click_count_double = 2;
        if is_name_icon_click || click_count_double >= 2 {
            application.explorer.toggle_selected().unwrap();
        }
        assert!(application.explorer.entries()[subdir_index].expanded);
    }

    #[test]
    fn editor_tab_click_switches_active_document_in_pane() {
        let mut application = Application::new_for_test();
        let first_document = application.focused_document_id().unwrap();
        let second_document = application.documents.new_untitled();
        let geometry = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        let focused_id = application.workspace.focused;
        let pane_area = *geometry.pane_layout.panes.get(&focused_id).unwrap();

        // First tab is " Untitled ", width is 10 chars, starts at pane_area.x + 1
        // Second tab starts after divider "│"
        // Let's test render::editor_tab_at
        let clicked_first =
            render::editor_tab_at(&application, pane_area, pane_area.x + 3, pane_area.y);
        assert_eq!(clicked_first, Some(first_document));

        let clicked_second =
            render::editor_tab_at(&application, pane_area, pane_area.x + 15, pane_area.y);
        assert_eq!(clicked_second, Some(second_document));

        // Switch via editor tab click
        application.switch_editor_document(focused_id, second_document);
        assert_eq!(application.focused_document_id(), Some(second_document));
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
    fn open_editor_switching_preserves_cursor_selection_and_viewport() {
        let mut application = Application::new_for_test();
        let first_document = application.focused_document_id().unwrap();
        application
            .documents
            .get_mut(first_document)
            .insert(0, "first\nsecond");
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.cursor = 8;
        view.selection_anchor = Some(2);
        view.viewport_line = 1;
        view.viewport_column = 3;
        let second_document = application.documents.new_untitled();
        let area = Rect {
            x: 0,
            y: 1,
            width: 24,
            height: 20,
        };

        assert!(application.activate_open_editor_at(area.y + 3, area));
        assert_eq!(application.focused_document_id(), Some(second_document));
        assert!(application.activate_open_editor_at(area.y + 2, area));

        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.cursor, 8);
        assert_eq!(view.selection_anchor, Some(2));
        assert_eq!(view.viewport_line, 1);
        assert_eq!(view.viewport_column, 3);
    }

    #[test]
    fn mouse_scroll_and_scrollbar_share_the_last_full_viewport_bound() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        let text = (0..100)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        application.documents.get_mut(document_id).insert(0, &text);
        let geometry = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        let area = geometry.pane_layout.panes[&application.workspace.focused];
        let height = usize::from(area.height.saturating_sub(2));
        let maximum = crate::editor::EditorView::maximum_viewport_line(
            application.documents.get(document_id),
            height,
        );
        let cursor = 3;
        let selection_anchor = Some(1);
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.cursor = cursor;
        view.selection_anchor = selection_anchor;

        application.scroll_at(
            &geometry,
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            isize::MAX,
        );
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.viewport_line, maximum);
        assert_eq!(view.cursor, cursor);
        assert_eq!(view.selection_anchor, selection_anchor);

        application.update_scrollbar_drag(
            &geometry.pane_layout,
            area.y.saturating_add(area.height.saturating_sub(2)),
        );
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.viewport_line, maximum);
    }

    #[test]
    fn horizontal_mouse_routes_native_and_shift_modified_wheel_events() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(0, &format!("short\n{}", "x".repeat(200)));
        let geometry = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        let area = geometry.pane_layout.panes[&application.workspace.focused];
        let column = area.x.saturating_add(1);
        let row = area.y.saturating_add(1);

        for (kind, modifiers, expected) in [
            (MouseEventKind::ScrollRight, KeyModifiers::NONE, 3),
            (MouseEventKind::ScrollLeft, KeyModifiers::NONE, 0),
            (MouseEventKind::ScrollDown, KeyModifiers::SHIFT, 3),
            (MouseEventKind::ScrollUp, KeyModifiers::SHIFT, 0),
        ] {
            assert!(application.route_mouse_scroll(
                &geometry,
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers,
                },
            ));
            let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
                panic!("expected editor pane");
            };
            assert_eq!(view.viewport_column, expected);
            assert_eq!(view.cursor, 0);
        }

        assert!(application.route_mouse_scroll(
            &geometry,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
        ));
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.viewport_column, 0);
        assert_eq!(view.cursor, 0);
        assert!(
            application.event_preserves_editor_viewport(&Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollRight,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }))
        );

        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.viewport_column = 0;
        application.config.effective.editor.soft_wrap = true;
        application.route_mouse_scroll(
            &geometry,
            MouseEvent {
                kind: MouseEventKind::ScrollRight,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
        );
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.viewport_column, 0);
    }

    #[test]
    fn horizontal_scrollbar_clicks_and_drags_across_the_document_extent() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(0, &format!("short\n{}", "x".repeat(200)));
        let geometry = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        let area = geometry.pane_layout.panes[&application.workspace.focused];
        let content = render::editor_content_geometry(
            &application,
            area,
            application.documents.get(document_id),
        );
        let scrollbar = render::horizontal_scrollbar_geometry(
            area,
            content,
            application.documents.get(document_id),
            0,
        )
        .unwrap();

        assert!(
            application.start_horizontal_scrollbar_drag(
                &geometry.pane_layout,
                scrollbar
                    .track_x
                    .saturating_add(scrollbar.track_width.saturating_sub(1)),
                scrollbar.row,
            )
        );
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.viewport_column, scrollbar.maximum);
        assert_eq!(view.cursor, 0);

        application.update_horizontal_scrollbar_drag(&geometry.pane_layout, scrollbar.track_x);
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.viewport_column, 0);
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn document_shrink_clamps_stale_horizontal_offsets() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(0, &"x".repeat(200));
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.viewport_column = 100;
        application
            .documents
            .get_mut(document_id)
            .replace(0, 200, "short");

        application.clamp_editor_horizontal_offsets(100, 30);

        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.viewport_column, 0);
    }

    #[test]
    fn post_event_visibility_preserves_viewport_only_scrolling() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        let text = (0..100)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        application.documents.get_mut(document_id).insert(0, &text);
        let columns = 100;
        let rows = 30;
        let geometry = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
        );
        let area = geometry.pane_layout.panes[&application.workspace.focused];
        let before = application.editor_visibility_anchor(columns, rows);

        application.scroll_at(
            &geometry,
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            3,
        );
        application.sync_editor_viewport_after_event(before, columns, rows, false, true);

        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.viewport_line, 3);
        assert_eq!(view.cursor, 0);
        assert!(
            application.event_preserves_editor_viewport(&Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: area.x.saturating_add(1),
                row: area.y.saturating_add(1),
                modifiers: KeyModifiers::NONE,
            }))
        );
    }

    #[test]
    fn post_event_visibility_follows_keyboard_cursor_movement() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        let text = (0..100)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        application.documents.get_mut(document_id).insert(0, &text);
        let columns = 100;
        let rows = 30;
        let geometry = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: columns,
                height: rows,
            },
        );
        let area = geometry.pane_layout.panes[&application.workspace.focused];
        let visible_rows = usize::from(area.height.saturating_sub(2));
        let cursor = application
            .documents
            .get(document_id)
            .line_to_char(visible_rows.saturating_sub(1));
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.cursor = cursor;

        application
            .handle_event_and_sync_editor_viewport(
                Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Down,
                    KeyModifiers::NONE,
                )),
                columns,
                rows,
            )
            .unwrap();

        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(
            application
                .documents
                .get(document_id)
                .char_to_line(view.cursor),
            visible_rows
        );
        assert_eq!(view.viewport_line, 1);
    }

    #[test]
    fn post_event_visibility_reanchors_the_cursor_after_resize() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        let text = (0..100)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        application.documents.get_mut(document_id).insert(0, &text);
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.viewport_line = 50;

        application
            .handle_event_and_sync_editor_viewport(Event::Resize(100, 30), 100, 30)
            .unwrap();

        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert_eq!(view.cursor, 0);
        assert_eq!(view.viewport_line, 0);
    }

    #[test]
    fn cursor_navigation_updates_horizontal_viewport_and_status() {
        let mut application = Application::new_for_test();
        application.apply(Command::Insert("x".repeat(200))).unwrap();

        application.ensure_focused_editor_visible(80, 24);

        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert!(view.viewport_column > 0);
        assert!(application.status_text().contains("HScroll"));
    }

    #[test]
    fn title_control_hover_exposes_only_available_action_hints() {
        let mut application = Application::new_for_test();
        let controls = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        )
        .title_controls;
        let explorer = controls.left_panel_toggle.unwrap();
        application.title_control_hover = controls.action_at(explorer.x + 1, explorer.y);
        assert_eq!(application.status_text(), "Toggle Explorer");

        let right = controls.right_panel_toggle.unwrap();
        application.title_control_hover = controls.action_at(right.x + 1, right.y);
        assert_ne!(application.status_text(), "Toggle right panel");
        assert!(application.title_control_hover.is_none());
    }

    #[test]
    fn resize_to_a_taller_pane_clamps_the_editor_viewport() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        let text = (0..100)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        application.documents.get_mut(document_id).insert(0, &text);
        let short_geometry = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 15,
            },
        );
        let short_area = short_geometry.pane_layout.panes[&application.workspace.focused];
        let short_maximum = crate::editor::EditorView::maximum_viewport_line(
            application.documents.get(document_id),
            usize::from(short_area.height.saturating_sub(2)),
        );
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.viewport_line = short_maximum;

        application.resize_terminals(100, 30).unwrap();

        let tall_geometry = render::geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        let tall_area = tall_geometry.pane_layout.panes[&application.workspace.focused];
        let tall_maximum = crate::editor::EditorView::maximum_viewport_line(
            application.documents.get(document_id),
            usize::from(tall_area.height.saturating_sub(2)),
        );
        let Pane::Editor { view, .. } = application.workspace.focused_pane() else {
            panic!("expected editor pane");
        };
        assert!(tall_maximum < short_maximum);
        assert_eq!(view.viewport_line, tall_maximum);
    }

    #[test]
    fn saving_a_modified_untitled_document_completes_pending_close() {
        let mut application = Application::new_for_test();
        application
            .apply(Command::Insert("saved".to_owned()))
            .unwrap();
        let document_id = application.focused_document_id().unwrap();
        let path = application.explorer.root.join("saved.txt");
        application.request_close().unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        let Some(Overlay::FileDialog { input, .. }) = application.overlay.as_mut() else {
            panic!("expected Save As dialog");
        };
        *input = path.display().to_string();
        application.submit_file_dialog().unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "saved");
        assert!(
            application
                .documents
                .iter()
                .all(|(id, _)| id != document_id)
        );
        assert!(matches!(application.workspace.focused_pane(), Pane::Empty));
    }

    #[test]
    fn saving_one_document_does_not_close_the_next_pristine_document() {
        let mut application = Application::new_for_test();
        let first_document = application.focused_document_id().unwrap();
        application
            .apply(Command::Insert("saved".to_owned()))
            .unwrap();
        let second_document = application.documents.new_untitled();
        application.switch_editor_document(application.workspace.focused, second_document);
        application.switch_editor_document(application.workspace.focused, first_document);
        let path = application.explorer.root.join("saved-first.txt");

        application.request_close().unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        let Some(Overlay::FileDialog { input, .. }) = application.overlay.as_mut() else {
            panic!("expected Save As dialog");
        };
        *input = path.display().to_string();
        application.submit_file_dialog().unwrap();

        assert_eq!(application.focused_document_id(), Some(second_document));
        assert!(
            application
                .documents
                .iter()
                .any(|(id, _)| id == second_document)
        );
        assert!(application.overlay.is_none());
    }

    #[test]
    fn saving_during_close_does_not_close_or_prompt_for_the_next_document() {
        let mut application = Application::new_for_test();
        let first_document = application.focused_document_id().unwrap();
        application
            .apply(Command::Insert("first".to_owned()))
            .unwrap();
        let second_document = application.documents.new_untitled();
        application.switch_editor_document(application.workspace.focused, second_document);
        application
            .apply(Command::Insert("second".to_owned()))
            .unwrap();
        application.switch_editor_document(application.workspace.focused, first_document);
        let path = application.explorer.root.join("first.txt");

        application.request_close().unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        let Some(Overlay::FileDialog { input, .. }) = application.overlay.as_mut() else {
            panic!("expected Save As dialog");
        };
        *input = path.display().to_string();
        application.submit_file_dialog().unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "first");
        assert_eq!(application.focused_document_id(), Some(second_document));
        assert!(application.overlay.is_none());
        assert_eq!(application.focus_owner, FocusOwner::Workspace);
        assert!(application.documents.get(second_document).is_modified());
    }

    #[test]
    fn save_as_existing_file_waits_for_overwrite_and_escape_preserves_both() {
        let mut application = Application::new_for_test();
        application
            .apply(Command::Insert("replacement".to_owned()))
            .unwrap();
        let document_id = application.focused_document_id().unwrap();
        let path = application.explorer.root.join("existing.txt");
        fs::write(&path, "existing").unwrap();

        application.apply(Command::SaveAs(path.clone())).unwrap();
        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmSaveAsOverwrite { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing");
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "existing");
        assert!(application.documents.get(document_id).is_modified());
        assert!(application.documents.get(document_id).path.is_none());
    }

    #[test]
    fn confirmed_save_as_overwrite_completes_pending_document_close() {
        let mut application = Application::new_for_test();
        application
            .apply(Command::Insert("replacement".to_owned()))
            .unwrap();
        let document_id = application.focused_document_id().unwrap();
        let path = application.explorer.root.join("existing.txt");
        fs::write(&path, "existing").unwrap();
        application.request_close().unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        let Some(Overlay::FileDialog { input, .. }) = application.overlay.as_mut() else {
            panic!("expected Save As dialog");
        };
        *input = path.display().to_string();
        application.submit_file_dialog().unwrap();
        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmSaveAsOverwrite { .. })
        ));
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('o'),
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "replacement");
        assert!(
            application
                .documents
                .iter()
                .all(|(id, _)| id != document_id)
        );
        assert!(matches!(application.workspace.focused_pane(), Pane::Empty));
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
    fn closing_a_file_in_a_split_shows_another_open_file_in_the_same_pane() {
        for direction in [SplitDirection::Vertical, SplitDirection::Horizontal] {
            let directory = tempfile::tempdir().unwrap();
            let first_path = directory.path().join("first.txt");
            let second_path = directory.path().join("second.txt");
            fs::write(&first_path, "first").unwrap();
            fs::write(&second_path, "second").unwrap();
            let mut application = application_with_startup_path(directory.path());
            application.apply(Command::Open(first_path)).unwrap();
            let first_document = application.focused_document_id().unwrap();
            application.apply(Command::Split(direction)).unwrap();
            let split_pane = application.workspace.focused;
            application.apply(Command::Open(second_path)).unwrap();
            let second_document = application.focused_document_id().unwrap();

            application
                .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Char('w'),
                    KeyModifiers::CONTROL,
                )))
                .unwrap();

            assert_eq!(application.workspace.panes.len(), 2);
            assert_eq!(application.workspace.focused, split_pane);
            assert_eq!(application.focused_document_id(), Some(first_document));
            assert_eq!(application.open_document_ids(), vec![first_document]);
            assert!(!application.open_document_ids().contains(&second_document));

            application.apply(Command::ClosePane).unwrap();
            assert_eq!(application.workspace.panes.len(), 1);
            assert_eq!(application.focused_document_id(), Some(first_document));
        }
    }

    #[test]
    fn closing_a_modified_split_document_waits_for_confirmation() {
        let mut application = Application::new_for_test();
        let first_document = application.focused_document_id().unwrap();
        application
            .apply(Command::Split(SplitDirection::Vertical))
            .unwrap();
        let split_pane = application.workspace.focused;
        let second_document = application.documents.new_untitled();
        application.switch_editor_document(split_pane, second_document);
        application
            .apply(Command::Insert("modified".to_owned()))
            .unwrap();

        application.apply(Command::ClosePane).unwrap();
        assert_eq!(application.overlay, Some(Overlay::ConfirmDocumentClose));
        assert_eq!(application.focused_document_id(), Some(second_document));
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert_eq!(application.workspace.panes.len(), 2);
        assert_eq!(application.focused_document_id(), Some(second_document));

        application.apply(Command::ClosePane).unwrap();
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert_eq!(application.workspace.panes.len(), 2);
        assert_eq!(application.workspace.focused, split_pane);
        assert_eq!(application.focused_document_id(), Some(first_document));
        assert_eq!(application.open_document_ids(), vec![first_document]);
    }

    #[test]
    fn closing_one_of_multiple_views_keeps_the_shared_modified_document() {
        let mut application = Application::new_for_test();
        let shared_document = application.focused_document_id().unwrap();
        application
            .apply(Command::Insert("modified".to_owned()))
            .unwrap();
        let other_document = application.documents.new_untitled();
        application
            .apply(Command::Split(SplitDirection::Vertical))
            .unwrap();
        let split_pane = application.workspace.focused;

        application.apply(Command::ClosePane).unwrap();

        assert!(application.overlay.is_none());
        assert_eq!(application.workspace.panes.len(), 2);
        assert_eq!(application.workspace.focused, split_pane);
        assert_eq!(application.focused_document_id(), Some(other_document));
        assert!(application.documents.get(shared_document).is_modified());
        assert!(application.open_document_ids().contains(&shared_document));
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
    fn left_arrow_keeps_explorer_focus_and_collapses_the_selected_folder() {
        let mut application = Application::new_for_test();
        let directory = application.explorer.root.join("nested");
        fs::create_dir(&directory).unwrap();
        application.explorer.refresh().unwrap();
        application.explorer.selected = application
            .explorer
            .entries()
            .iter()
            .position(|entry| entry.path == directory)
            .unwrap();
        application.explorer.toggle_selected().unwrap();
        application.focus_owner = FocusOwner::Explorer;

        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Left,
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert_eq!(application.focus_owner, FocusOwner::Explorer);
        assert!(!application.explorer.selected_entry().unwrap().expanded);
    }

    fn explorer_delete_key(application: &mut Application, code: KeyCode) {
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                code,
                KeyModifiers::NONE,
            )))
            .unwrap();
    }

    fn select_explorer_path(application: &mut Application, path: &Path) {
        application.explorer.refresh().unwrap();
        application.explorer.selected = application
            .explorer
            .entries()
            .iter()
            .position(|entry| entry.path == path)
            .unwrap();
        application.focus_owner = FocusOwner::Explorer;
    }

    #[test]
    fn explorer_delete_requires_entry_and_confirmation() {
        let mut application = Application::new_for_test();
        application.focus_owner = FocusOwner::Explorer;
        explorer_delete_key(&mut application, KeyCode::Delete);
        assert!(application.overlay.is_none());

        let path = application.explorer.root.join("file");
        fs::write(&path, "text").unwrap();
        select_explorer_path(&mut application, &path);
        for root_header in [true, false] {
            application.explorer.root_header_selected = root_header;
            application.explorer.open_editors_header_selected = !root_header;
            explorer_delete_key(&mut application, KeyCode::Delete);
            assert!(application.overlay.is_none());
            assert!(path.exists());
        }
        application.explorer.open_editors_header_selected = false;
        explorer_delete_key(&mut application, KeyCode::Delete);
        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmExplorerDelete { .. })
        ));
        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new_with_kind(
                KeyCode::Char('d'),
                KeyModifiers::NONE,
                crossterm::event::KeyEventKind::Repeat,
            )))
            .unwrap();
        assert!(path.exists());
        explorer_delete_key(&mut application, KeyCode::Esc);
        assert!(path.exists());
        assert!(application.overlay.is_none());
        assert_eq!(application.focus_owner, FocusOwner::Explorer);
    }

    #[test]
    fn explorer_backspace_opens_delete_confirmation() {
        let mut application = Application::new_for_test();
        let path = application.explorer.root.join("file");
        fs::write(&path, "text").unwrap();
        select_explorer_path(&mut application, &path);

        for key in [
            crossterm::event::KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL),
            crossterm::event::KeyEvent::new_with_kind(
                KeyCode::Backspace,
                KeyModifiers::NONE,
                crossterm::event::KeyEventKind::Repeat,
            ),
        ] {
            application.handle_event(Event::Key(key)).unwrap();
            assert!(application.overlay.is_none());
            assert!(path.exists());
        }
        explorer_delete_key(&mut application, KeyCode::Backspace);

        assert!(path.exists());
        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmExplorerDelete { .. })
        ));
        explorer_delete_key(&mut application, KeyCode::Char('d'));
        assert!(!path.exists());
    }

    #[test]
    fn editor_backspace_still_deletes_text() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .apply(Command::Insert("text".to_owned()))
            .unwrap();

        application
            .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Backspace,
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert_eq!(application.documents.get(document_id).text(), "tex");
        assert!(application.overlay.is_none());
    }

    #[test]
    fn explorer_delete_keeps_modified_document_and_reports_missing_file() {
        let mut application = Application::new_for_test();
        let path = application.explorer.root.join("file");
        fs::write(&path, "original").unwrap();
        let (document_id, _) = application.documents.open(&path).unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(8, " edited");
        select_explorer_path(&mut application, &path);
        explorer_delete_key(&mut application, KeyCode::Delete);
        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmExplorerDelete {
                modified_documents: 1,
                ..
            })
        ));

        explorer_delete_key(&mut application, KeyCode::Char('d'));
        assert!(!path.exists());
        assert_eq!(
            application.documents.get(document_id).text(),
            "original edited"
        );
        assert!(application.documents.get(document_id).is_modified());
        assert!(application.explorer.entries().is_empty());
        assert_eq!(application.focus_owner, FocusOwner::Explorer);

        let now = Instant::now();
        application.last_external_file_check = now - Duration::from_secs(2);
        assert!(application.refresh_document_lifecycle(now));
        assert!(matches!(
            application.overlay,
            Some(Overlay::ExternalFileChange {
                document_id: id,
                missing: true,
                ..
            }) if id == document_id
        ));
    }

    #[test]
    fn explorer_delete_recurses_into_hidden_and_excluded_contents() {
        let mut application = Application::new_for_test();
        let folder = application.explorer.root.join("folder");
        let nested = folder.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(".hidden"), "text").unwrap();
        fs::create_dir(folder.join(".git")).unwrap();
        fs::write(folder.join(".git").join("secret"), "text").unwrap();
        for path in [nested.join(".hidden"), folder.join(".git").join("secret")] {
            let (id, _) = application.documents.open(&path).unwrap();
            application.documents.get_mut(id).insert(4, " edited");
        }
        application
            .explorer
            .set_exclusions(&["**/.git".to_owned(), "**/.git/**".to_owned()])
            .unwrap();
        select_explorer_path(&mut application, &folder);
        application.explorer.toggle_selected().unwrap();
        select_explorer_path(&mut application, &nested);
        application.explorer.toggle_selected().unwrap();
        select_explorer_path(&mut application, &folder);

        explorer_delete_key(&mut application, KeyCode::Delete);
        assert!(matches!(
            application.overlay,
            Some(Overlay::ConfirmExplorerDelete {
                modified_documents: 2,
                ..
            })
        ));
        explorer_delete_key(&mut application, KeyCode::Char('d'));

        assert!(!folder.exists());
        assert!(application.explorer.entries().is_empty());
        fs::create_dir_all(&nested).unwrap();
        application.explorer.refresh().unwrap();
        assert!(!application.explorer.entries()[0].expanded);
        application.explorer.toggle_selected().unwrap();
        assert!(!application.explorer.entries()[1].expanded);
    }

    #[test]
    fn explorer_delete_symlinks_preserves_targets_and_does_not_warn_for_them() {
        use std::os::unix::fs::symlink;

        let mut application = Application::new_for_test();
        let external = tempfile::tempdir().unwrap();
        let file = external.path().join("file");
        fs::write(&file, "original").unwrap();
        let (document_id, _) = application.documents.open(&file).unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(8, " edited");
        for (name, target) in [
            ("file-link", &file),
            ("folder-link", &external.path().to_owned()),
        ] {
            let link = application.explorer.root.join(name);
            symlink(target, &link).unwrap();
            select_explorer_path(&mut application, &link);
            explorer_delete_key(&mut application, KeyCode::Delete);
            assert!(matches!(
                application.overlay,
                Some(Overlay::ConfirmExplorerDelete {
                    modified_documents: 0,
                    ..
                })
            ));
            explorer_delete_key(&mut application, KeyCode::Char('d'));
            assert!(fs::symlink_metadata(&link).is_err());
            assert_eq!(fs::read_to_string(&file).unwrap(), "original");
        }
        assert_eq!(
            application.documents.get(document_id).text(),
            "original edited"
        );
    }

    #[test]
    fn explorer_delete_rejects_replaced_or_missing_targets_and_refreshes() {
        for replace in [false, true] {
            let mut application = Application::new_for_test();
            let path = application.explorer.root.join("file");
            let old = application.explorer.root.join("old");
            fs::write(&path, "original").unwrap();
            select_explorer_path(&mut application, &path);
            explorer_delete_key(&mut application, KeyCode::Delete);
            fs::rename(&path, &old).unwrap();
            if replace {
                fs::write(&path, "replacement").unwrap();
            }
            explorer_delete_key(&mut application, KeyCode::Char('d'));
            assert!(matches!(
                application.overlay,
                Some(Overlay::ExplorerDeleteError { .. })
            ));
            assert_eq!(fs::read_to_string(&old).unwrap(), "original");
            if replace {
                assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
            }
            assert!(
                application
                    .explorer
                    .entries()
                    .iter()
                    .any(|entry| entry.path == old)
            );
            explorer_delete_key(&mut application, KeyCode::Esc);
            assert_eq!(application.focus_owner, FocusOwner::Explorer);
        }
    }

    #[test]
    fn explorer_delete_reports_deletion_and_refresh_failures() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let path = root.join("file");
        fs::write(&path, "text").unwrap();
        let mut application = Application::new_for_test();
        application.explorer = Explorer::new(root.clone(), true, &[]).unwrap();
        let path = application.explorer.root.join("file");
        select_explorer_path(&mut application, &path);
        explorer_delete_key(&mut application, KeyCode::Delete);
        fs::rename(&root, parent.path().join("moved")).unwrap();

        explorer_delete_key(&mut application, KeyCode::Char('d'));

        let Some(Overlay::ExplorerDeleteError { message }) = &application.overlay else {
            panic!("expected deletion error");
        };
        assert!(message.contains("could not inspect"));
        assert!(message.contains("Explorer refresh failed"));
        assert_eq!(
            fs::read_to_string(parent.path().join("moved/file")).unwrap(),
            "text"
        );
    }

    #[test]
    fn control_h_opens_help_from_workspace_and_terminal_focus() {
        for focus_owner in [FocusOwner::Workspace, FocusOwner::BottomTerminal] {
            let mut application = Application::new_for_test();
            application.focus_owner = focus_owner;
            application
                .handle_event(Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Char('h'),
                    KeyModifiers::CONTROL,
                )))
                .unwrap();

            assert_eq!(application.overlay, Some(Overlay::Help { scroll: 0 }));
            assert_eq!(application.focus_owner, FocusOwner::Overlay);
        }
    }

    #[test]
    fn title_control_actions_route_to_existing_semantic_commands() {
        assert_eq!(
            title_control_command(render::TitleControlAction::ToggleLeftPanel),
            Command::ToggleExplorer
        );
        assert_eq!(
            title_control_command(render::TitleControlAction::ToggleBottomPanel),
            Command::ToggleBottomTerminal
        );
        assert_eq!(
            title_control_command(render::TitleControlAction::OpenSettings),
            Command::OpenSettings
        );
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
    fn autosave_writes_a_recovery_record_for_modified_documents() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(0, "unsaved");
        let now = Instant::now();
        application.last_autosave = now - Duration::from_secs(30);

        application.refresh_document_lifecycle(now);

        let records = load_recovery_records(&application.recovery_directory).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1.text, "unsaved");
    }

    #[test]
    fn non_positive_autosave_intervals_disable_recovery_writes() {
        for interval in [0, -1] {
            let mut application = Application::new_for_test();
            let document_id = application.focused_document_id().unwrap();
            application
                .documents
                .get_mut(document_id)
                .insert(0, "unsaved");
            application
                .config
                .effective
                .editor
                .autosave_interval_seconds = interval;
            let now = Instant::now();
            application.last_autosave = now - Duration::from_secs(30);

            application.refresh_document_lifecycle(now);

            assert!(
                load_recovery_records(&application.recovery_directory)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn modified_external_changes_open_a_resolution_overlay() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "original").unwrap();
        let original_modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut application = Application::new_for_test();
        let (document_id, _) = application.documents.open(&path).unwrap();
        application.switch_editor_document(application.workspace.focused, document_id);
        application
            .documents
            .get_mut(document_id)
            .insert(8, " local");
        fs::write(&path, "external change").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(original_modified))
            .unwrap();
        let now = Instant::now();
        application.last_external_file_check = now - Duration::from_secs(2);

        assert!(application.refresh_document_lifecycle(now));
        assert!(matches!(
            application.overlay,
            Some(Overlay::ExternalFileChange {
                document_id: changed_id,
                missing: false,
                ..
            }) if changed_id == document_id
        ));
    }

    #[test]
    fn recovery_overlay_restores_discards_and_defers_records() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        let source_path = application.explorer.root.join("recovered.txt");
        application.documents.get_mut(document_id).path = Some(source_path.clone());
        application
            .documents
            .get_mut(document_id)
            .insert(0, "unsaved");
        let path = write_recovery_record(
            &application.recovery_directory,
            document_id,
            application.documents.get(document_id),
        )
        .unwrap();
        let record = load_recovery_records(&application.recovery_directory)
            .unwrap()
            .pop()
            .unwrap()
            .1;
        let deferred_record = RecoveryRecord {
            text: "deferred".to_owned(),
            ..record.clone()
        };

        application.overlay = Some(Overlay::Recovery {
            path: path.clone(),
            record,
        });
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert!(!path.exists());
        assert_eq!(
            application
                .documents
                .get(application.focused_document_id().unwrap())
                .text(),
            "unsaved"
        );
        assert_eq!(
            application
                .documents
                .get(application.focused_document_id().unwrap())
                .path,
            Some(source_path)
        );

        let deferred_path = application.recovery_directory.join("deferred.toml");
        fs::write(&deferred_path, toml::to_string(&deferred_record).unwrap()).unwrap();
        application.overlay = Some(Overlay::Recovery {
            path: deferred_path.clone(),
            record: deferred_record.clone(),
        });
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert!(deferred_path.exists());

        application.overlay = Some(Overlay::Recovery {
            path: deferred_path.clone(),
            record: deferred_record,
        });
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert!(!deferred_path.exists());

        let untitled_record = RecoveryRecord {
            format_version: 1,
            path: None,
            revision: 1,
            captured_at_unix_seconds: 1,
            encoding: crate::document::EncodingState::Utf8,
            text: "untitled".to_owned(),
        };
        application
            .restore_recovery_record(untitled_record)
            .unwrap();
        assert!(
            application
                .documents
                .get(application.focused_document_id().unwrap())
                .path
                .is_none()
        );
    }

    #[test]
    fn save_and_close_remove_recovery_records() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        application
            .documents
            .get_mut(document_id)
            .insert(0, "unsaved");
        write_recovery_record(
            &application.recovery_directory,
            document_id,
            application.documents.get(document_id),
        )
        .unwrap();

        application.finish_successful_save(document_id).unwrap();
        assert!(
            load_recovery_records(&application.recovery_directory)
                .unwrap()
                .is_empty()
        );

        write_recovery_record(
            &application.recovery_directory,
            document_id,
            application.documents.get(document_id),
        )
        .unwrap();
        application.close_focused_editor_view();
        assert!(
            load_recovery_records(&application.recovery_directory)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn pristine_external_changes_reload_automatically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "original").unwrap();
        let mut application = Application::new_for_test();
        let (document_id, _) = application.documents.open(&path).unwrap();
        application.switch_editor_document(application.workspace.focused, document_id);
        fs::write(&path, "external change").unwrap();
        let now = Instant::now();
        application.last_external_file_check = now - Duration::from_secs(2);

        assert!(application.refresh_document_lifecycle(now));
        assert_eq!(
            application.documents.get(document_id).text(),
            "external change"
        );
        assert!(application.overlay.is_none());
    }

    #[test]
    fn external_change_resolution_reloads_or_keeps_local_contents() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "original").unwrap();
        let mut application = Application::new_for_test();
        let (document_id, _) = application.documents.open(&path).unwrap();
        application.switch_editor_document(application.workspace.focused, document_id);
        application
            .documents
            .get_mut(document_id)
            .insert(8, " local");
        fs::write(&path, "external change").unwrap();
        let now = Instant::now();
        application.last_external_file_check = now - Duration::from_secs(2);
        application.refresh_document_lifecycle(now);
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('k'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert_eq!(
            application.documents.get(document_id).text(),
            "original local"
        );
        assert_eq!(
            application
                .documents
                .external_file_state(document_id)
                .unwrap(),
            ExternalFileState::Unchanged
        );

        fs::write(&path, "replacement").unwrap();
        application.last_external_file_check = now - Duration::from_secs(2);
        application.refresh_document_lifecycle(now);
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert_eq!(application.documents.get(document_id).text(), "replacement");
        assert!(!application.documents.get(document_id).is_modified());
    }

    #[test]
    fn deleted_file_acknowledgement_and_new_external_versions_are_reported() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, "original").unwrap();
        let mut application = Application::new_for_test();
        let (document_id, _) = application.documents.open(&path).unwrap();
        application.switch_editor_document(application.workspace.focused, document_id);
        application
            .documents
            .get_mut(document_id)
            .insert(8, " local");
        fs::write(&path, "first external").unwrap();
        let now = Instant::now();
        application.last_external_file_check = now - Duration::from_secs(2);
        application.refresh_document_lifecycle(now);
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();
        fs::write(&path, "second external").unwrap();
        application.last_external_file_check = now - Duration::from_secs(2);
        assert!(application.refresh_document_lifecycle(now));
        assert!(matches!(
            application.overlay,
            Some(Overlay::ExternalFileChange { .. })
        ));
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();

        fs::remove_file(&path).unwrap();
        application.last_external_file_check = now - Duration::from_secs(2);
        application.refresh_document_lifecycle(now);
        assert!(matches!(
            application.overlay,
            Some(Overlay::ExternalFileChange { missing: true, .. })
        ));
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::NONE,
            )))
            .unwrap();
        assert_eq!(
            application
                .documents
                .external_file_state(document_id)
                .unwrap(),
            ExternalFileState::Unchanged
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
        let explorer_divider = Some(Rect {
            x: 24,
            y: 2,
            width: 1,
            height: 10,
        });
        assert!(explorer_divider_contains(area, explorer_divider, 23, 5));
        assert!(explorer_divider_contains(area, explorer_divider, 24, 5));
        assert!(!explorer_divider_contains(area, explorer_divider, 25, 5));

        let terminal_area = Rect {
            x: 0,
            y: 2,
            width: 80,
            height: 10,
        };
        let terminal_divider = Some(Rect {
            x: 0,
            y: 1,
            width: 80,
            height: 1,
        });
        assert!(bottom_divider_contains(
            terminal_area,
            terminal_divider,
            5,
            2
        ));
        assert!(bottom_divider_contains(
            terminal_area,
            terminal_divider,
            5,
            1
        ));
        assert!(!bottom_divider_contains(
            terminal_area,
            terminal_divider,
            5,
            0
        ));
    }

    #[test]
    fn file_dialog_navigates_into_folders_and_opens_selected_files() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let file = nested.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let canonical_nested = fs::canonicalize(&nested).unwrap();
        let canonical_file = fs::canonicalize(&file).unwrap();
        let mut application = Application::new_for_test();
        application.overlay = Some(file_dialog(directory.path(), false, &application.explorer));
        application.activate_file_dialog_selection().unwrap();
        let Some(Overlay::FileDialog {
            directory: current,
            entries,
            ..
        }) = application.overlay.as_ref()
        else {
            panic!("expected file dialog");
        };
        assert_eq!(current, &canonical_nested);
        assert_eq!(entries, &vec![canonical_file.clone()]);
        application.activate_file_dialog_selection().unwrap();
        assert!(application.overlay.is_none());
        let document_id = application.focused_document_id().unwrap();
        assert_eq!(
            application.documents.get(document_id).path.as_deref(),
            Some(canonical_file.as_path())
        );
    }

    #[test]
    fn file_dialog_uses_explorer_exclusions_but_keeps_other_dotfiles() {
        let mut application = Application::new_for_test();
        fs::create_dir(application.explorer.root.join(".git")).unwrap();
        fs::write(application.explorer.root.join(".env"), "value").unwrap();

        application.overlay = Some(file_dialog(
            &application.explorer.root,
            false,
            &application.explorer,
        ));

        let Some(Overlay::FileDialog { entries, .. }) = application.overlay.as_ref() else {
            panic!("expected file dialog");
        };
        assert!(entries.iter().any(|entry| entry.ends_with(".env")));
        assert!(!entries.iter().any(|entry| entry.ends_with(".git")));
    }

    #[test]
    fn file_dialog_mouse_wheel_moves_the_selection() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..8 {
            fs::write(directory.path().join(format!("{index}.txt")), "x").unwrap();
        }
        let mut application = Application::new_for_test();
        application.overlay = Some(file_dialog(directory.path(), false, &application.explorer));
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
    fn file_dialog_uses_an_absolute_path_for_relative_directories() {
        let current_directory = env::current_dir().unwrap();
        let explorer = Explorer::new(current_directory.clone(), true, &[]).unwrap();
        let Overlay::FileDialog {
            directory, input, ..
        } = file_dialog(Path::new("."), false, &explorer)
        else {
            panic!("expected file dialog");
        };

        assert_eq!(directory, current_directory);
        assert_eq!(input, current_directory.display().to_string());
    }

    #[test]
    fn explorer_create_uses_a_selected_files_parent_directory() {
        let mut application = Application::new_for_test();
        let directory = application.explorer.root.join("nested");
        let file = directory.join("note.txt");
        fs::create_dir(&directory).unwrap();
        fs::write(&file, "text").unwrap();
        application.explorer.refresh().unwrap();
        application.explorer.selected = application
            .explorer
            .entries()
            .iter()
            .position(|entry| entry.path == directory)
            .unwrap();
        application.explorer.toggle_selected().unwrap();
        application.explorer.selected = application
            .explorer
            .entries()
            .iter()
            .position(|entry| entry.path == file)
            .unwrap();

        application.open_explorer_create(false);

        let Some(Overlay::ExplorerCreate {
            directory: destination,
            ..
        }) = application.overlay
        else {
            panic!("expected creation dialog");
        };
        assert_eq!(destination, directory);
    }

    #[test]
    fn explorer_create_rejects_names_that_escape_the_workspace() {
        let mut application = Application::new_for_test();
        application.open_explorer_create(false);
        let Some(Overlay::ExplorerCreate { name, .. }) = application.overlay.as_mut() else {
            panic!("expected creation dialog");
        };
        *name = "../outside.txt".to_owned();

        application.submit_explorer_create().unwrap();

        let Some(Overlay::ExplorerCreate { error, .. }) = application.overlay.as_ref() else {
            panic!("creation dialog should remain open");
        };
        assert_eq!(error.as_deref(), Some("Enter a single file or folder name"));
    }

    #[test]
    fn save_as_enter_submits_the_typed_filename() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("saved.txt");
        let mut application = Application::new_for_test();
        application
            .apply(Command::Insert("saved content".to_owned()))
            .unwrap();
        application.overlay = Some(file_dialog(directory.path(), true, &application.explorer));
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
        application
            .documents
            .save_as(document_id, &path, false)
            .unwrap();

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

    #[test]
    fn theme_setting_opens_picker_and_applies_the_selected_theme() {
        let mut application = Application::new_for_test();
        fs::create_dir_all(application.theme_registry.directory()).unwrap();
        fs::write(
            application.theme_registry.directory().join("ocean.toml"),
            "[meta]\nname = \"Ocean\"\n[colors]\naccent = \"#123456\"\n",
        )
        .unwrap();
        application.overlay = Some(Overlay::Settings {
            query: String::new(),
            scope: ConfigScope::User,
            selected: 0,
            editing: None,
            new_value: String::new(),
        });
        application.focus_owner = FocusOwner::Overlay;
        for character in "theme.active".chars() {
            application
                .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Char(character),
                    KeyModifiers::NONE,
                )))
                .unwrap();
        }
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )))
            .unwrap();
        let ocean_index = application
            .theme_registry
            .entries()
            .position(|(id, _)| id == "ocean")
            .unwrap();
        let Some(Overlay::ThemePicker { selected, .. }) = application.overlay.as_mut() else {
            panic!("expected theme picker");
        };
        *selected = ocean_index;
        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert_eq!(application.theme.id, "ocean");
        assert_eq!(
            application.theme.colors.accent,
            ratatui::style::Color::Rgb(0x12, 0x34, 0x56)
        );
        assert!(
            fs::read_to_string(&application.config.user_path)
                .unwrap()
                .contains("active = \"ocean\"")
        );
    }

    #[test]
    fn cancelling_the_theme_picker_does_not_change_the_active_theme() {
        let mut application = Application::new_for_test();
        application.overlay = Some(Overlay::ThemePicker {
            scope: ConfigScope::User,
            selected: 0,
        });
        application.focus_owner = FocusOwner::Overlay;

        application
            .handle_overlay_event(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            )))
            .unwrap();

        assert_eq!(application.theme.id, "default");
        assert!(application.overlay.is_none());
        assert!(!application.config.user_path.exists());
    }

    #[test]
    fn invalid_live_theme_reload_retains_the_previous_valid_theme() {
        let mut application = Application::new_for_test();
        fs::create_dir_all(application.theme_registry.directory()).unwrap();
        let path = application.theme_registry.directory().join("ocean.toml");
        fs::write(
            &path,
            "[meta]\nname = \"Ocean\"\n[colors]\naccent = \"#123456\"\n",
        )
        .unwrap();
        application.rescan_themes(false).unwrap();
        application
            .select_theme(ConfigScope::User, "ocean")
            .unwrap();
        fs::write(path, "[meta").unwrap();

        application.rescan_themes(true).unwrap();

        assert_eq!(application.theme.id, "ocean");
        assert!(
            application
                .message
                .as_deref()
                .is_some_and(|message| message.contains("previous valid version"))
        );
    }

    #[test]
    fn saving_a_user_theme_in_the_editor_reloads_it_immediately() {
        let mut application = Application::new_for_test();
        fs::create_dir_all(application.theme_registry.directory()).unwrap();
        let path = application.theme_registry.directory().join("ocean.toml");
        fs::write(
            &path,
            "[meta]\nname = \"Ocean\"\n[colors]\naccent = \"#123456\"\n",
        )
        .unwrap();
        application.rescan_themes(false).unwrap();
        application
            .select_theme(ConfigScope::User, "ocean")
            .unwrap();
        application.open_path(&path, None).unwrap();
        let document_id = application.focused_document_id().unwrap();
        let document = application.documents.get_mut(document_id);
        document.replace(
            0,
            document.len_chars(),
            "[meta]\nname = \"Ocean\"\n[colors]\naccent = \"#654321\"\n",
        );
        application.documents.save(document_id, false).unwrap();

        application.finish_successful_save(document_id).unwrap();

        assert_eq!(
            application.theme.colors.accent,
            ratatui::style::Color::Rgb(0x65, 0x43, 0x21)
        );
    }
}
