use std::path::Path;

use ratatui::{
    Frame,
    layout::{Alignment, Rect as TuiRect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{Application, FocusOwner, Overlay},
    document::Document,
    layout::{CalculatedLayout, PaneId, Rect},
    terminal_emulator::{TerminalCell, TerminalColor},
    workspace::Pane,
};

const APPLICATION_TITLE: &str = " TinLynk Terminal Editor";
const TITLE_CONTROL_WIDTH: u16 = 2;
const TITLE_CONTROL_HEIGHT: u16 = 1;
const TITLE_CONTROL_GAP: u16 = 1;
const TITLE_CONTROLS_WIDTH: u16 = TITLE_CONTROL_WIDTH * 4 + TITLE_CONTROL_GAP * 3;
const TITLE_CONTROLS_RIGHT_INSET: u16 = 1;
const EXPLORER_BORDER_WIDTH: u16 = 2;
const EXPLORER_SEPARATOR_WIDTH: u16 = 1;
const EXPLORER_ROOT_ACTION_GAP: usize = 1;
const MINIMUM_WORKSPACE_WIDTH: u16 = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TitleControlAction {
    ToggleLeftPanel,
    ToggleBottomPanel,
    OpenSettings,
}

impl TitleControlAction {
    pub fn status_hint(self) -> &'static str {
        match self {
            Self::ToggleLeftPanel => "Toggle Explorer",
            Self::ToggleBottomPanel => "Toggle Bottom Panel",
            Self::OpenSettings => "Open Settings",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TitleControls {
    pub left_panel_toggle: Option<Rect>,
    pub bottom_panel_toggle: Option<Rect>,
    pub right_panel_toggle: Option<Rect>,
    pub settings: Option<Rect>,
}

impl TitleControls {
    pub fn action_at(self, column: u16, row: u16) -> Option<TitleControlAction> {
        if self
            .left_panel_toggle
            .is_some_and(|area| area.contains(column, row))
        {
            Some(TitleControlAction::ToggleLeftPanel)
        } else if self
            .bottom_panel_toggle
            .is_some_and(|area| area.contains(column, row))
        {
            Some(TitleControlAction::ToggleBottomPanel)
        } else if self.settings.is_some_and(|area| area.contains(column, row)) {
            Some(TitleControlAction::OpenSettings)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorkbenchGeometry {
    pub title: Option<Rect>,
    pub title_controls: TitleControls,
    pub explorer: Option<Rect>,
    pub workspace: Rect,
    pub terminal: Option<Rect>,
    pub status: Rect,
    pub pane_layout: CalculatedLayout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExplorerRootAction { NewFile, NewFolder, Refresh }

pub fn explorer_root_action_at(area: Rect, open_editor_count: usize, expanded: bool, column: u16, row: u16) -> Option<ExplorerRootAction> {
    let root_row = explorer_rows(area, open_editor_count, expanded).root;
    if row != root_row { return None; }
    let right = area.x.saturating_add(area.width.saturating_sub(2));
    if column == right { Some(ExplorerRootAction::Refresh) }
    else if column == right.saturating_sub(2) { Some(ExplorerRootAction::NewFolder) }
    else if column == right.saturating_sub(4) { Some(ExplorerRootAction::NewFile) }
    else { None }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorContentGeometry {
    pub height: usize,
    pub horizontal_scrollbar_visible: bool,
    pub line_number_width: usize,
    pub scrollbar_visible: bool,
    pub width: usize,
}

pub fn editor_content_geometry(
    application: &Application,
    area: Rect,
    document: &Document,
) -> EditorContentGeometry {
    let height = usize::from(area.height.saturating_sub(2));
    let line_number_width = if application.config.effective.editor.line_numbers {
        document.len_lines().to_string().len().max(2) + 1
    } else {
        0
    };
    let inner_width = usize::from(area.width.saturating_sub(2));
    let width_without_scrollbar = inner_width.saturating_sub(line_number_width).max(1);
    let wrapped_overflow = application.config.effective.editor.soft_wrap
        && crate::editor::wrapped_positions(
            document,
            crate::editor::WrappedPosition::default(),
            width_without_scrollbar,
            height.saturating_add(1),
        )
        .len()
            > height;
    let scrollbar_visible = application.config.effective.editor.scrollbar
        && (document.len_lines() > height || wrapped_overflow)
        && area.width > 3
        && height > 0;
    let width = inner_width
        .saturating_sub(line_number_width)
        .saturating_sub(usize::from(scrollbar_visible))
        .max(1);
    let horizontal_scrollbar_visible = application.config.effective.editor.scrollbar
        && !application.config.effective.editor.soft_wrap
        && document.maximum_line_display_width() > width
        && area.width > 4;
    EditorContentGeometry {
        height,
        horizontal_scrollbar_visible,
        line_number_width,
        scrollbar_visible,
        width,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HorizontalScrollbarGeometry {
    pub maximum: usize,
    pub row: u16,
    pub thumb_width: u16,
    pub thumb_x: u16,
    pub track_width: u16,
    pub track_x: u16,
}

pub fn horizontal_scrollbar_geometry(
    area: Rect,
    content: EditorContentGeometry,
    document: &Document,
    viewport_column: usize,
) -> Option<HorizontalScrollbarGeometry> {
    if !content.horizontal_scrollbar_visible {
        return None;
    }
    let track_width = area.width.saturating_sub(3);
    if track_width == 0 || area.height == 0 {
        return None;
    }
    let extent = document.maximum_line_display_width().max(1);
    let visible = content.width.max(1);
    let maximum = extent.saturating_sub(visible);
    let track = usize::from(track_width);
    let thumb_width = visible
        .saturating_mul(track)
        .saturating_add(extent.saturating_sub(1))
        / extent;
    let thumb_width = thumb_width.clamp(1, track);
    let travel = track.saturating_sub(thumb_width);
    let thumb_column = viewport_column.min(maximum).saturating_mul(travel) / maximum.max(1);
    let track_x = area.x.saturating_add(1);
    Some(HorizontalScrollbarGeometry {
        maximum,
        row: area.y.saturating_add(area.height.saturating_sub(1)),
        thumb_width: u16::try_from(thumb_width).unwrap_or(u16::MAX),
        thumb_x: track_x.saturating_add(u16::try_from(thumb_column).unwrap_or(u16::MAX)),
        track_width,
        track_x,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExplorerRows {
    pub header: u16,
    pub open_editors_start: u16,
    pub separator: u16,
    pub root: u16,
    pub entries_start: u16,
}

pub fn explorer_rows(area: Rect, open_editor_count: usize, expanded: bool) -> ExplorerRows {
    let open_editor_rows = if expanded {
        u16::try_from(open_editor_count).unwrap_or(u16::MAX)
    } else {
        0
    };
    let header = area.y.saturating_add(1);
    let open_editors_start = header.saturating_add(1);
    let separator = open_editors_start.saturating_add(open_editor_rows);
    let root = separator.saturating_add(1);
    ExplorerRows {
        header,
        open_editors_start,
        separator,
        root,
        entries_start: root.saturating_add(1),
    }
}

pub fn editor_tab_at(
    application: &Application,
    pane_area: Rect,
    x: u16,
    y: u16,
) -> Option<crate::document::DocumentId> {
    if y != pane_area.y {
        return None;
    }
    if application.config.effective.editor.header_style != crate::config::EditorHeaderStyle::Tabs {
        return None;
    }
    let open_ids = application.open_document_ids();
    if open_ids.is_empty() {
        return None;
    }
    // Tabs are rendered in the title starting at pane_area.x + 1 (inside the top-left corner)
    let mut current_x = pane_area.x.saturating_add(1);
    for (index, doc_id) in open_ids.iter().enumerate() {
        if index > 0 {
            current_x = current_x.saturating_add(1); // divider "│"
        }
        let doc = application.documents.get(*doc_id);
        let name = doc
            .path
            .as_deref()
            .and_then(|path| path.file_name())
            .map_or("Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
        let modified = if doc.is_modified() {
            format!(" {}", application.theme.icons.modified)
        } else {
            String::new()
        };
        let label = format!(" {name}{modified} ");
        let tab_width = u16::try_from(UnicodeWidthStr::width(label.as_str())).unwrap_or(u16::MAX);
        let tab_end = current_x.saturating_add(tab_width);
        if x >= current_x && x < tab_end {
            return Some(*doc_id);
        }
        current_x = tab_end;
    }
    None
}

pub fn geometry(application: &Application, screen: Rect) -> WorkbenchGeometry {
    let status_height = u16::from(screen.height > 0);
    let show_title = screen.height >= 12;
    let title_height = if show_title { TITLE_CONTROL_HEIGHT } else { 0 };
    let mut content = Rect {
        x: screen.x,
        y: screen.y.saturating_add(title_height),
        width: screen.width,
        height: screen.height.saturating_sub(title_height + status_height),
    };
    let title = show_title.then_some(Rect {
        height: TITLE_CONTROL_HEIGHT,
        ..screen
    });
    let title_controls = title.map_or_else(TitleControls::default, title_controls);
    let status = Rect {
        x: screen.x,
        y: screen
            .y
            .saturating_add(screen.height.saturating_sub(status_height)),
        width: screen.width,
        height: status_height,
    };
    let explorer = (application.explorer.visible && screen.width >= 80).then(|| {
        let maximum_width = content.width.saturating_sub(
            MINIMUM_WORKSPACE_WIDTH.saturating_add(EXPLORER_SEPARATOR_WIDTH),
        );
        let width = application
            .config
            .effective
            .explorer
            .width
            .max(explorer_required_width(application))
            .min(maximum_width);
        let area = Rect { width, ..content };
        content.x = content.x.saturating_add(width.saturating_add(EXPLORER_SEPARATOR_WIDTH));
        content.width = content.width.saturating_sub(width.saturating_add(EXPLORER_SEPARATOR_WIDTH));
        area
    });
    let bottom_terminal = (application.bottom_terminal_visible && screen.height >= 24).then(|| {
        let height = application
            .config
            .effective
            .ui
            .bottom_terminal_height
            .min(content.height.saturating_sub(4));
        let area = Rect {
            x: content.x,
            y: content
                .y
                .saturating_add(content.height.saturating_sub(height)),
            width: content.width,
            height,
        };
        content.height = content.height.saturating_sub(height.saturating_add(1));
        area
    });
    let pane_layout = application.workspace.calculate(
        content,
        application.config.effective.workspace.minimum_pane_width,
        application.config.effective.workspace.minimum_pane_height,
    );
    WorkbenchGeometry {
        title,
        title_controls,
        explorer,
        workspace: content,
        terminal: bottom_terminal,
        status,
        pane_layout,
    }
}

fn explorer_actions(application: &Application) -> String {
    format!(
        "{} {} {}",
        application.theme.icons.explorer_new_file,
        application.theme.icons.explorer_new_folder,
        application.theme.icons.explorer_refresh,
    )
}

fn explorer_root_label(application: &Application) -> String {
    let root_name = application
        .explorer
        .root
        .file_name()
        .unwrap_or(application.explorer.root.as_os_str())
        .to_string_lossy();
    format!("{} {root_name}", application.theme.icons.folder_open)
}

fn explorer_required_width(application: &Application) -> u16 {
    let inner_width = explorer_root_label(application)
        .width()
        .saturating_add(EXPLORER_ROOT_ACTION_GAP)
        .saturating_add(explorer_actions(application).width());
    u16::try_from(inner_width)
        .unwrap_or(u16::MAX)
        .saturating_add(EXPLORER_BORDER_WIDTH)
}

fn truncate_display_width(value: &str, maximum_width: usize, overflow: &str) -> String {
    if value.width() <= maximum_width {
        return value.to_owned();
    }
    let available_width = maximum_width.saturating_sub(overflow.width());
    let mut width: usize = 0;
    let mut end: usize = 0;
    for grapheme in value.graphemes(true) {
        let grapheme_width = grapheme.width();
        if width.saturating_add(grapheme_width) > available_width {
            break;
        }
        width = width.saturating_add(grapheme_width);
        end = end.saturating_add(grapheme.len());
    }
    format!("{}{overflow}", &value[..end])
}

fn title_controls(area: Rect) -> TitleControls {
    let title_width = u16::try_from(APPLICATION_TITLE.width()).unwrap_or(u16::MAX);
    if area.width
        < title_width
            .saturating_add(TITLE_CONTROLS_WIDTH)
            .saturating_add(TITLE_CONTROLS_RIGHT_INSET)
    {
        return TitleControls::default();
    }
    let start = area.x.saturating_add(
        area.width
            .saturating_sub(TITLE_CONTROLS_WIDTH)
            .saturating_sub(TITLE_CONTROLS_RIGHT_INSET),
    );
    let control = |index: u16| Rect {
        x: start.saturating_add(index.saturating_mul(TITLE_CONTROL_WIDTH + TITLE_CONTROL_GAP)),
        y: area.y,
        width: TITLE_CONTROL_WIDTH,
        height: TITLE_CONTROL_HEIGHT,
    };
    TitleControls {
        left_panel_toggle: Some(control(0)),
        bottom_panel_toggle: Some(control(1)),
        right_panel_toggle: Some(control(2)),
        settings: Some(control(3)),
    }
}

pub fn draw(frame: &mut Frame<'_>, application: &Application) {
    let screen = from_tui(frame.area());
    let geometry = geometry(application, screen);
    frame.render_widget(
        Block::default().style(
            Style::default()
                .bg(application.theme.colors.background)
                .fg(application.theme.colors.foreground),
        ),
        frame.area(),
    );
    if let Some(area) = geometry.title {
        let title_style = Style::default()
            .bg(application.theme.colors.title_background)
            .fg(application.theme.colors.title_foreground);
        frame.render_widget(Block::default().style(title_style), to_tui(area));
        let title_row = Rect {
            y: area.y.saturating_add(area.height.saturating_sub(1) / 2),
            height: 1,
            ..area
        };
        frame.render_widget(
            Paragraph::new(APPLICATION_TITLE).style(title_style.add_modifier(Modifier::BOLD)),
            to_tui(title_row),
        );
        draw_title_controls(frame, application, geometry.title_controls);
    }
    if let Some(area) = geometry.explorer {
        draw_explorer(frame, application, area);
    }
    draw_workspace(frame, application, &geometry.pane_layout);
    for splitter in &geometry.pane_layout.splitters {
        frame.render_widget(
            Paragraph::new(" ").style(Style::default().bg(application.theme.colors.divider)),
            to_tui(splitter.rect),
        );
    }
    if let Some(area) = geometry.terminal {
        draw_bottom_terminal(frame, application, area);
    }
    draw_status(frame, application, geometry.status);
    draw_overlay(frame, application, screen);
}

fn draw_title_controls(frame: &mut Frame<'_>, application: &Application, controls: TitleControls) {
    let background = application.theme.colors.title_background;
    let available = Style::default()
        .fg(application.theme.colors.title_foreground)
        .bg(background)
        .add_modifier(Modifier::BOLD);
    let disabled = Style::default()
        .fg(application.theme.colors.disabled)
        .bg(background);
    for (area, label, style) in [
        (
            controls.left_panel_toggle,
            if application.explorer.visible {
                application.theme.icons.left_panel_visible.as_str()
            } else {
                application.theme.icons.left_panel_hidden.as_str()
            },
            available,
        ),
        (
            controls.bottom_panel_toggle,
            if application.bottom_terminal_visible {
                application.theme.icons.bottom_panel_visible.as_str()
            } else {
                application.theme.icons.bottom_panel_hidden.as_str()
            },
            available,
        ),
        (
            controls.right_panel_toggle,
            application.theme.icons.right_panel_hidden.as_str(),
            disabled,
        ),
        (
            controls.settings,
            application.theme.icons.settings.as_str(),
            available,
        ),
    ] {
        if let Some(area) = area {
            frame.render_widget(Block::default().style(style), to_tui(area));
            let label_row = Rect {
                y: area.y.saturating_add(area.height.saturating_sub(1) / 2),
                height: 1,
                ..area
            };
            frame.render_widget(
                Paragraph::new(label)
                    .style(style)
                    .alignment(Alignment::Center),
                to_tui(label_row),
            );
        }
    }
}

fn draw_explorer(frame: &mut Frame<'_>, application: &Application, area: Rect) {
    let header_style = if application.explorer.open_editors_header_selected
        && application.focus_owner == FocusOwner::Explorer
    {
        Style::default()
            .bg(application.theme.colors.selected_row_background)
            .fg(application.theme.colors.foreground)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(application.theme.colors.muted)
            .add_modifier(Modifier::BOLD)
    };
    let mut lines = vec![Line::styled(
        format!(
            "{} Open Editors  Ctrl+W closes",
            if application.explorer.open_editors_expanded {
                application.theme.icons.tree_expanded.as_str()
            } else {
                application.theme.icons.tree_collapsed.as_str()
            }
        ),
        header_style,
    )];
    if application.explorer.open_editors_expanded {
        for document_id in application.open_document_ids() {
            let document = application.documents.get(document_id);
            let name = document
                .path
                .as_deref()
                .and_then(|path| path.file_name())
                .map_or_else(|| "Untitled".into(), |name| name.to_string_lossy());
            let modified = if document.is_modified() {
                format!(" {}", application.theme.icons.modified)
            } else {
                String::new()
            };
            let style = if application.focused_document_id() == Some(document_id) {
                Style::default().bg(application.theme.colors.selected_row_background)
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!("  {name}{modified}"), style));
        }
    }
    lines.push(Line::raw(""));
    let actions = explorer_actions(application);
    let root_width = usize::from(area.width.saturating_sub(2));
    let root_label = truncate_display_width(
        &explorer_root_label(application),
        root_width.saturating_sub(
            actions
                .width()
                .saturating_add(EXPLORER_ROOT_ACTION_GAP),
        ),
        application.theme.icons.overflow.as_str(),
    );
    lines.push(Line::styled(
        format!("{root_label}{:>padding$}", actions, padding = root_width.saturating_sub(root_label.width())),
        Style::default()
            .fg(application.theme.colors.muted)
            .add_modifier(Modifier::BOLD),
    ));
    let entry_capacity = usize::from(
        area.height
            .saturating_sub(u16::try_from(lines.len().saturating_add(2)).unwrap_or(u16::MAX)),
    )
    .max(1);
    for (index, entry) in application
        .explorer
        .entries()
        .iter()
        .enumerate()
        .skip(application.explorer.viewport_row)
        .take(entry_capacity)
    {
        let (prefix, is_dir) = if entry.is_directory && !entry.is_symlink {
            if entry.expanded {
                (application.theme.icons.folder_open.as_str(), true)
            } else {
                (application.theme.icons.folder_closed.as_str(), true)
            }
        } else {
            ("", false)
        };
        let name = entry.path.file_name().map_or_else(
            || entry.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let style = if index == application.explorer.selected
            && application.focus_owner == FocusOwner::Explorer
        {
            Style::default().bg(application.theme.colors.selected_row_background)
        } else {
            Style::default()
        };
        let indent = "  ".repeat(entry.depth + 1);
        let line_text = if is_dir {
            format!("{indent}{prefix} {name}")
        } else {
            format!("{indent}  {name}")
        };
        lines.push(Line::styled(line_text, style));
    }
    let border = if application.focus_owner == FocusOwner::Explorer {
        application.theme.colors.active_border
    } else {
        application.theme.colors.inactive_border
    };
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((0, u16::try_from(application.explorer.viewport_column).unwrap_or(u16::MAX)))
            .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(application.theme.border_type())
                .border_style(Style::default().fg(border))
                .style(
                    Style::default()
                        .bg(application.theme.colors.background)
                        .fg(application.theme.colors.foreground),
                )
                .title(" Explorer "),
            ),
        to_tui(area),
    );
}

fn draw_workspace(frame: &mut Frame<'_>, application: &Application, layout: &CalculatedLayout) {
    for (pane_id, area) in &layout.panes {
        let Some(pane) = application.workspace.panes.get(pane_id) else {
            continue;
        };
        match pane {
            Pane::Empty => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(application.theme.border_type())
                    .border_style(Style::default().fg(application.theme.colors.inactive_border))
                    .style(
                        Style::default()
                            .bg(application.theme.colors.background)
                            .fg(application.theme.colors.foreground),
                    );
                frame.render_widget(Paragraph::new("").block(block), to_tui(*area));
            }
            Pane::Editor { document_id, view } => {
                let document = application.documents.get(*document_id);
                draw_editor(
                    frame,
                    application,
                    *pane_id,
                    *area,
                    document,
                    view,
                    *document_id,
                );
            }
        }
    }
}

fn editor_header_title<'a>(
    application: &Application,
    current_doc_id: crate::document::DocumentId,
) -> Line<'a> {
    match application.config.effective.editor.header_style {
        crate::config::EditorHeaderStyle::Tabs => {
            let mut spans = Vec::new();
            let open_ids = application.open_document_ids();
            if open_ids.is_empty() {
                let doc = application.documents.get(current_doc_id);
                let name = doc
                    .path
                    .as_deref()
                    .and_then(|path| path.file_name())
                    .map_or("Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
                let modified = if doc.is_modified() {
                    format!(" {}", application.theme.icons.modified)
                } else {
                    String::new()
                };
                spans.push(Span::styled(
                    format!(" {name}{modified} "),
                    Style::default()
                        .bg(application.theme.colors.tab_active_background)
                        .fg(application.theme.colors.tab_active_foreground)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                for (index, doc_id) in open_ids.iter().enumerate() {
                    if index > 0 {
                        spans.push(Span::styled(
                            "│",
                            Style::default().fg(application.theme.colors.tab_border),
                        ));
                    }
                    let doc = application.documents.get(*doc_id);
                    let name = doc
                        .path
                        .as_deref()
                        .and_then(|path| path.file_name())
                        .map_or("Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
                    let modified = if doc.is_modified() {
                        format!(" {}", application.theme.icons.modified)
                    } else {
                        String::new()
                    };
                    if *doc_id == current_doc_id {
                        spans.push(Span::styled(
                            format!(" {name}{modified} "),
                            Style::default()
                                .bg(application.theme.colors.tab_active_background)
                                .fg(application.theme.colors.tab_active_foreground)
                                .add_modifier(Modifier::BOLD),
                        ));
                    } else {
                        spans.push(Span::styled(
                            format!(" {name}{modified} "),
                            Style::default()
                                .bg(application.theme.colors.tab_inactive_background)
                                .fg(application.theme.colors.tab_inactive_foreground),
                        ));
                    }
                }
            }
            Line::from(spans)
        }
        crate::config::EditorHeaderStyle::Breadcrumbs => {
            let doc = application.documents.get(current_doc_id);
            let name = doc
                .path
                .as_deref()
                .and_then(|path| path.file_name())
                .map_or("Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
            let modified = if doc.is_modified() {
                format!(" {}", application.theme.icons.modified)
            } else {
                String::new()
            };
            let root_name = application
                .explorer
                .root
                .file_name()
                .unwrap_or(application.explorer.root.as_os_str())
                .to_string_lossy();
            Line::styled(
                format!(" {root_name} › {name}{modified} "),
                Style::default().fg(application.theme.colors.title_foreground),
            )
        }
    }
}

fn draw_editor(
    frame: &mut Frame<'_>,
    application: &Application,
    pane_id: PaneId,
    area: Rect,
    document: &Document,
    view: &crate::editor::EditorView,
    document_id: crate::document::DocumentId,
) {
    let title = editor_header_title(application, document_id);
    let content = editor_content_geometry(application, area, document);
    let inner_height = content.height;
    let viewport_line = view.clamped_viewport_line(document, inner_height);
    let line_number_width = content.line_number_width;
    let soft_wrap = application.config.effective.editor.soft_wrap;
    let scrollbar_visible = content.scrollbar_visible;
    let available_text_width = content.width;
    let viewport_column =
        view.viewport_column
            .min(crate::editor::EditorView::maximum_horizontal_offset(
                document,
                available_text_width,
            ));
    let wrap_width = available_text_width.max(1);
    let mut lines = Vec::with_capacity(inner_height);
    let selection = view.selection();
    let is_focused_editor = pane_id == application.workspace.focused
        && application.focus_owner == FocusOwner::Workspace;
    let cursor_line = document.char_to_line(view.cursor);
    let wrapped_viewport = crate::editor::WrappedPosition {
        line: view.viewport_line,
        segment: view.viewport_wrap_segment,
    };
    let wrapped_viewport =
        crate::editor::advance_wrapped_position(document, wrapped_viewport, wrap_width, 0);
    let visible_wrapped_rows = if soft_wrap {
        crate::editor::wrapped_rows(document, wrapped_viewport, wrap_width, inner_height)
    } else {
        Vec::new()
    };
    let cursor_wrapped = if soft_wrap {
        crate::editor::wrapped_position_for_cursor(document, view.cursor, wrap_width)
    } else {
        crate::editor::WrappedPosition::default()
    };
    if soft_wrap {
        for row in &visible_wrapped_rows {
            let position = row.position;
            let mut spans = Vec::new();
            if line_number_width > 0 {
                let number = if position.segment == 0 {
                    format!(
                        "{:>width$} ",
                        position.line + 1,
                        width = line_number_width - 1
                    )
                } else {
                    " ".repeat(line_number_width)
                };
                let is_active = is_focused_editor && position.line == cursor_wrapped.line;
                let num_style = if is_active {
                    Style::default()
                        .fg(application.theme.colors.line_number_active)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(application.theme.colors.muted)
                };
                spans.push(Span::styled(number, num_style));
            }
            spans.extend(styled_editor_text(
                &document.slice(row.start, row.end),
                row.start,
                selection.as_ref(),
                0,
                application.theme.colors.editor_selection,
                wrap_width,
            ));
            lines.push(Line::from(spans));
        }
    } else {
        for line_index in viewport_line..document.len_lines() {
            if lines.len() >= inner_height {
                break;
            }
            let end = document.line_content_end(line_index);
            let segment_start =
                crate::editor::char_at_display_column(document, line_index, viewport_column)
                    .min(end);
            let maximum_chars = usize::from(area.width).saturating_mul(4).max(1);
            let segment_end = segment_start.saturating_add(maximum_chars).min(end);
            let text = document.slice(segment_start, segment_end);
            let overflow = segment_end < end || text.width() > available_text_width;
            let text_width = available_text_width.saturating_sub(usize::from(overflow));
            let mut spans = Vec::new();
            if line_number_width > 0 {
                let is_active = is_focused_editor && line_index == cursor_line;
                let num_style = if is_active {
                    Style::default()
                        .fg(application.theme.colors.line_number_active)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(application.theme.colors.muted)
                };
                spans.push(Span::styled(
                    format!("{:>width$} ", line_index + 1, width = line_number_width - 1),
                    num_style,
                ));
            }
            spans.extend(styled_editor_text(
                &text,
                segment_start,
                selection.as_ref(),
                0,
                application.theme.colors.editor_selection,
                text_width,
            ));
            if overflow && available_text_width > 0 {
                let rendered_width = spans
                    .iter()
                    .skip(usize::from(line_number_width > 0))
                    .map(|span| span.content.width())
                    .sum::<usize>();
                let padding = text_width.saturating_sub(rendered_width);
                if padding > 0 {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
                spans.push(Span::styled(
                    application.theme.icons.overflow.clone(),
                    Style::default().fg(application.theme.colors.muted),
                ));
            }
            lines.push(Line::from(spans));
        }
    }
    let block = pane_block(application, pane_id, title);
    frame.render_widget(Paragraph::new(lines).block(block), to_tui(area));
    if scrollbar_visible {
        let current_wrapped_rows = if soft_wrap {
            crate::editor::wrapped_row_count(document, wrapped_viewport.line, wrap_width)
        } else {
            1
        };
        let estimated_rows = document
            .len_lines()
            .saturating_add(current_wrapped_rows.saturating_sub(1));
        let estimated_offset = viewport_line.saturating_add(if soft_wrap {
            wrapped_viewport.segment
        } else {
            0
        });
        let maximum = estimated_rows.saturating_sub(inner_height.max(1));
        let thumb_height = inner_height
            .saturating_mul(inner_height)
            .saturating_add(estimated_rows.saturating_sub(1))
            / estimated_rows.max(1);
        let thumb_height = thumb_height.clamp(1, inner_height);
        let thumb_travel = inner_height.saturating_sub(thumb_height);
        let thumb_row = estimated_offset.min(maximum).saturating_mul(thumb_travel) / maximum.max(1);
        let thumb = std::iter::repeat_n(application.theme.icons.scrollbar.as_str(), thumb_height)
            .collect::<Vec<_>>()
            .join("\n");
        frame.render_widget(
            Paragraph::new(thumb).style(
                Style::default()
                    .fg(application.theme.colors.scrollbar)
                    .bg(application.theme.colors.scrollbar),
            ),
            TuiRect::new(
                area.x.saturating_add(area.width.saturating_sub(2)),
                area.y
                    .saturating_add(1)
                    .saturating_add(u16::try_from(thumb_row).unwrap_or(u16::MAX)),
                1,
                u16::try_from(thumb_height).unwrap_or(u16::MAX),
            ),
        );
    }
    if let Some(scrollbar) = horizontal_scrollbar_geometry(area, content, document, viewport_column)
    {
        let thumb = std::iter::repeat_n(
            application.theme.icons.scrollbar_horizontal.as_str(),
            usize::from(scrollbar.thumb_width),
        )
        .collect::<Vec<_>>()
        .join("");
        frame.render_widget(
            Paragraph::new(thumb).style(Style::default().fg(application.theme.colors.scrollbar)),
            TuiRect::new(scrollbar.thumb_x, scrollbar.row, scrollbar.thumb_width, 1),
        );
    }
    if pane_id == application.workspace.focused
        && application.focus_owner == FocusOwner::Workspace
        && area.width > 2
        && area.height > 2
    {
        let line = document.char_to_line(view.cursor);
        let cursor_wrapped =
            crate::editor::wrapped_position_for_cursor(document, view.cursor, wrap_width);
        let row = if soft_wrap {
            visible_wrapped_rows
                .iter()
                .position(|row| row.position == cursor_wrapped)
                .unwrap_or(inner_height)
        } else {
            line.saturating_sub(viewport_line)
        };
        if row < inner_height {
            let display_column = crate::editor::display_column(document, line, view.cursor);
            let column = if soft_wrap {
                let cursor_row = visible_wrapped_rows[row];
                display_column
                    .saturating_sub(cursor_row.display_start)
                    .min(wrap_width.saturating_sub(1))
            } else {
                display_column.saturating_sub(viewport_column)
            }
            .saturating_add(line_number_width);
            let x = area
                .x
                .saturating_add(1)
                .saturating_add(u16::try_from(column).unwrap_or(u16::MAX));
            let y = area
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            let cursor_right = area
                .x
                .saturating_add(area.width.saturating_sub(1))
                .saturating_sub(u16::from(scrollbar_visible));
            if x < cursor_right && y < area.y.saturating_add(area.height.saturating_sub(1)) {
                frame.set_cursor_position((x, y));
            }
        }
    }
}

fn styled_editor_text(
    text: &str,
    line_start: usize,
    selection: Option<&std::ops::Range<usize>>,
    viewport_column: usize,
    selection_color: Color,
    maximum_width: usize,
) -> Vec<Span<'static>> {
    let mut display_column: usize = 0;
    let mut char_offset: usize = 0;
    let mut rendered_width: usize = 0;
    let mut spans = Vec::new();
    for grapheme in text.graphemes(true) {
        let width = grapheme.width();
        let grapheme_start = line_start + char_offset;
        char_offset += grapheme.chars().count();
        let grapheme_end = line_start + char_offset;
        let visible = display_column.saturating_add(width) > viewport_column;
        display_column = display_column.saturating_add(width);
        if !visible {
            continue;
        }
        if rendered_width.saturating_add(width) > maximum_width {
            break;
        }
        let selected =
            selection.is_some_and(|range| range.start < grapheme_end && range.end > grapheme_start);
        spans.push(Span::styled(
            grapheme.to_owned(),
            if selected {
                Style::default().bg(selection_color)
            } else {
                Style::default()
            },
        ));
        rendered_width = rendered_width.saturating_add(width);
    }
    spans
}

fn pane_block<'a>(application: &Application, pane_id: PaneId, title: Line<'a>) -> Block<'a> {
    let style = if application.focus_owner == FocusOwner::Workspace
        && pane_id == application.workspace.focused
    {
        Style::default().fg(application.theme.colors.active_border)
    } else {
        Style::default().fg(application.theme.colors.inactive_border)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(application.theme.border_type())
        .border_style(style)
        .style(
            Style::default()
                .bg(application.theme.colors.background)
                .fg(application.theme.colors.foreground),
        )
        .title(title)
}

fn draw_bottom_terminal(frame: &mut Frame<'_>, application: &Application, area: Rect) {
    let lines = application.bottom_terminal.as_ref().map_or_else(
        || vec![Line::raw("Terminal is not running")],
        |session| {
            terminal_lines(
                session.visible_cells(usize::from(area.height.saturating_sub(2))),
                session.selection(),
                application.theme.colors.terminal_selection_background,
                application.theme.colors.terminal_selection_foreground,
            )
        },
    );
    let style = if application.focus_owner == FocusOwner::BottomTerminal {
        Style::default().fg(application.theme.colors.active_border)
    } else {
        Style::default().fg(application.theme.colors.inactive_border)
    };
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(application.theme.border_type())
                .border_style(style)
                .title(" Terminal "),
        ),
        to_tui(area),
    );
    if application.focus_owner == FocusOwner::BottomTerminal
        && let Some(session) = application.bottom_terminal.as_ref()
        && session.at_live_bottom()
    {
        let (row, column) = session.cursor();
        frame.set_cursor_position((
            area.x.saturating_add(1).saturating_add(column),
            area.y.saturating_add(1).saturating_add(row),
        ));
    }
}

fn terminal_lines(
    rows: Vec<Vec<TerminalCell>>,
    selection: Option<((usize, usize), (usize, usize))>,
    selection_background: Color,
    selection_foreground: Color,
) -> Vec<Line<'static>> {
    let selection = selection.map(|(first, second)| {
        if first <= second {
            (first, second)
        } else {
            (second, first)
        }
    });
    rows.into_iter()
        .enumerate()
        .map(|(row_index, row)| {
            Line::from(
                row.into_iter()
                    .enumerate()
                    .filter(|(_, cell)| !cell.continuation)
                    .map(|(column_index, cell)| {
                        let (foreground, background) = if cell.inverse {
                            (cell.background, cell.foreground)
                        } else {
                            (cell.foreground, cell.background)
                        };
                        let mut style = Style::default()
                            .fg(terminal_color(foreground))
                            .bg(terminal_color(background));
                        if cell.bold {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        if selection.is_some_and(|(start, end)| {
                            (row_index, column_index) >= start && (row_index, column_index) <= end
                        }) {
                            style = style.bg(selection_background).fg(selection_foreground);
                        }
                        Span::styled(cell.character.to_string(), style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn terminal_color(color: TerminalColor) -> Color {
    match color {
        TerminalColor::Default => Color::Reset,
        TerminalColor::Indexed(index) => Color::Indexed(index),
        TerminalColor::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}

fn draw_status(frame: &mut Frame<'_>, application: &Application, area: Rect) {
    let mut spans = Vec::new();
    let (mode_text, mode_bg, mode_fg) = match application.focus_owner {
        FocusOwner::Workspace => (
            " EDIT ",
            application.theme.colors.status_mode_background,
            application.theme.colors.status_mode_foreground,
        ),
        FocusOwner::BottomTerminal => (
            " TERM ",
            application.theme.colors.status_mode_background,
            application.theme.colors.status_mode_foreground,
        ),
        FocusOwner::Explorer => (
            " EXPLORER ",
            application.theme.colors.status_mode_background,
            application.theme.colors.status_mode_foreground,
        ),
        FocusOwner::Overlay => (
            " DIALOG ",
            application.theme.colors.status_mode_background,
            application.theme.colors.status_mode_foreground,
        ),
    };
    spans.push(Span::styled(
        mode_text,
        Style::default()
            .bg(mode_bg)
            .fg(mode_fg)
            .add_modifier(Modifier::BOLD),
    ));

    if let Some(branch) = application.git_branch() {
        let icon = &application.theme.icons.git_branch;
        let branch_label = if icon.is_empty() {
            format!(" {branch} ")
        } else {
            format!(" {icon} {branch} ")
        };
        spans.push(Span::styled(
            branch_label,
            Style::default()
                .bg(application.theme.colors.status_segment_background)
                .fg(application.theme.colors.status_segment_foreground),
        ));
    }

    let status = application.status_text();
    spans.push(Span::styled(
        format!(" {status} "),
        Style::default()
            .bg(application.theme.colors.status_background)
            .fg(application.theme.colors.status_foreground),
    ));

    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).style(
            Style::default()
                .bg(application.theme.colors.status_background)
                .fg(application.theme.colors.status_foreground),
        ),
        to_tui(area),
    );
}

fn draw_overlay(frame: &mut Frame<'_>, application: &Application, screen: Rect) {
    let Some(overlay) = &application.overlay else {
        return;
    };
    let area = overlay_area(screen, overlay);
    let (title, body) = match overlay {
        Overlay::FileDialog {
            input,
            entries,
            selected,
            save_as,
            error,
            ..
        } => {
            let visible_entries = file_dialog_visible_rows(area);
            let start = file_dialog_view_start(*selected, entries.len(), visible_entries);
            let listing = entries
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_entries)
                .map(|(index, path)| {
                    let suffix = if path.is_dir() { "/" } else { "" };
                    format!(
                        "{} {}{suffix}",
                        if index == *selected { ">" } else { " " },
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                if *save_as {
                    " Save As "
                } else {
                    " Open File "
                },
                Text::from(format!(
                    "Path: {input}\n{listing}\n{}",
                    error.as_deref().unwrap_or(if *save_as {
                        "Type a filename and press Enter to save, Right opens the selected folder, Left goes up, and Esc cancels."
                    } else {
                        "Up/Down selects, Enter/Right opens, Left goes up, Ctrl+Enter submits the path, and Esc cancels."
                    })
                )),
            )
        }
        Overlay::ExplorerCreate {
            directory,
            name,
            entries,
            selected,
            create_directory,
            error,
        } => {
            let path = explorer_create_path_label(&application.explorer.root, directory);
            let listing = entries
                .iter()
                .enumerate()
                .take(8)
                .map(|(index, entry)| format!("{} {}/", if index == *selected { ">" } else { " " }, entry.file_name().unwrap_or_default().to_string_lossy()))
                .collect::<Vec<_>>()
                .join("\n");
            (
                if *create_directory { " New Folder " } else { " New File " },
                Text::from(format!(
                    "Path: {path}\nName: {name}\n\n{listing}\n\nEnter creates, Right enters a folder, Left goes up, and Esc cancels.{}",
                    error.as_deref().map_or(String::new(), |message| format!("\n{message}"))
                )),
            )
        }
        Overlay::Settings {
            query,
            scope,
            selected,
            editing,
            new_value,
        } => {
            let normalized_query = query.to_lowercase();
            let defaults = crate::config::Config::default();
            let matching = crate::app::SETTINGS_KEYS
                .iter()
                .enumerate()
                .filter(|(_, key)| key.to_lowercase().contains(&normalized_query))
                .collect::<Vec<_>>();
            let selected_position = matching
                .iter()
                .position(|(index, _)| index == selected)
                .unwrap_or(0);
            let start = selected_position.saturating_sub(10).min(matching.len().saturating_sub(12));
            let listing = matching
                .into_iter()
                .skip(start)
                .take(12)
                .map(|(index, key)| {
                    let source = application
                        .config
                        .provenance
                        .get(key.split('.').next().unwrap_or(key))
                        .copied()
                        .unwrap_or(crate::config::ConfigScope::Default);
                    let value = if *editing == Some(index) {
                        let choices = setting_choice_hint(key);
                        if choices.is_empty() {
                            new_value.clone()
                        } else {
                            format!("{new_value} [{choices}]")
                        }
                    } else {
                        application.config.effective.get_setting_value(key)
                    };
                    format!(
                        "{} {key} = {} (default {}, {source:?})",
                        if *selected == index { ">" } else { " " },
                        value,
                        defaults.get_setting_value(key),
                    )
                })
                .collect::<Vec<_>>();
            let search = if query.is_empty() {
                Line::from(vec![
                    Span::raw(format!(
                        "Scope: {scope:?}  {} Search: ",
                        application.theme.icons.search
                    )),
                    Span::styled(
                        "type to search",
                        Style::default()
                            .fg(application.theme.colors.muted)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ])
            } else {
                Line::raw(format!(
                    "Scope: {scope:?}  {} Search: {query}",
                    application.theme.icons.search
                ))
            };
            let mut lines = vec![search, Line::raw("")];
            lines.extend(listing.into_iter().map(Line::raw));
            lines.extend([
                Line::raw(""),
                Line::raw(
                    "Up/Down selects, Ctrl+R resets, Tab changes scope, Enter to edit, Ctrl+Enter saves, Esc cancels or closes.",
                ),
            ]);
            (" Settings ", Text::from(lines))
        }
        Overlay::ThemePicker { scope, selected } => {
            let entries = application.theme_registry.entries().collect::<Vec<_>>();
            let start = selected.saturating_sub(10).min(entries.len().saturating_sub(12));
            let listing = entries
                .iter()
                .enumerate()
                .skip(start)
                .take(12)
                .map(|(index, (id, entry))| {
                    let source = match &entry.source {
                        crate::theme::ThemeSource::Embedded => "embedded".to_owned(),
                        crate::theme::ThemeSource::User(path) => path.display().to_string(),
                    };
                    let diagnostic = entry
                        .diagnostics
                        .first()
                        .map_or(String::new(), |message| format!(" - {message}"));
                    format!(
                        "{} {id}: {} ({source}){diagnostic}",
                        if *selected == index { ">" } else { " " },
                        entry.theme.meta.name,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let discovery = application
                .theme_registry
                .diagnostics()
                .first()
                .map_or(String::new(), |diagnostic| {
                    let path = diagnostic
                        .path
                        .as_deref()
                        .map_or(String::new(), |path| format!("{}: ", path.display()));
                    format!("\nDiagnostic: {path}{}", diagnostic.message)
                });
            (
                " Themes ",
                Text::from(format!(
                    "Scope: {scope:?}  Active: {}\n\n{listing}{discovery}\n\nUp/Down selects, Enter applies, R rescans, and Esc cancels.",
                    application.theme.id
                )),
            )
        }
        Overlay::Help { .. } => {
            (
                " Help ",
                Text::from(help_text(application)),
            )
        }
        Overlay::Message { title, message } => (title.as_str(), Text::from(message.clone())),
        Overlay::ConfirmQuit => (
            " Unsaved Changes ",
            Text::from("Modified documents remain. Press Ctrl+Q again to discard and exit, or Esc to cancel."),
        ),
        Overlay::ConfirmTerminateAll => (
            " Live Terminals ",
            Text::from("Live terminal processes remain. Press T to terminate them and exit, or Esc to cancel."),
        ),
        Overlay::ConfirmTerminalClose => (
            " Live Terminal ",
            Text::from("Press T to terminate the live terminal process, or Esc to cancel."),
        ),
        Overlay::ConfirmDocumentClose => (
            " Unsaved Document ",
            Text::from("Press S to save, D to discard, or Esc to cancel."),
        ),
        Overlay::SaveConflict => (
            " File Changed Externally ",
            Text::from("Press O to overwrite the file, or Esc to cancel. Save As is available from the file dialog."),
        ),
        Overlay::ConfirmSaveAsOverwrite { path, .. } => (
            " File Already Exists ",
            Text::from(format!(
                "{} already exists. Press O to overwrite it, or Esc to cancel.",
                path.display()
            )),
        ),
    };
    frame.render_widget(Clear, to_tui(area));
    let paragraph = Paragraph::new(body)
        .style(
            Style::default()
                .bg(application.theme.colors.overlay_surface)
                .fg(application.theme.colors.foreground),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(application.theme.border_type())
                .border_style(Style::default().fg(application.theme.colors.active_border))
                .style(
                    Style::default()
                        .bg(application.theme.colors.overlay_surface)
                        .fg(application.theme.colors.foreground),
                )
                .title(title),
        );
    let paragraph = if matches!(overlay, Overlay::FileDialog { .. }) {
        paragraph
    } else {
        paragraph.wrap(Wrap { trim: false })
    };
    let paragraph = if let Overlay::Help { scroll } = overlay {
        paragraph.scroll((u16::try_from(*scroll).unwrap_or(u16::MAX), 0))
    } else {
        paragraph
    };
    frame.render_widget(paragraph, to_tui(area));
}

pub fn help_line_count(application: &Application) -> usize {
    help_text(application).lines().count()
}

fn help_text(application: &Application) -> String {
    let bindings = application
        .config
        .effective
        .keybindings
        .iter()
        .filter(|(shortcut, _)| {
            !matches!(shortcut.as_str(), "ctrl+c" | "ctrl+x" | "ctrl+v" | "ctrl+g")
        })
        .map(|(shortcut, command)| format!("  {shortcut:<16} {command}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Keybindings\n\n{bindings}\n\nNavigation\n  Ctrl+H           Help\n  Ctrl+G           Settings\n  Esc              Close dialog\n  Arrow keys       Move cursor\n  Home / End       Start / end of line\n  Page Up / Down   Move by page\n\nEditing\n  Ctrl+C           Copy selection\n  Ctrl+X           Cut selection\n  Ctrl+V           Paste\n  Enter            New line\n  Backspace        Delete backward\n  Delete           Delete forward\n  Tab              Indent\n  Shift+Tab        Unindent"
    )
}

fn explorer_create_path_label(workspace_root: &Path, directory: &Path) -> String {
    let workspace_name = workspace_root
        .file_name()
        .unwrap_or(workspace_root.as_os_str())
        .to_string_lossy();
    let relative = directory.strip_prefix(workspace_root).unwrap_or(Path::new(""));
    if relative.as_os_str().is_empty() {
        format!("{workspace_name}/")
    } else {
        format!("{workspace_name}/{}/", relative.display())
    }
}

fn setting_choice_hint(key: &str) -> &'static str {
    match key {
        "editor.insert_spaces" | "editor.line_numbers" | "editor.scrollbar"
        | "editor.soft_wrap" | "explorer.visible" | "mouse.enabled" | "clipboard.osc52"
        | "terminal.allow_child_osc52_write" | "ui.bottom_terminal_visible" => "true | false",
        "editor.default_line_ending" => "lf | crlf",
        "editor.header_style" => "tabs | breadcrumbs",
        _ => "",
    }
}

pub fn overlay_area(screen: Rect, overlay: &Overlay) -> Rect {
    let width = screen.width.saturating_sub(4).clamp(1, 70);
    let height = match overlay {
        Overlay::Settings { .. } | Overlay::ThemePicker { .. } | Overlay::Help { .. } => {
            screen.height.saturating_sub(4).clamp(3, 18)
        }
        Overlay::FileDialog { .. } | Overlay::ExplorerCreate { .. } => screen
            .height
            .saturating_sub(4)
            .clamp(7, 20)
            .min(screen.height),
        _ => 7.min(screen.height),
    };
    Rect {
        x: screen
            .x
            .saturating_add(screen.width.saturating_sub(width) / 2),
        y: screen
            .y
            .saturating_add(screen.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

pub fn file_dialog_visible_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(5)).max(1)
}

pub fn file_dialog_view_start(selected: usize, entry_count: usize, visible: usize) -> usize {
    selected
        .saturating_add(1)
        .saturating_sub(visible)
        .min(entry_count.saturating_sub(visible))
}

fn to_tui(rect: Rect) -> TuiRect {
    TuiRect::new(rect.x, rect.y, rect.width, rect.height)
}

fn from_tui(rect: TuiRect) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::Application;

    #[test]
    fn explorer_creation_path_includes_workspace_name() {
        let root = Path::new("/workspace/terminal-editor");

        assert_eq!(
            explorer_create_path_label(root, root),
            "terminal-editor/"
        );
        assert_eq!(
            explorer_create_path_label(root, &root.join("src/app")),
            "terminal-editor/src/app/"
        );
    }

    #[test]
    fn responsive_regions_collapse_in_expected_order() {
        let mut application = Application::new_for_test();
        application.bottom_terminal_visible = true;
        let wide = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        assert!(wide.explorer.is_some());
        assert!(wide.terminal.is_some());
        assert!(wide.title.is_some());
        let compact = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 12,
            },
        );
        assert!(compact.explorer.is_none());
        assert!(compact.terminal.is_none());
        assert!(compact.title.is_some());
        let tiny = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 5,
            },
        );
        assert!(tiny.title.is_none());
        assert_eq!(tiny.status.height, 1);
    }

    #[test]
    fn explorer_row_model_includes_the_section_separator() {
        let area = Rect {
            x: 0,
            y: 1,
            width: 24,
            height: 20,
        };
        assert_eq!(
            explorer_rows(area, 2, true),
            ExplorerRows {
                header: 2,
                open_editors_start: 3,
                separator: 5,
                root: 6,
                entries_start: 7,
            }
        );
        assert_eq!(explorer_rows(area, 2, false).entries_start, 5);
    }

    #[test]
    fn explorer_root_action_hit_targets_match_compact_icon_positions() {
        let area = Rect {
            x: 4,
            y: 2,
            width: 24,
            height: 20,
        };
        let row = explorer_rows(area, 0, false).root;
        let right = area.x.saturating_add(area.width.saturating_sub(2));

        assert_eq!(
            explorer_root_action_at(area, 0, false, right.saturating_sub(4), row),
            Some(ExplorerRootAction::NewFile)
        );
        assert_eq!(
            explorer_root_action_at(area, 0, false, right.saturating_sub(2), row),
            Some(ExplorerRootAction::NewFolder)
        );
        assert_eq!(
            explorer_root_action_at(area, 0, false, right, row),
            Some(ExplorerRootAction::Refresh)
        );
        assert_eq!(
            explorer_root_action_at(area, 0, false, right.saturating_sub(3), row),
            None
        );
    }

    #[test]
    fn explorer_width_expands_for_workspace_name_and_actions() {
        let mut application = Application::new_for_test();
        application.explorer.root = PathBuf::from("workspace-with-a-long-name");
        let geometry = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        let explorer = geometry.explorer.unwrap();

        assert!(explorer.width > application.config.effective.explorer.width);
        assert_eq!(explorer.width, explorer_required_width(&application));
        assert!(geometry.workspace.width >= MINIMUM_WORKSPACE_WIDTH);
    }

    #[test]
    fn title_controls_are_right_aligned_and_the_right_placeholder_is_inert() {
        let application = Application::new_for_test();
        let geometry = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        let controls = geometry.title_controls;
        let explorer = controls.left_panel_toggle.unwrap();
        let bottom = controls.bottom_panel_toggle.unwrap();
        let right = controls.right_panel_toggle.unwrap();
        let settings = controls.settings.unwrap();

        assert_eq!(settings.x.saturating_add(settings.width), 99);
        assert_eq!(bottom.x, explorer.x + explorer.width + TITLE_CONTROL_GAP);
        assert_eq!(right.x, bottom.x + bottom.width + TITLE_CONTROL_GAP);
        assert_eq!(settings.x, right.x + right.width + TITLE_CONTROL_GAP);
        assert_eq!(
            controls.action_at(explorer.x + 1, explorer.y),
            Some(TitleControlAction::ToggleLeftPanel)
        );
        assert_eq!(
            controls.action_at(bottom.x + 1, bottom.y),
            Some(TitleControlAction::ToggleBottomPanel)
        );
        assert_eq!(controls.action_at(right.x + 1, right.y), None);
        assert_eq!(
            controls.action_at(settings.x + 1, settings.y),
            Some(TitleControlAction::OpenSettings)
        );
        for icon in [
            application.theme.icons.left_panel_visible.as_str(),
            application.theme.icons.left_panel_hidden.as_str(),
            application.theme.icons.bottom_panel_visible.as_str(),
            application.theme.icons.bottom_panel_hidden.as_str(),
            application.theme.icons.right_panel_visible.as_str(),
            application.theme.icons.right_panel_hidden.as_str(),
            application.theme.icons.settings.as_str(),
        ] {
            assert!(icon.width() <= usize::from(TITLE_CONTROL_WIDTH));
        }
    }

    #[test]
    fn title_controls_hide_before_overlapping_the_application_title() {
        let application = Application::new_for_test();
        let width = u16::try_from(APPLICATION_TITLE.width())
            .unwrap()
            .saturating_add(TITLE_CONTROLS_WIDTH)
            .saturating_add(TITLE_CONTROLS_RIGHT_INSET)
            .saturating_sub(1);
        let controls = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width,
                height: 10,
            },
        )
        .title_controls;

        assert_eq!(controls, TitleControls::default());
    }

    #[test]
    fn title_controls_render_visible_hidden_disabled_and_stateless_settings() {
        let mut application = Application::new_for_test();
        application.bottom_terminal_visible = false;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let controls = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        )
        .title_controls;
        let buffer = terminal.backend().buffer();
        let explorer = controls.left_panel_toggle.unwrap();
        let bottom = controls.bottom_panel_toggle.unwrap();
        let right = controls.right_panel_toggle.unwrap();
        let settings = controls.settings.unwrap();

        let explorer_cell = buffer.cell((explorer.x + 1, explorer.y)).unwrap();
        assert_eq!(
            explorer_cell.symbol(),
            application.theme.icons.left_panel_visible
        );
        assert_eq!(explorer_cell.fg, application.theme.colors.title_foreground);
        assert_eq!(explorer_cell.bg, application.theme.colors.title_background);
        assert!(explorer_cell.modifier.contains(Modifier::BOLD));
        let bottom_cell = buffer.cell((bottom.x + 1, bottom.y)).unwrap();
        assert_eq!(
            bottom_cell.symbol(),
            application.theme.icons.bottom_panel_hidden
        );
        assert_eq!(bottom_cell.fg, application.theme.colors.title_foreground);
        assert_eq!(bottom_cell.bg, application.theme.colors.title_background);
        assert_eq!(
            buffer.cell((right.x + 1, right.y)).unwrap().fg,
            Color::DarkGray
        );
        assert_eq!(
            buffer.cell((right.x + 1, right.y)).unwrap().symbol(),
            application.theme.icons.right_panel_hidden
        );
        assert!((settings.x..settings.x + settings.width).any(|column| {
            buffer.cell((column, settings.y)).unwrap().symbol() == application.theme.icons.settings
        }));

        application.explorer.visible = false;
        application.bottom_terminal_visible = true;
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer.cell((explorer.x + 1, explorer.y)).unwrap().symbol(),
            application.theme.icons.left_panel_hidden
        );
        assert_eq!(
            buffer.cell((bottom.x + 1, bottom.y)).unwrap().symbol(),
            application.theme.icons.bottom_panel_visible
        );

        application.overlay = Some(Overlay::Settings {
            query: String::new(),
            scope: crate::config::ConfigScope::User,
            selected: 0,
            editing: None,
            new_value: String::new(),
        });
        application.focus_owner = FocusOwner::Overlay;
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let settings_cell = terminal
            .backend()
            .buffer()
            .cell((settings.x + 1, settings.y))
            .unwrap();
        assert_eq!(settings_cell.bg, application.theme.colors.title_background);
        assert_eq!(settings_cell.fg, application.theme.colors.title_foreground);
    }

    #[test]
    fn title_controls_use_active_theme_colors_and_icons() {
        let mut application = Application::new_for_test();
        application.theme.colors.title_background = Color::Rgb(1, 2, 3);
        application.theme.colors.title_foreground = Color::Rgb(4, 5, 6);
        application.theme.icons.left_panel_visible = "E".to_owned();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let controls = geometry(
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
        let buffer = terminal.backend().buffer();
        assert!(
            (explorer.x..explorer.x + explorer.width).any(|column| buffer
                .cell((column, explorer.y))
                .unwrap()
                .symbol()
                == "E")
        );
        assert_eq!(
            buffer.cell((explorer.x + 1, explorer.y)).unwrap().fg,
            Color::Rgb(4, 5, 6)
        );
        assert_eq!(
            buffer.cell((explorer.x + 1, explorer.y)).unwrap().bg,
            Color::Rgb(1, 2, 3)
        );
        assert_eq!(buffer.cell((0, 0)).unwrap().bg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn explorer_uses_a_focused_bordered_panel() {
        let mut application = Application::new_for_test();
        application.focus_owner = FocusOwner::Explorer;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        )
        .explorer
        .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((area.x, area.y)).unwrap().symbol(), "╭");
        assert_eq!(
            buffer.cell((area.x, area.y)).unwrap().fg,
            Color::Rgb(0, 122, 204)
        );
        let title = (area.x..area.x.saturating_add(area.width))
            .map(|column| buffer.cell((column, area.y)).unwrap().symbol())
            .collect::<String>();
        assert!(title.contains("Explorer"));
    }

    #[test]
    fn editor_border_is_active_only_while_the_workspace_owns_focus() {
        let mut application = Application::new_for_test();
        let render_editor_border = |application: &Application| {
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|frame| draw(frame, application)).unwrap();
            let area = geometry(
                application,
                Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 30,
                },
            )
            .pane_layout
            .panes[&application.workspace.focused];
            terminal
                .backend()
                .buffer()
                .cell((area.x, area.y))
                .unwrap()
                .fg
        };

        assert_eq!(render_editor_border(&application), Color::Rgb(0, 122, 204));

        application.focus_owner = FocusOwner::Explorer;
        assert_eq!(render_editor_border(&application), Color::DarkGray);

        application.bottom_terminal_visible = true;
        application.focus_owner = FocusOwner::BottomTerminal;
        assert_eq!(render_editor_border(&application), Color::DarkGray);

        application.overlay = Some(Overlay::Message {
            title: " Notice ".to_owned(),
            message: "Overlay owns focus.".to_owned(),
        });
        application.focus_owner = FocusOwner::Overlay;
        assert_eq!(render_editor_border(&application), Color::DarkGray);
    }

    #[test]
    fn terminal_focus_activates_only_the_terminal_border() {
        let mut application = Application::new_for_test();
        application.bottom_terminal_visible = true;
        application.focus_owner = FocusOwner::BottomTerminal;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let geometry = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        );
        let editor = geometry.pane_layout.panes[&application.workspace.focused];
        let terminal_area = geometry.terminal.unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(
            buffer.cell((editor.x, editor.y)).unwrap().fg,
            Color::DarkGray
        );
        assert_eq!(
            buffer.cell((terminal_area.x, terminal_area.y)).unwrap().fg,
            Color::Rgb(0, 122, 204)
        );
    }

    #[test]
    fn collapsed_open_editors_hides_documents_and_uses_the_root_name() {
        let mut application = Application::new_for_test();
        application.explorer.open_editors_expanded = false;
        application.theme.icons.explorer_new_file = "1".to_owned();
        application.theme.icons.explorer_new_folder = "2".to_owned();
        application.theme.icons.explorer_refresh = "3".to_owned();
        application.explorer.root = PathBuf::from("root");
        let root_name = application
            .explorer
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        )
        .explorer
        .unwrap();
        let buffer = terminal.backend().buffer();
        let row_text = |row| {
            (area.x.saturating_add(1)..area.x.saturating_add(area.width.saturating_sub(1)))
                .map(|column| buffer.cell((column, row)).unwrap().symbol())
                .collect::<String>()
        };
        assert!(row_text(area.y + 1).trim().starts_with(&format!(
            "{} Open Editors",
            application.theme.icons.tree_collapsed
        )));
        assert!(row_text(area.y + 2).trim().is_empty());
        assert!(row_text(area.y + 3).trim().contains(&root_name));
        assert!(row_text(area.y + 3).contains("1 2 3"));
        let root_row = row_text(area.y + 3);
        let action_start = root_row.find('1').unwrap();
        assert_eq!(root_row.as_bytes()[action_start - 1], b' ');
        assert!(!row_text(area.y + 1).contains("Untitled"));
        let root_cell = buffer.cell((area.x + 1, area.y + 3)).unwrap();
        assert!(root_cell.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn explorer_root_actions_remain_visible_when_workspace_name_is_wide() {
        let mut application = Application::new_for_test();
        application.explorer.open_editors_expanded = false;
        application.explorer.root = PathBuf::from("workspace-name-that-cannot-fit-in-the-explorer-header");
        application.theme.icons.explorer_new_file = "1".to_owned();
        application.theme.icons.explorer_new_folder = "2".to_owned();
        application.theme.icons.explorer_refresh = "3".to_owned();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 30,
            },
        )
        .explorer
        .unwrap();
        let row_text = (area.x.saturating_add(1)..area.x.saturating_add(area.width.saturating_sub(1)))
            .map(|column| terminal.backend().buffer().cell((column, area.y + 3)).unwrap().symbol())
            .collect::<String>();

        assert!(row_text.contains("1 2 3"));
        assert!(row_text.contains(application.theme.icons.overflow.as_str()));
    }

    #[test]
    fn empty_editor_has_an_inactive_border_without_a_title() {
        let mut application = Application::new_for_test();
        application.workspace = crate::workspace::Workspace::empty(0.5);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        )
        .pane_layout
        .panes[&application.workspace.focused];
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((area.x, area.y)).unwrap().fg, Color::DarkGray);
        let top_border = (area.x..area.x.saturating_add(area.width))
            .map(|column| buffer.cell((column, area.y)).unwrap().symbol())
            .collect::<String>();
        assert!(!top_border.contains("EDITOR"));
        assert!(!top_border.contains("Untitled"));
    }

    #[test]
    fn editor_selection_marks_only_intersecting_graphemes() {
        let spans = styled_editor_text(
            "a界b",
            10,
            Some(&(11..12)),
            0,
            Color::Rgb(38, 79, 120),
            usize::MAX,
        );
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].style.bg, None);
        assert_eq!(spans[1].style.bg, Some(Color::Rgb(38, 79, 120)));
        assert_eq!(spans[2].style.bg, None);
    }

    #[test]
    fn overflowing_editor_line_reserves_an_unselected_right_edge_marker() {
        let mut application = Application::new_for_test();
        application
            .apply(crate::command::Command::Insert("x".repeat(200)))
            .unwrap();
        let document_id = application.focused_document_id().unwrap();
        let document = application.documents.get(document_id);
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.selection_anchor = Some(0);
        view.cursor = document.len_chars();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        )
        .pane_layout
        .panes[&application.workspace.focused];
        let marker = terminal
            .backend()
            .buffer()
            .cell((area.x + area.width - 2, area.y + 1))
            .unwrap();
        assert_eq!(marker.symbol(), application.theme.icons.overflow);
        assert_ne!(
            marker.bg, application.theme.colors.editor_selection,
            "overflow marker must not inherit document selection styling"
        );
    }

    #[test]
    fn fitting_editor_line_does_not_render_an_overflow_marker() {
        let mut application = Application::new_for_test();
        application
            .apply(crate::command::Command::Insert("short".to_owned()))
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        )
        .pane_layout
        .panes[&application.workspace.focused];
        assert_ne!(
            terminal
                .backend()
                .buffer()
                .cell((area.x + area.width - 2, area.y + 1))
                .unwrap()
                .symbol(),
            application.theme.icons.overflow
        );
    }

    #[test]
    fn soft_wrap_renders_continuation_rows_without_overflow_markers() {
        let mut application = Application::new_for_test();
        application.config.effective.editor.soft_wrap = true;
        let initial_geometry = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 50,
                height: 12,
            },
        );
        let area = initial_geometry.pane_layout.panes[&application.workspace.focused];
        let document_id = application.focused_document_id().unwrap();
        let content =
            editor_content_geometry(&application, area, application.documents.get(document_id));
        let text = "x".repeat(content.width + 5);
        application
            .apply(crate::command::Command::Insert(text))
            .unwrap();
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.cursor = 0;
        view.selection_anchor = Some(content.width + 1);
        let mut terminal = Terminal::new(TestBackend::new(50, 12)).unwrap();

        terminal.draw(|frame| draw(frame, &application)).unwrap();

        let buffer = terminal.backend().buffer();
        let first_content_x = area.x + 1 + u16::try_from(content.line_number_width).unwrap();
        assert_eq!(
            buffer.cell((first_content_x, area.y + 1)).unwrap().symbol(),
            "x"
        );
        assert_eq!(
            buffer.cell((first_content_x, area.y + 2)).unwrap().symbol(),
            "x"
        );
        assert_eq!(
            buffer.cell((first_content_x, area.y + 2)).unwrap().bg,
            application.theme.colors.editor_selection
        );
        for row in area.y + 1..area.y + area.height - 1 {
            for column in area.x + 1..area.x + area.width - 1 {
                assert_ne!(
                    buffer.cell((column, row)).unwrap().symbol(),
                    application.theme.icons.overflow
                );
            }
        }
        for column in area.x + 1..first_content_x {
            assert_eq!(buffer.cell((column, area.y + 2)).unwrap().symbol(), " ");
        }
    }

    #[test]
    fn editor_scrollbar_thumb_is_proportional_and_visually_continuous() {
        let mut application = Application::new_for_test();
        let text = (0..100)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        application
            .apply(crate::command::Command::Insert(text))
            .unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        )
        .pane_layout
        .panes[&application.workspace.focused];
        let document_id = application.focused_document_id().unwrap();
        let maximum = crate::editor::EditorView::maximum_viewport_line(
            application.documents.get(document_id),
            usize::from(area.height.saturating_sub(2)),
        );
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.viewport_line = maximum / 2;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let column = area.x + area.width - 2;
        let thumb_rows = (area.y + 1..area.y + area.height - 1)
            .filter(|row| {
                terminal.backend().buffer().cell((column, *row)).unwrap().bg
                    == application.theme.colors.scrollbar
            })
            .collect::<Vec<_>>();
        assert!(thumb_rows.len() > 1);
        assert!(thumb_rows.len() < usize::from(area.height.saturating_sub(2)));
        assert!(thumb_rows.windows(2).all(|rows| rows[1] == rows[0] + 1));
        for row in &thumb_rows {
            let cell = terminal.backend().buffer().cell((column, *row)).unwrap();
            assert_eq!(cell.fg, application.theme.colors.scrollbar);
            assert_eq!(cell.bg, application.theme.colors.scrollbar);
            assert_eq!(cell.symbol(), application.theme.icons.scrollbar);
        }
        let first = *thumb_rows.first().unwrap();
        let last = *thumb_rows.last().unwrap();
        assert!(first > area.y + 1);
        assert!(last < area.y + area.height - 2);
        assert_ne!(
            terminal
                .backend()
                .buffer()
                .cell((column, first - 1))
                .unwrap()
                .bg,
            application.theme.colors.scrollbar
        );
        assert_ne!(
            terminal
                .backend()
                .buffer()
                .cell((column, last + 1))
                .unwrap()
                .bg,
            application.theme.colors.scrollbar
        );
    }

    #[test]
    fn horizontal_scrollbar_uses_an_offscreen_document_maximum() {
        let mut application = Application::new_for_test();
        let text = (0..40)
            .map(|line| format!("short {line}"))
            .chain(std::iter::once("x".repeat(200)))
            .collect::<Vec<_>>()
            .join("\n");
        application
            .apply(crate::command::Command::Insert(text))
            .unwrap();
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let area = geometry(&application, bounds).pane_layout.panes[&application.workspace.focused];
        let document_id = application.focused_document_id().unwrap();
        let document = application.documents.get(document_id);
        let content = editor_content_geometry(&application, area, document);
        let scrollbar = horizontal_scrollbar_geometry(area, content, document, 0).unwrap();
        assert!(scrollbar.maximum > 0);
        assert!(content.horizontal_scrollbar_visible);

        let mut terminal = Terminal::new(TestBackend::new(bounds.width, bounds.height)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let thumb_columns = (scrollbar.track_x..scrollbar.track_x + scrollbar.track_width)
            .filter(|column| {
                terminal
                    .backend()
                    .buffer()
                    .cell((*column, scrollbar.row))
                    .unwrap()
                    .symbol()
                    == application.theme.icons.scrollbar_horizontal
            })
            .collect::<Vec<_>>();
        assert_eq!(thumb_columns.len(), usize::from(scrollbar.thumb_width));
        assert!(
            thumb_columns
                .windows(2)
                .all(|columns| columns[1] == columns[0] + 1)
        );
        for column in thumb_columns {
            let cell = terminal
                .backend()
                .buffer()
                .cell((column, scrollbar.row))
                .unwrap();
            assert_eq!(cell.fg, application.theme.colors.scrollbar);
            assert_ne!(cell.bg, application.theme.colors.scrollbar);
            assert_eq!(cell.symbol(), application.theme.icons.scrollbar_horizontal);
        }
    }

    #[test]
    fn horizontal_scrollbar_is_suppressed_for_soft_wrap_and_fitting_lines() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let short_content =
            editor_content_geometry(&application, area, application.documents.get(document_id));
        assert!(!short_content.horizontal_scrollbar_visible);

        application
            .documents
            .get_mut(document_id)
            .insert(0, &"x".repeat(100));
        application.config.effective.editor.soft_wrap = true;
        let wrapped_content =
            editor_content_geometry(&application, area, application.documents.get(document_id));
        assert!(!wrapped_content.horizontal_scrollbar_visible);
        assert!(
            horizontal_scrollbar_geometry(
                area,
                wrapped_content,
                application.documents.get(document_id),
                0,
            )
            .is_none()
        );
    }

    #[test]
    fn horizontal_scrollbar_tracks_typing_undo_and_redo() {
        let mut application = Application::new_for_test();
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 12,
        };
        let area = geometry(&application, bounds).pane_layout.panes[&application.workspace.focused];
        let document_id = application.focused_document_id().unwrap();
        let visible_before =
            editor_content_geometry(&application, area, application.documents.get(document_id));
        assert!(!visible_before.horizontal_scrollbar_visible);

        application
            .apply(crate::command::Command::Insert("x".repeat(100)))
            .unwrap();
        let visible_after =
            editor_content_geometry(&application, area, application.documents.get(document_id));
        assert!(visible_after.horizontal_scrollbar_visible);

        application.apply(crate::command::Command::Undo).unwrap();
        let visible_after_undo =
            editor_content_geometry(&application, area, application.documents.get(document_id));
        assert!(!visible_after_undo.horizontal_scrollbar_visible);

        application.apply(crate::command::Command::Redo).unwrap();
        let visible_after_redo =
            editor_content_geometry(&application, area, application.documents.get(document_id));
        assert!(visible_after_redo.horizontal_scrollbar_visible);
    }

    #[test]
    fn terminal_child_colors_are_not_replaced_by_theme_selection_colors() {
        let cell = TerminalCell {
            character: 'x',
            foreground: TerminalColor::Indexed(1),
            background: TerminalColor::Rgb(2, 3, 4),
            ..TerminalCell::default()
        };
        let lines = terminal_lines(
            vec![vec![cell]],
            None,
            Color::LightGreen,
            Color::LightYellow,
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Indexed(1)));
        assert_eq!(lines[0].spans[0].style.bg, Some(Color::Rgb(2, 3, 4)));
    }

    #[test]
    fn horizontal_viewport_uses_display_columns_for_wide_text() {
        let mut application = Application::new_for_test();
        application
            .apply(crate::command::Command::Insert("界abc".to_owned()))
            .unwrap();
        let document_id = application.focused_document_id().unwrap();
        let document = application.documents.get(document_id);
        assert_eq!(crate::editor::char_at_display_column(document, 0, 2), 1);
    }

    #[test]
    fn rendering_clamps_a_stale_viewport_after_the_document_shrinks() {
        let mut application = Application::new_for_test();
        application
            .apply(crate::command::Command::Insert("first\nsecond".to_owned()))
            .unwrap();
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.viewport_line = usize::MAX;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        terminal.draw(|frame| draw(frame, &application)).unwrap();

        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        )
        .pane_layout
        .panes[&application.workspace.focused];
        let first_row = (area.x.saturating_add(1)
            ..area.x.saturating_add(area.width.saturating_sub(1)))
            .map(|column| {
                terminal
                    .backend()
                    .buffer()
                    .cell((column, area.y.saturating_add(1)))
                    .unwrap()
                    .symbol()
            })
            .collect::<String>();
        assert!(first_row.contains("first"));
    }

    #[test]
    fn file_dialog_view_follows_selection() {
        assert_eq!(file_dialog_view_start(0, 20, 5), 0);
        assert_eq!(file_dialog_view_start(4, 20, 5), 0);
        assert_eq!(file_dialog_view_start(6, 20, 5), 2);
        assert_eq!(file_dialog_view_start(19, 20, 5), 15);
    }

    #[test]
    fn editor_header_renders_tabs_and_breadcrumbs_according_to_config() {
        let mut application = Application::new_for_test();
        let document_id = application.focused_document_id().unwrap();

        // Default: Tabs
        assert_eq!(
            application.config.effective.editor.header_style,
            crate::config::EditorHeaderStyle::Tabs
        );
        let tab_line = editor_header_title(&application, document_id);
        assert!(!tab_line.spans.is_empty());
        assert!(
            tab_line
                .spans
                .iter()
                .any(|span| span.content.contains("Untitled"))
        );
        // Active tab should not contain file_default icon
        assert!(
            !tab_line
                .spans
                .iter()
                .any(|span| span.content.contains(&application.theme.icons.file_default))
        );

        // Configured: Breadcrumbs
        application.config.effective.editor.header_style =
            crate::config::EditorHeaderStyle::Breadcrumbs;
        let breadcrumb_line = editor_header_title(&application, document_id);
        assert!(breadcrumb_line.spans[0].content.contains("›"));
        assert!(breadcrumb_line.spans[0].content.contains("Untitled"));
    }

    #[test]
    fn active_line_number_highlighted_when_focused() {
        let mut application = Application::new_for_test();
        application
            .apply(crate::command::Command::Insert(
                "line1\nline2\nline3".to_owned(),
            ))
            .unwrap();
        let Pane::Editor { view, .. } = application.workspace.focused_pane_mut() else {
            panic!("expected editor pane");
        };
        view.cursor = 0; // on line 1
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        )
        .pane_layout
        .panes[&application.workspace.focused];
        let buffer = terminal.backend().buffer();
        // Line 1 gutter cell (active)
        let active_gutter_cell = buffer.cell((area.x + 1, area.y + 1)).unwrap();
        assert_eq!(
            active_gutter_cell.fg,
            application.theme.colors.line_number_active
        );
        assert!(active_gutter_cell.modifier.contains(Modifier::BOLD));
        // Line 2 gutter cell (inactive)
        let inactive_gutter_cell = buffer.cell((area.x + 1, area.y + 2)).unwrap();
        assert_eq!(inactive_gutter_cell.fg, application.theme.colors.muted);
    }

    #[test]
    fn explorer_open_editors_and_entries_have_clean_file_labels_without_bullets() {
        let mut application = Application::new_for_test();
        application.explorer.open_editors_expanded = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let area = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30,
            },
        )
        .explorer
        .unwrap();
        let buffer = terminal.backend().buffer();
        let open_editor_row = (area.x.saturating_add(1)
            ..area.x.saturating_add(area.width.saturating_sub(1)))
            .map(|column| buffer.cell((column, area.y + 2)).unwrap().symbol())
            .collect::<String>();
        assert!(open_editor_row.contains("Untitled"));
        assert!(!open_editor_row.contains(&application.theme.icons.file_default));
    }

    #[test]
    fn status_bar_renders_git_branch_when_available() {
        let application = Application::new_for_test();
        let git_dir = application.explorer.root.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let status_y = bounds.height - 1;
        let buffer = terminal.backend().buffer();
        let status_row = (0..bounds.width)
            .map(|col| buffer.cell((col, status_y)).unwrap().symbol())
            .collect::<String>();
        assert!(status_row.contains("EDIT"));
        assert!(status_row.contains("main"));
    }

    #[test]
    fn status_bar_renders_mode_pill_and_metadata() {
        let application = Application::new_for_test();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &application)).unwrap();
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let status_y = bounds.height - 1;
        let buffer = terminal.backend().buffer();
        let status_row = (0..bounds.width)
            .map(|col| buffer.cell((col, status_y)).unwrap().symbol())
            .collect::<String>();
        assert!(status_row.contains("EDIT"));
        assert!(status_row.contains("Ln 1, Col 1"));
        assert!(status_row.contains("UTF-8"));
    }
}
