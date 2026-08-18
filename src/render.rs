use ratatui::{
    Frame,
    layout::Rect as TuiRect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
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

#[derive(Clone, Debug, Default)]
pub struct WorkbenchGeometry {
    pub title: Option<Rect>,
    pub explorer: Option<Rect>,
    pub workspace: Rect,
    pub bottom_terminal: Option<Rect>,
    pub status: Rect,
    pub pane_layout: CalculatedLayout,
}

pub fn geometry(application: &Application, screen: Rect) -> WorkbenchGeometry {
    let status_height = u16::from(screen.height > 0);
    let show_title = screen.height >= 10;
    let title_height = u16::from(show_title);
    let mut content = Rect {
        x: screen.x,
        y: screen.y.saturating_add(title_height),
        width: screen.width,
        height: screen.height.saturating_sub(title_height + status_height),
    };
    let title = show_title.then_some(Rect {
        height: 1,
        ..screen
    });
    let status = Rect {
        x: screen.x,
        y: screen
            .y
            .saturating_add(screen.height.saturating_sub(status_height)),
        width: screen.width,
        height: status_height,
    };
    let explorer = (application.explorer.visible && screen.width >= 80).then(|| {
        let width = application
            .config
            .effective
            .explorer
            .width
            .min(content.width.saturating_sub(20));
        let area = Rect { width, ..content };
        content.x = content.x.saturating_add(width.saturating_add(1));
        content.width = content.width.saturating_sub(width.saturating_add(1));
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
        explorer,
        workspace: content,
        bottom_terminal,
        status,
        pane_layout,
    }
}

pub fn draw(frame: &mut Frame<'_>, application: &Application) {
    let screen = from_tui(frame.area());
    let geometry = geometry(application, screen);
    if let Some(area) = geometry.title {
        frame.render_widget(
            Paragraph::new(" TinLynk Terminal Editor")
                .style(Style::default().bg(Color::Rgb(45, 45, 48))),
            to_tui(area),
        );
    }
    if let Some(area) = geometry.explorer {
        draw_explorer(frame, application, area);
    }
    draw_workspace(frame, application, &geometry.pane_layout);
    for splitter in &geometry.pane_layout.splitters {
        frame.render_widget(
            Paragraph::new(" ").style(Style::default().bg(Color::DarkGray)),
            to_tui(splitter.rect),
        );
    }
    if let Some(area) = geometry.bottom_terminal {
        draw_bottom_terminal(frame, application, area);
    }
    draw_status(frame, application, geometry.status);
    draw_overlay(frame, application, screen);
}

fn draw_explorer(frame: &mut Frame<'_>, application: &Application, area: Rect) {
    let mut lines = vec![Line::styled(
        format!(
            "{} Open Editors",
            if application.explorer.open_editors_expanded {
                "⌄"
            } else {
                ">"
            }
        ),
        Style::default().fg(Color::DarkGray),
    )];
    if application.explorer.open_editors_expanded {
        for document_id in application.open_document_ids() {
            let document = application.documents.get(document_id);
            let name = document
                .path
                .as_deref()
                .and_then(|path| path.file_name())
                .map_or_else(|| "Untitled".into(), |name| name.to_string_lossy());
            let modified = if document.is_modified() { " ●" } else { "" };
            let style = if application.focused_document_id() == Some(document_id) {
                Style::default().bg(Color::Rgb(55, 55, 61))
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!("  {name}{modified}"), style));
        }
    }
    lines.push(Line::styled(
        application
            .explorer
            .root
            .file_name()
            .unwrap_or(application.explorer.root.as_os_str())
            .to_string_lossy()
            .into_owned(),
        Style::default().fg(Color::DarkGray),
    ));
    for (index, entry) in application.explorer.entries().iter().enumerate() {
        let prefix = if entry.is_directory && !entry.is_symlink {
            if entry.expanded { "⌄" } else { ">" }
        } else {
            " "
        };
        let name = entry.path.file_name().map_or_else(
            || entry.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let style = if index == application.explorer.selected
            && application.focus_owner == FocusOwner::Explorer
        {
            Style::default().bg(Color::Rgb(55, 55, 61))
        } else {
            Style::default()
        };
        lines.push(Line::styled(
            format!("{}{} {name}", "  ".repeat(entry.depth), prefix),
            style,
        ));
    }
    let border = if application.focus_owner == FocusOwner::Explorer {
        Color::Rgb(0, 122, 204)
    } else {
        Color::DarkGray
    };
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border))
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
                    .border_style(Style::default().fg(Color::DarkGray));
                frame.render_widget(Paragraph::new("").block(block), to_tui(*area));
            }
            Pane::Editor { document_id, view } => {
                let document = application.documents.get(*document_id);
                draw_editor(frame, application, *pane_id, *area, document, view);
            }
            Pane::Terminal { terminal_id } => {
                let lines = application.terminal_sessions.get(terminal_id).map_or_else(
                    || vec![Line::raw("Starting terminal")],
                    |session| {
                        terminal_lines(
                            session.visible_cells(usize::from(area.height.saturating_sub(2))),
                            session.selection(),
                        )
                    },
                );
                let block = pane_block(application, *pane_id, "Terminal");
                frame.render_widget(Paragraph::new(lines).block(block), to_tui(*area));
                if *pane_id == application.workspace.focused
                    && application.focus_owner == FocusOwner::Workspace
                    && let Some(session) = application.terminal_sessions.get(terminal_id)
                    && session.at_live_bottom()
                {
                    let (row, column) = session.cursor();
                    frame.set_cursor_position((
                        area.x.saturating_add(1).saturating_add(column),
                        area.y.saturating_add(1).saturating_add(row),
                    ));
                }
            }
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
) {
    let name = document
        .path
        .as_deref()
        .and_then(|path| path.file_name())
        .map_or("Untitled".to_owned(), |name| {
            name.to_string_lossy().into_owned()
        });
    let title = format!(" {name}{} ", if document.is_modified() { " ●" } else { "" });
    let inner_height = usize::from(area.height.saturating_sub(2));
    let line_number_width = if application.config.effective.editor.line_numbers {
        document.len_lines().to_string().len().max(2) + 1
    } else {
        0
    };
    let mut lines = Vec::with_capacity(inner_height);
    let selection = view.selection();
    for line_index in view.viewport_line..document.len_lines() {
        if lines.len() >= inner_height {
            break;
        }
        let end = document.line_content_end(line_index);
        let segment_start =
            crate::editor::char_at_display_column(document, line_index, view.viewport_column)
                .min(end);
        let maximum_chars = usize::from(area.width).saturating_mul(4).max(1);
        let segment_end = segment_start.saturating_add(maximum_chars).min(end);
        let text = document.slice(segment_start, segment_end);
        let mut spans = Vec::new();
        if line_number_width > 0 {
            spans.push(Span::styled(
                format!("{:>width$} ", line_index + 1, width = line_number_width - 1),
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.extend(styled_editor_text(
            &text,
            segment_start,
            selection.as_ref(),
            0,
        ));
        lines.push(Line::from(spans));
    }
    let block = pane_block(application, pane_id, &title);
    frame.render_widget(Paragraph::new(lines).block(block), to_tui(area));
    if application.config.effective.editor.scrollbar
        && document.len_lines() > inner_height
        && area.width > 3
        && inner_height > 0
    {
        let thumb_row = view
            .viewport_line
            .saturating_mul(inner_height.saturating_sub(1))
            / document.len_lines().saturating_sub(1).max(1);
        frame.render_widget(
            Paragraph::new("█").style(Style::default().fg(Color::DarkGray)),
            TuiRect::new(
                area.x.saturating_add(area.width.saturating_sub(2)),
                area.y
                    .saturating_add(1)
                    .saturating_add(u16::try_from(thumb_row).unwrap_or(u16::MAX)),
                1,
                1,
            ),
        );
    }
    if pane_id == application.workspace.focused
        && application.focus_owner == FocusOwner::Workspace
        && area.width > 2
        && area.height > 2
    {
        let line = document.char_to_line(view.cursor);
        let row = line.saturating_sub(view.viewport_line);
        if row < inner_height {
            let column = crate::editor::display_column(document, line, view.cursor)
                .saturating_sub(view.viewport_column)
                .saturating_add(line_number_width);
            let x = area
                .x
                .saturating_add(1)
                .saturating_add(u16::try_from(column).unwrap_or(u16::MAX));
            let y = area
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            if x < area.x.saturating_add(area.width.saturating_sub(1))
                && y < area.y.saturating_add(area.height.saturating_sub(1))
            {
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
) -> Vec<Span<'static>> {
    let mut display_column: usize = 0;
    let mut char_offset: usize = 0;
    text.graphemes(true)
        .filter_map(|grapheme| {
            let width = grapheme.width();
            let grapheme_start = line_start + char_offset;
            char_offset += grapheme.chars().count();
            let grapheme_end = line_start + char_offset;
            let visible = display_column.saturating_add(width) > viewport_column;
            display_column = display_column.saturating_add(width);
            visible.then(|| {
                let selected = selection
                    .is_some_and(|range| range.start < grapheme_end && range.end > grapheme_start);
                Span::styled(
                    grapheme.to_owned(),
                    if selected {
                        Style::default().bg(Color::Rgb(38, 79, 120))
                    } else {
                        Style::default()
                    },
                )
            })
        })
        .collect()
}

fn pane_block<'a>(application: &Application, pane_id: PaneId, title: &'a str) -> Block<'a> {
    let style = if pane_id == application.workspace.focused {
        Style::default().fg(Color::Rgb(0, 122, 204))
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(title)
}

fn draw_bottom_terminal(frame: &mut Frame<'_>, application: &Application, area: Rect) {
    let lines = application
        .bottom_terminal
        .and_then(|id| application.terminal_sessions.get(&id))
        .map_or_else(
            || vec![Line::raw("Terminal is not running")],
            |session| {
                terminal_lines(
                    session.visible_cells(usize::from(area.height.saturating_sub(2))),
                    session.selection(),
                )
            },
        );
    let style = if application.focus_owner == FocusOwner::BottomTerminal {
        Style::default().fg(Color::Rgb(0, 122, 204))
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(style)
                .title(" Terminal "),
        ),
        to_tui(area),
    );
    if application.focus_owner == FocusOwner::BottomTerminal
        && let Some(session) = application
            .bottom_terminal
            .and_then(|id| application.terminal_sessions.get(&id))
            .filter(|session| session.at_live_bottom())
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
                            style = style.bg(Color::Blue).fg(Color::White);
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
    let status = application.status_text();
    frame.render_widget(
        Paragraph::new(format!(" {status}")).style(
            Style::default()
                .bg(Color::Rgb(0, 122, 204))
                .fg(Color::White),
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
            directory,
            input,
            entries,
            selected,
            save_as,
            error,
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
                format!(
                    "Folder: {}\nPath: {input}\n{listing}\n{}",
                    directory.display(),
                    error.as_deref().unwrap_or(if *save_as {
                        "Type a filename and press Enter to save, Right opens the selected folder, Left goes up, and Esc cancels."
                    } else {
                        "Up/Down selects, Enter/Right opens, Left goes up, Ctrl+Enter submits the path, and Esc cancels."
                    })
                ),
            )
        }
        Overlay::Settings {
            query,
            scope,
            selected,
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
                    format!(
                        "{} {key} = {} (default {}, {source:?})",
                        if *selected == index { ">" } else { " " },
                        setting_value(&application.config.effective, key),
                        setting_value(&defaults, key),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                " Settings ",
                format!(
                    "Scope: {scope:?}  Search: {query}\n\n{listing}\n\nUp/Down selects, Ctrl+R resets, Tab changes scope, Enter opens raw configuration, and Esc closes."
                ),
            )
        }
        Overlay::Message { title, message } => (title.as_str(), message.clone()),
        Overlay::ConfirmQuit => (
            " Unsaved Changes ",
            "Modified documents remain. Press Ctrl+Q again to discard and exit, or Esc to cancel.".to_owned(),
        ),
        Overlay::ConfirmTerminateAll => (
            " Live Terminals ",
            "Live terminal processes remain. Press T to terminate them and exit, or Esc to cancel."
                .to_owned(),
        ),
        Overlay::ConfirmTerminalClose { .. } => (
            " Live Terminal ",
            "Press T to terminate the live terminal process, or Esc to cancel.".to_owned(),
        ),
        Overlay::ConfirmDocumentClose => (
            " Unsaved Document ",
            "Press S to save, D to discard, or Esc to cancel.".to_owned(),
        ),
        Overlay::SaveConflict => (
            " File Changed Externally ",
            "Press O to overwrite the file, or Esc to cancel. Save As is available from the file dialog.".to_owned(),
        ),
    };
    frame.render_widget(Clear, to_tui(area));
    let paragraph = Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(title));
    let paragraph = if matches!(overlay, Overlay::FileDialog { .. }) {
        paragraph
    } else {
        paragraph.wrap(Wrap { trim: false })
    };
    frame.render_widget(paragraph, to_tui(area));
}

fn setting_value(config: &crate::config::Config, dotted_key: &str) -> String {
    let Ok(mut value) = toml::Value::try_from(config) else {
        return "unavailable".to_owned();
    };
    for segment in dotted_key.split('.') {
        let Some(next) = value.get(segment).cloned() else {
            return "unavailable".to_owned();
        };
        value = next;
    }
    value.to_string()
}

pub fn overlay_area(screen: Rect, overlay: &Overlay) -> Rect {
    let width = screen.width.saturating_sub(4).clamp(1, 70);
    let height = match overlay {
        Overlay::Settings { .. } => screen.height.saturating_sub(4).clamp(3, 18),
        Overlay::FileDialog { .. } => screen
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
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::Application;

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
        assert!(wide.bottom_terminal.is_some());
        assert!(wide.title.is_some());
        let compact = geometry(
            &application,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
        );
        assert!(compact.explorer.is_none());
        assert!(compact.bottom_terminal.is_none());
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
        assert_eq!(buffer.cell((area.x, area.y)).unwrap().symbol(), "┌");
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
    fn collapsed_open_editors_hides_documents_and_uses_the_root_name() {
        let mut application = Application::new_for_test();
        application.explorer.open_editors_expanded = false;
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
        assert!(row_text(area.y + 1).trim().starts_with("> Open Editors"));
        assert_eq!(row_text(area.y + 2).trim(), root_name);
        assert!(!row_text(area.y + 1).contains("Untitled"));
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
        let spans = styled_editor_text("a界b", 10, Some(&(11..12)), 0);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].style.bg, None);
        assert_eq!(spans[1].style.bg, Some(Color::Rgb(38, 79, 120)));
        assert_eq!(spans[2].style.bg, None);
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
    fn file_dialog_view_follows_selection() {
        assert_eq!(file_dialog_view_start(0, 20, 5), 0);
        assert_eq!(file_dialog_view_start(4, 20, 5), 0);
        assert_eq!(file_dialog_view_start(6, 20, 5), 2);
        assert_eq!(file_dialog_view_start(19, 20, 5), 15);
    }
}
