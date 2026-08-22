use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

use crate::{AppError, Result};

const DEFAULT_THEME: &str = include_str!("../themes/default.toml");
const DEFAULT_THEME_ID: &str = "default";
const MAX_THEME_BYTES: u64 = 256 * 1024;
const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeMeta {
    pub name: String,
    pub schema_version: i64,
    pub border_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeColors {
    pub background: Color,
    pub foreground: Color,
    pub title_background: Color,
    pub title_foreground: Color,
    pub status_background: Color,
    pub status_foreground: Color,
    pub status_mode_background: Color,
    pub status_mode_foreground: Color,
    pub status_segment_background: Color,
    pub status_segment_foreground: Color,
    pub status_keyhint_background: Color,
    pub status_keyhint_foreground: Color,
    pub accent: Color,
    pub accent_foreground: Color,
    pub active_border: Color,
    pub inactive_border: Color,
    pub divider: Color,
    pub muted: Color,
    pub disabled: Color,
    pub selected_row_background: Color,
    pub editor_selection: Color,
    pub scrollbar: Color,
    pub overlay_surface: Color,
    pub terminal_selection_background: Color,
    pub terminal_selection_foreground: Color,
    pub tab_active_background: Color,
    pub tab_active_foreground: Color,
    pub tab_inactive_background: Color,
    pub tab_inactive_foreground: Color,
    pub tab_bar_background: Color,
    pub tab_border: Color,
    pub line_number_active: Color,
    pub current_line_background: Color,
    pub gutter_border: Color,
    pub tree_guide: Color,
    pub dialog_backdrop: Color,
    pub dialog_header_background: Color,
    pub dialog_header_foreground: Color,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeIcons {
    pub left_panel_visible: String,
    pub left_panel_hidden: String,
    pub bottom_panel_visible: String,
    pub bottom_panel_hidden: String,
    pub right_panel_visible: String,
    pub right_panel_hidden: String,
    pub settings: String,
    pub tree_expanded: String,
    pub tree_collapsed: String,
    pub folder_closed: String,
    pub folder_open: String,
    pub file_default: String,
    pub tab_close: String,
    pub tree_guide_vertical: String,
    pub tree_guide_branch: String,
    pub tree_guide_corner: String,
    pub search: String,
    pub git_branch: String,
    pub modified: String,
    pub scrollbar: String,
    pub scrollbar_horizontal: String,
    pub overflow: String,
    pub explorer_new_file: String,
    pub explorer_new_folder: String,
    pub explorer_refresh: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Theme {
    pub id: String,
    pub meta: ThemeMeta,
    pub colors: ThemeColors,
    pub icons: ThemeIcons,
}

impl Theme {
    pub fn border_type(&self) -> ratatui::widgets::BorderType {
        match self.meta.border_type.to_lowercase().as_str() {
            "rounded" => ratatui::widgets::BorderType::Rounded,
            "double" => ratatui::widgets::BorderType::Double,
            "thick" => ratatui::widgets::BorderType::Thick,
            _ => ratatui::widgets::BorderType::Plain,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemeSource {
    Embedded,
    User(PathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeEntry {
    pub theme: Theme,
    pub source: ThemeSource,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeDiagnostic {
    pub path: Option<PathBuf>,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ThemeRegistry {
    directory: PathBuf,
    entries: BTreeMap<String, ThemeEntry>,
    diagnostics: Vec<ThemeDiagnostic>,
}

impl ThemeRegistry {
    pub fn load_for_config(config_path: &Path) -> Result<Self> {
        let parent = config_path.parent().ok_or_else(|| {
            AppError::Arguments(format!("{} has no parent directory", config_path.display()))
        })?;
        Self::load(parent.join("themes"))
    }

    pub fn load(directory: PathBuf) -> Result<Self> {
        let (default, default_diagnostics) =
            parse_theme(DEFAULT_THEME_ID, DEFAULT_THEME, None, ParseMode::Complete).ok_or_else(
                || AppError::Config {
                    path: PathBuf::from("themes/default.toml"),
                    message: "the embedded default theme is invalid".to_owned(),
                },
            )?;
        if !default_diagnostics.is_empty() {
            return Err(AppError::Config {
                path: PathBuf::from("themes/default.toml"),
                message: default_diagnostics.join("; "),
            });
        }
        let mut registry = Self {
            directory,
            entries: BTreeMap::from([(
                DEFAULT_THEME_ID.to_owned(),
                ThemeEntry {
                    theme: default,
                    source: ThemeSource::Embedded,
                    diagnostics: Vec::new(),
                },
            )]),
            diagnostics: Vec::new(),
        };
        registry.discover();
        Ok(registry)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &ThemeEntry)> {
        self.entries.iter().map(|(id, entry)| (id.as_str(), entry))
    }

    pub fn get(&self, id: &str) -> Option<&ThemeEntry> {
        self.entries.get(id)
    }

    pub fn diagnostics(&self) -> &[ThemeDiagnostic] {
        &self.diagnostics
    }

    pub fn resolve(&self, requested: &str) -> (&Theme, Option<String>) {
        if let Some(entry) = self.entries.get(requested) {
            return (&entry.theme, None);
        }
        (
            &self.entries[DEFAULT_THEME_ID].theme,
            Some(format!(
                "Theme `{requested}` is unavailable; the embedded default theme is active"
            )),
        )
    }

    fn discover(&mut self) {
        let directory = match fs::read_dir(&self.directory) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.diagnostics.push(ThemeDiagnostic {
                    path: Some(self.directory.clone()),
                    message: format!("Could not read theme directory: {error}"),
                });
                return;
            }
        };
        let mut paths = directory
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry.path()),
                Err(error) => {
                    self.diagnostics.push(ThemeDiagnostic {
                        path: Some(self.directory.clone()),
                        message: format!("Could not inspect theme entry: {error}"),
                    });
                    None
                }
            })
            .filter(|path| path.is_file() && path.extension().is_some_and(|value| value == "toml"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            self.load_user_theme(path);
        }
    }

    fn load_user_theme(&mut self, path: PathBuf) {
        let Some(id) = path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
        else {
            self.diagnostics.push(ThemeDiagnostic {
                path: Some(path),
                message: "Theme filename must have a valid UTF-8 stem".to_owned(),
            });
            return;
        };
        if id == DEFAULT_THEME_ID {
            self.diagnostics.push(ThemeDiagnostic {
                path: Some(path),
                message: "Theme identifier `default` is reserved".to_owned(),
            });
            return;
        }
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.push_read_error(path, error);
                return;
            }
        };
        if metadata.len() > MAX_THEME_BYTES {
            self.diagnostics.push(ThemeDiagnostic {
                path: Some(path),
                message: format!("Theme exceeds the {MAX_THEME_BYTES}-byte size limit"),
            });
            return;
        }
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) => {
                self.push_read_error(path, error);
                return;
            }
        };
        let mut bytes = Vec::new();
        if let Err(error) = file
            .take(MAX_THEME_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
        {
            self.push_read_error(path, error);
            return;
        }
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_THEME_BYTES {
            self.diagnostics.push(ThemeDiagnostic {
                path: Some(path),
                message: format!("Theme exceeds the {MAX_THEME_BYTES}-byte size limit"),
            });
            return;
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(error) => {
                self.diagnostics.push(ThemeDiagnostic {
                    path: Some(path),
                    message: format!("Theme is not UTF-8: {error}"),
                });
                return;
            }
        };
        let base = &self.entries[DEFAULT_THEME_ID].theme;
        let Some((theme, diagnostics)) = parse_theme(&id, text, Some(base), ParseMode::Partial)
        else {
            self.diagnostics.push(ThemeDiagnostic {
                path: Some(path),
                message: "Theme contains invalid TOML".to_owned(),
            });
            return;
        };
        self.entries.insert(
            id,
            ThemeEntry {
                theme,
                source: ThemeSource::User(path),
                diagnostics,
            },
        );
    }

    fn push_read_error(&mut self, path: PathBuf, error: std::io::Error) {
        self.diagnostics.push(ThemeDiagnostic {
            path: Some(path),
            message: format!("Could not read theme: {error}"),
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParseMode {
    Complete,
    Partial,
}

fn parse_theme(
    id: &str,
    text: &str,
    base: Option<&Theme>,
    mode: ParseMode,
) -> Option<(Theme, Vec<String>)> {
    let value = match toml::from_str::<toml::Value>(text) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let root = value.as_table()?;
    let mut diagnostics = Vec::new();
    warn_unknown(root, &["meta", "colors", "icons"], "", &mut diagnostics);
    let meta = theme_table(root, "meta", mode, &mut diagnostics);
    let colors = theme_table(root, "colors", mode, &mut diagnostics);
    let icons = theme_table(root, "icons", mode, &mut diagnostics);

    let fallback = base.cloned().unwrap_or_else(empty_theme);
    if let Some(table) = meta {
        warn_unknown(
            table,
            &["name", "schema_version", "border_type"],
            "meta",
            &mut diagnostics,
        );
    }
    if let Some(table) = colors {
        warn_unknown(
            table,
            &[
                "background",
                "foreground",
                "title_background",
                "title_foreground",
                "status_background",
                "status_foreground",
                "status_mode_background",
                "status_mode_foreground",
                "status_segment_background",
                "status_segment_foreground",
                "status_keyhint_background",
                "status_keyhint_foreground",
                "accent",
                "accent_foreground",
                "active_border",
                "inactive_border",
                "divider",
                "muted",
                "disabled",
                "selected_row_background",
                "editor_selection",
                "scrollbar",
                "overlay_surface",
                "terminal_selection_background",
                "terminal_selection_foreground",
                "tab_active_background",
                "tab_active_foreground",
                "tab_inactive_background",
                "tab_inactive_foreground",
                "tab_bar_background",
                "tab_border",
                "line_number_active",
                "current_line_background",
                "gutter_border",
                "tree_guide",
                "dialog_backdrop",
                "dialog_header_background",
                "dialog_header_foreground",
            ],
            "colors",
            &mut diagnostics,
        );
    }
    if let Some(table) = icons {
        warn_unknown(
            table,
            &[
                "left_panel_visible",
                "left_panel_hidden",
                "bottom_panel_visible",
                "bottom_panel_hidden",
                "right_panel_visible",
                "right_panel_hidden",
                "settings",
                "tree_expanded",
                "tree_collapsed",
                "folder_closed",
                "folder_open",
                "file_default",
                "tab_close",
                "tree_guide_vertical",
                "tree_guide_branch",
                "tree_guide_corner",
                "search",
                "git_branch",
                "modified",
                "scrollbar",
                "scrollbar_horizontal",
                "overflow",
                "explorer_new_file",
                "explorer_new_folder",
                "explorer_refresh",
            ],
            "icons",
            &mut diagnostics,
        );
    }

    let inherited_name = if mode == ParseMode::Partial {
        id
    } else {
        &fallback.meta.name
    };
    let name = string_value(
        meta,
        "name",
        inherited_name,
        mode,
        &mut diagnostics,
        |value| !value.trim().is_empty() && !value.chars().any(char::is_control),
        "must be non-empty and contain no control characters",
    );
    let border_type = string_value(
        meta,
        "border_type",
        &fallback.meta.border_type,
        mode,
        &mut diagnostics,
        |value| !value.trim().is_empty() && !value.chars().any(char::is_control),
        "must be non-empty and contain no control characters",
    );
    let schema_version = integer_value(
        meta,
        "schema_version",
        fallback.meta.schema_version,
        mode,
        &mut diagnostics,
    );
    let schema_version = if schema_version == SCHEMA_VERSION {
        schema_version
    } else {
        diagnostics.push(format!(
            "meta.schema_version must be {SCHEMA_VERSION}; the inherited value was used"
        ));
        fallback.meta.schema_version
    };

    macro_rules! color {
        ($field:ident) => {
            color_value(
                colors,
                stringify!($field),
                fallback.colors.$field,
                mode,
                &mut diagnostics,
            )
        };
    }
    macro_rules! icon {
        ($field:ident, $width:expr, $exact:expr) => {
            icon_value(
                icons,
                stringify!($field),
                &fallback.icons.$field,
                mode,
                $width,
                $exact,
                &mut diagnostics,
            )
        };
    }

    let theme = Theme {
        id: id.to_owned(),
        meta: ThemeMeta {
            name,
            schema_version,
            border_type,
        },
        colors: ThemeColors {
            background: color!(background),
            foreground: color!(foreground),
            title_background: color!(title_background),
            title_foreground: color!(title_foreground),
            status_background: color!(status_background),
            status_foreground: color!(status_foreground),
            status_mode_background: color!(status_mode_background),
            status_mode_foreground: color!(status_mode_foreground),
            status_segment_background: color!(status_segment_background),
            status_segment_foreground: color!(status_segment_foreground),
            status_keyhint_background: color!(status_keyhint_background),
            status_keyhint_foreground: color!(status_keyhint_foreground),
            accent: color!(accent),
            accent_foreground: color!(accent_foreground),
            active_border: color!(active_border),
            inactive_border: color!(inactive_border),
            divider: color!(divider),
            muted: color!(muted),
            disabled: color!(disabled),
            selected_row_background: color!(selected_row_background),
            editor_selection: color!(editor_selection),
            scrollbar: color!(scrollbar),
            overlay_surface: color!(overlay_surface),
            terminal_selection_background: color!(terminal_selection_background),
            terminal_selection_foreground: color!(terminal_selection_foreground),
            tab_active_background: color!(tab_active_background),
            tab_active_foreground: color!(tab_active_foreground),
            tab_inactive_background: color!(tab_inactive_background),
            tab_inactive_foreground: color!(tab_inactive_foreground),
            tab_bar_background: color!(tab_bar_background),
            tab_border: color!(tab_border),
            line_number_active: color!(line_number_active),
            current_line_background: color!(current_line_background),
            gutter_border: color!(gutter_border),
            tree_guide: color!(tree_guide),
            dialog_backdrop: color!(dialog_backdrop),
            dialog_header_background: color!(dialog_header_background),
            dialog_header_foreground: color!(dialog_header_foreground),
        },
        icons: ThemeIcons {
            left_panel_visible: icon!(left_panel_visible, 3, false),
            left_panel_hidden: icon!(left_panel_hidden, 3, false),
            bottom_panel_visible: icon!(bottom_panel_visible, 3, false),
            bottom_panel_hidden: icon!(bottom_panel_hidden, 3, false),
            right_panel_visible: icon!(right_panel_visible, 3, false),
            right_panel_hidden: icon!(right_panel_hidden, 3, false),
            settings: icon!(settings, 3, false),
            tree_expanded: icon!(tree_expanded, 1, true),
            tree_collapsed: icon!(tree_collapsed, 1, true),
            folder_closed: icon!(folder_closed, 1, true),
            folder_open: icon!(folder_open, 1, true),
            file_default: icon!(file_default, 1, true),
            tab_close: icon!(tab_close, 1, true),
            tree_guide_vertical: icon!(tree_guide_vertical, 1, true),
            tree_guide_branch: icon!(tree_guide_branch, 1, true),
            tree_guide_corner: icon!(tree_guide_corner, 1, true),
            search: icon!(search, 3, false),
            git_branch: icon!(git_branch, 3, false),
            modified: icon!(modified, 1, true),
            scrollbar: icon!(scrollbar, 1, true),
            scrollbar_horizontal: icon!(scrollbar_horizontal, 1, true),
            overflow: icon!(overflow, 1, true),
            explorer_new_file: icon!(explorer_new_file, 1, true),
            explorer_new_folder: icon!(explorer_new_folder, 1, true),
            explorer_refresh: icon!(explorer_refresh, 1, true),
        },
    };
    Some((theme, diagnostics))
}

fn empty_theme() -> Theme {
    Theme {
        id: String::new(),
        meta: ThemeMeta {
            name: String::new(),
            schema_version: SCHEMA_VERSION,
            border_type: "rounded".to_owned(),
        },
        colors: ThemeColors {
            background: Color::Reset,
            foreground: Color::Reset,
            title_background: Color::Reset,
            title_foreground: Color::Reset,
            status_background: Color::Reset,
            status_foreground: Color::Reset,
            status_mode_background: Color::Reset,
            status_mode_foreground: Color::Reset,
            status_segment_background: Color::Reset,
            status_segment_foreground: Color::Reset,
            status_keyhint_background: Color::Reset,
            status_keyhint_foreground: Color::Reset,
            accent: Color::Reset,
            accent_foreground: Color::Reset,
            active_border: Color::Reset,
            inactive_border: Color::Reset,
            divider: Color::Reset,
            muted: Color::Reset,
            disabled: Color::Reset,
            selected_row_background: Color::Reset,
            editor_selection: Color::Reset,
            scrollbar: Color::Reset,
            overlay_surface: Color::Reset,
            terminal_selection_background: Color::Reset,
            terminal_selection_foreground: Color::Reset,
            tab_active_background: Color::Reset,
            tab_active_foreground: Color::Reset,
            tab_inactive_background: Color::Reset,
            tab_inactive_foreground: Color::Reset,
            tab_bar_background: Color::Reset,
            tab_border: Color::Reset,
            line_number_active: Color::Reset,
            current_line_background: Color::Reset,
            gutter_border: Color::Reset,
            tree_guide: Color::Reset,
            dialog_backdrop: Color::Reset,
            dialog_header_background: Color::Reset,
            dialog_header_foreground: Color::Reset,
        },
        icons: ThemeIcons {
            left_panel_visible: String::new(),
            left_panel_hidden: String::new(),
            bottom_panel_visible: String::new(),
            bottom_panel_hidden: String::new(),
            right_panel_visible: String::new(),
            right_panel_hidden: String::new(),
            settings: String::new(),
            tree_expanded: String::new(),
            tree_collapsed: String::new(),
            folder_closed: String::new(),
            folder_open: String::new(),
            file_default: String::new(),
            tab_close: String::new(),
            tree_guide_vertical: String::new(),
            tree_guide_branch: String::new(),
            tree_guide_corner: String::new(),
            search: String::new(),
            git_branch: String::new(),
            modified: String::new(),
            scrollbar: String::new(),
            scrollbar_horizontal: String::new(),
            overflow: String::new(),
            explorer_new_file: String::new(),
            explorer_new_folder: String::new(),
            explorer_refresh: String::new(),
        },
    }
}

fn theme_table<'a>(
    root: &'a toml::Table,
    key: &str,
    mode: ParseMode,
    diagnostics: &mut Vec<String>,
) -> Option<&'a toml::Table> {
    match root.get(key) {
        Some(value) => match value.as_table() {
            Some(table) => Some(table),
            None => {
                diagnostics.push(format!("{key} must be a table; inherited values were used"));
                None
            }
        },
        None => {
            if mode == ParseMode::Complete {
                diagnostics.push(format!("{key} table is required"));
            }
            None
        }
    }
}

fn warn_unknown(table: &toml::Table, known: &[&str], prefix: &str, diagnostics: &mut Vec<String>) {
    for key in table.keys().filter(|key| !known.contains(&key.as_str())) {
        let separator = if prefix.is_empty() { "" } else { "." };
        diagnostics.push(format!("unknown theme key `{prefix}{separator}{key}`"));
    }
}

fn string_value(
    table: Option<&toml::Table>,
    key: &str,
    fallback: &str,
    mode: ParseMode,
    diagnostics: &mut Vec<String>,
    validate: impl Fn(&str) -> bool,
    validation_message: &str,
) -> String {
    let Some(value) = table.and_then(|table| table.get(key)) else {
        if mode == ParseMode::Complete {
            diagnostics.push(format!("meta.{key} is required"));
        }
        return fallback.to_owned();
    };
    let Some(value) = value.as_str() else {
        diagnostics.push(format!(
            "meta.{key} must be a string; the inherited value was used"
        ));
        return fallback.to_owned();
    };
    if validate(value) {
        value.to_owned()
    } else {
        diagnostics.push(format!(
            "meta.{key} {validation_message}; the inherited value was used"
        ));
        fallback.to_owned()
    }
}

fn integer_value(
    table: Option<&toml::Table>,
    key: &str,
    fallback: i64,
    mode: ParseMode,
    diagnostics: &mut Vec<String>,
) -> i64 {
    let Some(value) = table.and_then(|table| table.get(key)) else {
        if mode == ParseMode::Complete {
            diagnostics.push(format!("meta.{key} is required"));
        }
        return fallback;
    };
    value.as_integer().unwrap_or_else(|| {
        diagnostics.push(format!(
            "meta.{key} must be an integer; the inherited value was used"
        ));
        fallback
    })
}

fn color_value(
    table: Option<&toml::Table>,
    key: &str,
    fallback: Color,
    mode: ParseMode,
    diagnostics: &mut Vec<String>,
) -> Color {
    let Some(value) = table.and_then(|table| table.get(key)) else {
        if mode == ParseMode::Complete {
            diagnostics.push(format!("colors.{key} is required"));
        }
        return fallback;
    };
    let Some(value) = value.as_str() else {
        diagnostics.push(format!(
            "colors.{key} must be a string; the inherited value was used"
        ));
        return fallback;
    };
    parse_color(value).unwrap_or_else(|| {
        diagnostics.push(format!(
            "colors.{key} has unsupported color `{value}`; the inherited value was used"
        ));
        fallback
    })
}

fn parse_color(value: &str) -> Option<Color> {
    match value {
        "default" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" => Some(Color::Gray),
        "dark-gray" => Some(Color::DarkGray),
        "light-red" => Some(Color::LightRed),
        "light-green" => Some(Color::LightGreen),
        "light-yellow" => Some(Color::LightYellow),
        "light-blue" => Some(Color::LightBlue),
        "light-magenta" => Some(Color::LightMagenta),
        "light-cyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        value if value.len() == 7 && value.starts_with('#') => {
            let red = u8::from_str_radix(&value[1..3], 16).ok()?;
            let green = u8::from_str_radix(&value[3..5], 16).ok()?;
            let blue = u8::from_str_radix(&value[5..7], 16).ok()?;
            Some(Color::Rgb(red, green, blue))
        }
        _ => None,
    }
}

fn icon_value(
    table: Option<&toml::Table>,
    key: &str,
    fallback: &str,
    mode: ParseMode,
    maximum_width: usize,
    exact_width: bool,
    diagnostics: &mut Vec<String>,
) -> String {
    let Some(value) = table.and_then(|table| table.get(key)) else {
        if mode == ParseMode::Complete {
            diagnostics.push(format!("icons.{key} is required"));
        }
        return fallback.to_owned();
    };
    let Some(value) = value.as_str() else {
        diagnostics.push(format!(
            "icons.{key} must be a string; the inherited value was used"
        ));
        return fallback.to_owned();
    };
    let width = value.width();
    let invalid_width = if exact_width {
        width != maximum_width
    } else {
        width == 0 || width > maximum_width
    };
    if value.chars().any(char::is_control) || invalid_width {
        let requirement = if exact_width {
            format!("exactly {maximum_width}")
        } else {
            format!("between 1 and {maximum_width}")
        };
        diagnostics.push(format!(
            "icons.{key} must contain no control characters and occupy {requirement} display cells; the inherited value was used"
        ));
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn border_type_maps_to_ratatui_border_types() {
        let mut theme = ThemeRegistry::load(tempdir().unwrap().path().join("missing"))
            .unwrap()
            .get("default")
            .unwrap()
            .theme
            .clone();
        assert_eq!(theme.border_type(), ratatui::widgets::BorderType::Rounded);
        theme.meta.border_type = "plain".to_owned();
        assert_eq!(theme.border_type(), ratatui::widgets::BorderType::Plain);
        theme.meta.border_type = "double".to_owned();
        assert_eq!(theme.border_type(), ratatui::widgets::BorderType::Double);
        theme.meta.border_type = "thick".to_owned();
        assert_eq!(theme.border_type(), ratatui::widgets::BorderType::Thick);
        theme.meta.border_type = "other".to_owned();
        assert_eq!(theme.border_type(), ratatui::widgets::BorderType::Plain);
    }

    #[test]
    fn embedded_default_is_complete_and_valid() {
        let registry = ThemeRegistry::load(tempdir().unwrap().path().join("missing")).unwrap();
        let default = registry.get("default").unwrap();
        assert_eq!(default.theme.meta.schema_version, 1);
        assert_eq!(default.theme.meta.border_type, "rounded");
        assert_eq!(default.theme.icons.left_panel_visible, "◧");
        assert_eq!(default.theme.icons.left_panel_hidden, "◫");
        assert_eq!(default.theme.icons.bottom_panel_visible, "⬓");
        assert_eq!(default.theme.icons.bottom_panel_hidden, "⊟");
        assert_eq!(default.theme.icons.right_panel_visible, "◨");
        assert_eq!(default.theme.icons.right_panel_hidden, "◫");
        assert_eq!(default.theme.icons.settings, "⚙");
        assert_eq!(default.theme.icons.folder_open, "▾");
        assert_eq!(default.theme.icons.folder_closed, "▸");
        assert_eq!(default.theme.icons.file_default, "•");
        assert_eq!(default.theme.icons.git_branch, "⎇");
        assert_eq!(default.theme.icons.scrollbar_horizontal, "▀");
        assert_eq!(default.theme.icons.explorer_new_file, "+");
        assert_eq!(default.theme.icons.explorer_new_folder, "+");
        assert_eq!(default.theme.icons.explorer_refresh, "R");
        assert!(default.diagnostics.is_empty());
    }

    #[test]
    fn partial_theme_preserves_valid_siblings_and_inherits_invalid_values() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("ocean.toml"),
            "[meta]\nname = \"Ocean\"\n[colors]\naccent = \"#123456\"\nmuted = \"invalid\"\n[icons]\ntree_collapsed = \"too-wide\"\nscrollbar_horizontal = \"▄\"\n",
        )
        .unwrap();
        let registry = ThemeRegistry::load(directory.path().to_owned()).unwrap();
        let ocean = registry.get("ocean").unwrap();
        let default = registry.get("default").unwrap();
        assert_eq!(ocean.theme.colors.accent, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(ocean.theme.colors.muted, default.theme.colors.muted);
        assert_eq!(
            ocean.theme.icons.tree_collapsed,
            default.theme.icons.tree_collapsed
        );
        assert_eq!(ocean.theme.icons.scrollbar_horizontal, "▄");
        assert_eq!(
            ocean.theme.icons.explorer_new_file,
            default.theme.icons.explorer_new_file
        );
        assert_eq!(
            ocean.theme.icons.explorer_new_folder,
            default.theme.icons.explorer_new_folder
        );
        assert_eq!(
            ocean.theme.icons.explorer_refresh,
            default.theme.icons.explorer_refresh
        );
        assert_eq!(ocean.diagnostics.len(), 2);
    }

    #[test]
    fn discovery_is_sorted_bounded_and_reserves_default() {
        let directory = tempdir().unwrap();
        fs::create_dir(directory.path().join("nested")).unwrap();
        fs::write(
            directory.path().join("nested/ignored.toml"),
            "[meta]\nname = \"Ignored\"\n",
        )
        .unwrap();
        fs::write(directory.path().join("z.toml"), "[meta]\nname = \"Z\"\n").unwrap();
        fs::write(directory.path().join("a.toml"), "[meta]\nname = \"Z\"\n").unwrap();
        fs::write(
            directory.path().join("default.toml"),
            "[meta]\nname = \"Override\"\n",
        )
        .unwrap();
        fs::write(directory.path().join("ignored.txt"), "ignored").unwrap();
        let registry = ThemeRegistry::load(directory.path().to_owned()).unwrap();
        assert_eq!(
            registry.entries().map(|(id, _)| id).collect::<Vec<_>>(),
            vec!["a", "default", "z"]
        );
        assert!(
            registry
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message.contains("reserved"))
        );
        assert!(registry.get("ignored").is_none());
    }

    #[test]
    fn malformed_theme_is_reported_without_replacing_default() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("broken.toml"), "[meta").unwrap();
        let registry = ThemeRegistry::load(directory.path().to_owned()).unwrap();
        assert!(registry.get("broken").is_none());
        assert!(registry.get("default").is_some());
        assert_eq!(registry.diagnostics().len(), 1);
    }

    #[test]
    fn oversized_and_non_utf8_themes_are_rejected() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("large.toml"),
            vec![b' '; usize::try_from(MAX_THEME_BYTES).unwrap() + 1],
        )
        .unwrap();
        fs::write(directory.path().join("binary.toml"), [0xff]).unwrap();
        let registry = ThemeRegistry::load(directory.path().to_owned()).unwrap();
        assert!(registry.get("large").is_none());
        assert!(registry.get("binary").is_none());
        assert_eq!(registry.diagnostics().len(), 2);
    }

    #[test]
    fn color_parser_rejects_alpha_and_unknown_names() {
        for name in [
            "default",
            "black",
            "red",
            "green",
            "yellow",
            "blue",
            "magenta",
            "cyan",
            "gray",
            "dark-gray",
            "light-red",
            "light-green",
            "light-yellow",
            "light-blue",
            "light-magenta",
            "light-cyan",
            "white",
        ] {
            assert!(parse_color(name).is_some(), "{name} should be supported");
        }
        assert_eq!(parse_color("#010203"), Some(Color::Rgb(1, 2, 3)));
        assert_eq!(parse_color("#010203ff"), None);
        assert_eq!(parse_color("orange"), None);
    }

    #[test]
    fn schema_unknown_keys_and_invalid_glyph_widths_are_diagnostic() {
        let registry = ThemeRegistry::load(tempdir().unwrap().path().join("missing")).unwrap();
        let base = &registry.get("default").unwrap().theme;
        let (theme, diagnostics) = parse_theme(
            "invalid",
            "unknown = true\n[meta]\nname = \"\"\nschema_version = 2\nextra = true\n[icons]\nexplorer_visible = \"\"\ntree_collapsed = \"界\"\nsettings = \"\\n\"\nscrollbar_horizontal = \"界\"\n",
            Some(base),
            ParseMode::Partial,
        )
        .unwrap();
        assert_eq!(theme.meta.schema_version, 1);
        assert_eq!(
            theme.icons.left_panel_visible,
            base.icons.left_panel_visible
        );
        assert_eq!(theme.icons.tree_collapsed, base.icons.tree_collapsed);
        assert_eq!(
            theme.icons.scrollbar_horizontal,
            base.icons.scrollbar_horizontal
        );
        for expected in [
            "unknown theme key `unknown`",
            "unknown theme key `meta.extra`",
            "meta.name",
            "meta.schema_version",
            "icons.explorer_visible",
            "icons.tree_collapsed",
            "icons.settings",
            "icons.scrollbar_horizontal",
        ] {
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.contains(expected)),
                "missing diagnostic for {expected}: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn missing_active_theme_resolves_to_embedded_default() {
        let registry = ThemeRegistry::load(tempdir().unwrap().path().join("missing")).unwrap();
        let (theme, message) = registry.resolve("missing");
        assert_eq!(theme.id, "default");
        assert!(message.is_some_and(|message| message.contains("unavailable")));
    }
}
