use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::{AppError, Result};

const CONFIG_RELATIVE_PATH: &str = ".tinlynk/terminal-editor/config.toml";
const DEFAULT_UNDO_LIMIT_MIB: usize = 32;
const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
const DEFAULT_SYNTAX_HIGHLIGHTING_MAX_FILE_SIZE_KIB: usize = 1024;
const DEFAULT_AUTOSAVE_INTERVAL_SECONDS: i64 = 10;
static CONFIG_TEMPORARY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorHeaderStyle {
    #[default]
    Tabs,
    Breadcrumbs,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct EditorConfig {
    pub indent_width: u8,
    pub insert_spaces: bool,
    pub default_line_ending: LineEnding,
    pub undo_limit_mib: usize,
    pub line_numbers: bool,
    pub scrollbar: bool,
    pub soft_wrap: bool,
    pub syntax_highlighting: bool,
    pub syntax_highlighting_max_file_size_kib: usize,
    pub autosave_interval_seconds: i64,
    pub header_style: EditorHeaderStyle,
    pub check_modified_files_on_quit: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            indent_width: 4,
            insert_spaces: true,
            default_line_ending: LineEnding::Lf,
            undo_limit_mib: DEFAULT_UNDO_LIMIT_MIB,
            line_numbers: true,
            scrollbar: true,
            soft_wrap: false,
            syntax_highlighting: true,
            syntax_highlighting_max_file_size_kib: DEFAULT_SYNTAX_HIGHLIGHTING_MAX_FILE_SIZE_KIB,
            autosave_interval_seconds: DEFAULT_AUTOSAVE_INTERVAL_SECONDS,
            header_style: EditorHeaderStyle::Tabs,
            check_modified_files_on_quit: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub minimum_pane_width: u16,
    pub minimum_pane_height: u16,
    pub initial_split_ratio: f32,
    pub vertical_divider_thickness: u16,
    pub horizontal_divider_thickness: u16,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            minimum_pane_width: 12,
            minimum_pane_height: 4,
            initial_split_ratio: 0.5,
            vertical_divider_thickness: 1,
            horizontal_divider_thickness: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ExplorerConfig {
    pub visible: bool,
    pub width: u16,
    pub exclusions: Vec<String>,
    pub refresh_interval_seconds: u64,
    pub divider_thickness: u16,
}

impl Default for ExplorerConfig {
    fn default() -> Self {
        Self {
            visible: true,
            width: 24,
            exclusions: vec![".git".to_owned(), ".git/**".to_owned()],
            refresh_interval_seconds: 5,
            divider_thickness: 1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct MouseConfig {
    pub enabled: bool,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ClipboardConfig {
    pub osc52: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self { osc52: true }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub shell: Option<PathBuf>,
    pub arguments: Vec<String>,
    pub scrollback_lines: usize,
    pub allow_child_osc52_write: bool,
    pub leader: String,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: None,
            arguments: Vec::new(),
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            allow_child_osc52_write: false,
            leader: "ctrl+b".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct UiConfig {
    pub bottom_terminal_visible: bool,
    pub bottom_terminal_height: u16,
    pub terminal_divider_thickness: u16,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub active: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            active: "default".to_owned(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            bottom_terminal_visible: false,
            bottom_terminal_height: 6,
            terminal_divider_thickness: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Config {
    pub editor: EditorConfig,
    pub workspace: WorkspaceConfig,
    pub explorer: ExplorerConfig,
    pub mouse: MouseConfig,
    pub clipboard: ClipboardConfig,
    pub terminal: TerminalConfig,
    pub ui: UiConfig,
    pub theme: ThemeConfig,
    pub keybindings: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            editor: EditorConfig::default(),
            workspace: WorkspaceConfig::default(),
            explorer: ExplorerConfig::default(),
            mouse: MouseConfig::default(),
            clipboard: ClipboardConfig::default(),
            terminal: TerminalConfig::default(),
            ui: UiConfig::default(),
            theme: ThemeConfig::default(),
            keybindings: default_keybindings(),
        }
    }
}

impl Config {
    pub fn get_setting_value(&self, dotted_key: &str) -> String {
        match dotted_key {
            "editor.indent_width" => self.editor.indent_width.to_string(),
            "editor.insert_spaces" => self.editor.insert_spaces.to_string(),
            "editor.default_line_ending" => match self.editor.default_line_ending {
                LineEnding::Lf => "lf".to_owned(),
                LineEnding::Crlf => "crlf".to_owned(),
            },
            "editor.undo_limit_mib" => self.editor.undo_limit_mib.to_string(),
            "editor.line_numbers" => self.editor.line_numbers.to_string(),
            "editor.scrollbar" => self.editor.scrollbar.to_string(),
            "editor.soft_wrap" => self.editor.soft_wrap.to_string(),
            "editor.syntax_highlighting" => self.editor.syntax_highlighting.to_string(),
            "editor.syntax_highlighting_max_file_size_kib" => self
                .editor
                .syntax_highlighting_max_file_size_kib
                .to_string(),
            "editor.autosave_interval_seconds" => self.editor.autosave_interval_seconds.to_string(),
            "editor.header_style" => match self.editor.header_style {
                EditorHeaderStyle::Tabs => "tabs".to_owned(),
                EditorHeaderStyle::Breadcrumbs => "breadcrumbs".to_owned(),
            },
            "editor.check_modified_files_on_quit" => {
                self.editor.check_modified_files_on_quit.to_string()
            }
            "workspace.minimum_pane_width" => self.workspace.minimum_pane_width.to_string(),
            "workspace.minimum_pane_height" => self.workspace.minimum_pane_height.to_string(),
            "workspace.initial_split_ratio" => self.workspace.initial_split_ratio.to_string(),
            "workspace.vertical_divider_thickness" => {
                self.workspace.vertical_divider_thickness.to_string()
            }
            "workspace.horizontal_divider_thickness" => {
                self.workspace.horizontal_divider_thickness.to_string()
            }
            "explorer.visible" => self.explorer.visible.to_string(),
            "explorer.width" => self.explorer.width.to_string(),
            "explorer.exclusions" => self.explorer.exclusions.join(", "),
            "explorer.refresh_interval_seconds" => {
                self.explorer.refresh_interval_seconds.to_string()
            }
            "explorer.divider_thickness" => self.explorer.divider_thickness.to_string(),
            "mouse.enabled" => self.mouse.enabled.to_string(),
            "clipboard.osc52" => self.clipboard.osc52.to_string(),
            "terminal.shell" => self
                .terminal
                .shell
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            "terminal.arguments" => self.terminal.arguments.join(", "),
            "terminal.scrollback_lines" => self.terminal.scrollback_lines.to_string(),
            "terminal.allow_child_osc52_write" => self.terminal.allow_child_osc52_write.to_string(),
            "terminal.leader" => self.terminal.leader.clone(),
            "ui.bottom_terminal_visible" => self.ui.bottom_terminal_visible.to_string(),
            "ui.bottom_terminal_height" => self.ui.bottom_terminal_height.to_string(),
            "ui.terminal_divider_thickness" => self.ui.terminal_divider_thickness.to_string(),
            "theme.active" => self.theme.active.clone(),
            key => self.keybindings.get(key).cloned().unwrap_or_default(),
        }
    }
}

fn default_keybindings() -> BTreeMap<String, String> {
    [
        ("ctrl+q", "quit"),
        ("ctrl+s", "save"),
        ("ctrl+o", "open"),
        ("ctrl+w", "close-pane"),
        ("ctrl+z", "undo"),
        ("ctrl+y", "redo"),
        ("ctrl+g", "settings"),
        ("ctrl+t", "toggle-terminal"),
    ]
    .into_iter()
    .map(|(key, command)| (key.to_owned(), command.to_owned()))
    .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigScope {
    Default,
    User,
    Workspace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigDiagnostic {
    pub scope: ConfigScope,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct LoadedConfig {
    pub effective: Config,
    pub user_path: PathBuf,
    pub workspace_path: PathBuf,
    pub diagnostics: Vec<ConfigDiagnostic>,
    pub provenance: BTreeMap<String, ConfigScope>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialConfig {
    editor: Option<PartialEditorConfig>,
    workspace: Option<PartialWorkspaceConfig>,
    explorer: Option<PartialExplorerConfig>,
    mouse: Option<PartialMouseConfig>,
    clipboard: Option<PartialClipboardConfig>,
    terminal: Option<PartialTerminalConfig>,
    ui: Option<PartialUiConfig>,
    theme: Option<PartialThemeConfig>,
    keybindings: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialEditorConfig {
    indent_width: Option<u8>,
    insert_spaces: Option<bool>,
    default_line_ending: Option<LineEnding>,
    undo_limit_mib: Option<usize>,
    line_numbers: Option<bool>,
    scrollbar: Option<bool>,
    soft_wrap: Option<bool>,
    syntax_highlighting: Option<bool>,
    syntax_highlighting_max_file_size_kib: Option<usize>,
    autosave_interval_seconds: Option<i64>,
    header_style: Option<EditorHeaderStyle>,
    check_modified_files_on_quit: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialWorkspaceConfig {
    minimum_pane_width: Option<u16>,
    minimum_pane_height: Option<u16>,
    initial_split_ratio: Option<f32>,
    vertical_divider_thickness: Option<u16>,
    horizontal_divider_thickness: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialExplorerConfig {
    visible: Option<bool>,
    width: Option<u16>,
    exclusions: Option<Vec<String>>,
    refresh_interval_seconds: Option<u64>,
    divider_thickness: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialMouseConfig {
    enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialClipboardConfig {
    osc52: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialTerminalConfig {
    shell: Option<PathBuf>,
    arguments: Option<Vec<String>>,
    scrollback_lines: Option<usize>,
    allow_child_osc52_write: Option<bool>,
    leader: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialUiConfig {
    bottom_terminal_visible: Option<bool>,
    bottom_terminal_height: Option<u16>,
    terminal_divider_thickness: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialThemeConfig {
    active: Option<String>,
}

impl LoadedConfig {
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let user_path = user_config_path()?;
        let workspace_path = workspace_root.join(CONFIG_RELATIVE_PATH);
        Self::load_paths(user_path, workspace_path)
    }

    fn load_paths(user_path: PathBuf, workspace_path: PathBuf) -> Result<Self> {
        let mut diagnostics = Vec::new();
        let user = read_scope(&user_path, ConfigScope::User, &mut diagnostics)?;
        let workspace = read_scope(&workspace_path, ConfigScope::Workspace, &mut diagnostics)?;
        let mut effective = Config::default();
        let mut provenance = default_provenance();
        apply_scope(
            &mut effective,
            user,
            ConfigScope::User,
            &mut provenance,
            &mut diagnostics,
        );
        apply_scope(
            &mut effective,
            workspace,
            ConfigScope::Workspace,
            &mut provenance,
            &mut diagnostics,
        );
        validate(&mut effective, &mut diagnostics);
        Ok(Self {
            effective,
            user_path,
            workspace_path,
            diagnostics,
            provenance,
        })
    }

    pub fn ensure_raw_config(&self, scope: ConfigScope) -> Result<PathBuf> {
        let path = match scope {
            ConfigScope::User => &self.user_path,
            ConfigScope::Workspace => &self.workspace_path,
            ConfigScope::Default => {
                return Err(AppError::Arguments(
                    "default settings do not have a writable raw configuration".to_owned(),
                ));
            }
        };
        if path.exists() {
            return Ok(path.clone());
        }
        let parent = path.parent().ok_or_else(|| {
            AppError::Arguments(format!("{} has no parent directory", path.display()))
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            AppError::io(format!("could not create {}", parent.display()), error)
        })?;
        let starter =
            toml::to_string_pretty(&Config::default()).map_err(|error| AppError::Config {
                path: path.clone(),
                message: error.to_string(),
            })?;
        fs::write(path, starter)
            .map_err(|error| AppError::io(format!("could not create {}", path.display()), error))?;
        Ok(path.clone())
    }

    pub fn reload(&mut self) -> Result<()> {
        *self = Self::load_paths(self.user_path.clone(), self.workspace_path.clone())?;
        Ok(())
    }

    pub fn reset_setting(&mut self, scope: ConfigScope, dotted_key: &str) -> Result<()> {
        let path = match scope {
            ConfigScope::User => self.user_path.clone(),
            ConfigScope::Workspace => self.workspace_path.clone(),
            ConfigScope::Default => {
                return Err(AppError::Arguments(
                    "default settings cannot be reset".to_owned(),
                ));
            }
        };
        if scope == ConfigScope::Workspace
            && (dotted_key.starts_with("terminal.")
                || dotted_key == "editor.autosave_interval_seconds")
        {
            return Err(AppError::Arguments(
                "this setting is restricted to user configuration".to_owned(),
            ));
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(AppError::io(
                    format!("could not read {}", path.display()),
                    error,
                ));
            }
        };
        let mut document = text
            .parse::<DocumentMut>()
            .map_err(|error| AppError::Config {
                path: path.clone(),
                message: error.to_string(),
            })?;
        remove_dotted_key(&mut document, dotted_key)?;
        atomic_config_write(&path, document.to_string().as_bytes())?;
        *self = Self::load_paths(self.user_path.clone(), self.workspace_path.clone())?;
        Ok(())
    }

    pub fn set_theme_active(&mut self, scope: ConfigScope, theme_id: &str) -> Result<()> {
        if scope == ConfigScope::Default {
            return Err(AppError::Arguments(
                "the default theme selection is not writable".to_owned(),
            ));
        }
        if theme_id.trim().is_empty() {
            return Err(AppError::Arguments(
                "theme identifier must not be empty".to_owned(),
            ));
        }
        let path = match scope {
            ConfigScope::User => self.user_path.clone(),
            ConfigScope::Workspace => self.workspace_path.clone(),
            ConfigScope::Default => unreachable!("default scope returned above"),
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(AppError::io(
                    format!("could not read {}", path.display()),
                    error,
                ));
            }
        };
        let mut document = if text.is_empty() {
            DocumentMut::new()
        } else {
            text.parse::<DocumentMut>()
                .map_err(|error| AppError::Config {
                    path: path.clone(),
                    message: error.to_string(),
                })?
        };
        let theme = document
            .as_table_mut()
            .entry("theme")
            .or_insert_with(|| Item::Table(Table::new()));
        let table = theme.as_table_mut().ok_or_else(|| AppError::Config {
            path: path.clone(),
            message: "theme must be a table".to_owned(),
        })?;
        table["active"] = value(theme_id);
        atomic_config_write(&path, document.to_string().as_bytes())?;
        *self = Self::load_paths(self.user_path.clone(), self.workspace_path.clone())?;
        Ok(())
    }

    pub fn set_setting_from_string(
        &mut self,
        scope: ConfigScope,
        dotted_key: &str,
        value: &str,
    ) -> Result<()> {
        if scope == ConfigScope::Default {
            return Err(AppError::Arguments(
                "default settings cannot be modified".to_owned(),
            ));
        }
        if (dotted_key.starts_with("terminal.") || dotted_key == "editor.autosave_interval_seconds")
            && scope == ConfigScope::Workspace
        {
            return Err(AppError::Arguments(
                "this setting is restricted to user configuration".to_owned(),
            ));
        }
        let path = match scope {
            ConfigScope::User => self.user_path.clone(),
            ConfigScope::Workspace => self.workspace_path.clone(),
            ConfigScope::Default => unreachable!(),
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(AppError::io(
                    format!("could not read {}", path.display()),
                    error,
                ));
            }
        };
        let mut document = if text.is_empty() {
            DocumentMut::new()
        } else {
            text.parse::<DocumentMut>()
                .map_err(|error| AppError::Config {
                    path: path.clone(),
                    message: error.to_string(),
                })?
        };
        let value = Item::Value(parsed_setting_value(dotted_key, value)?);
        let segments: Vec<&str> = dotted_key.split('.').collect();
        let mut table = document.as_table_mut();
        for segment in &segments[..segments.len() - 1] {
            let item = table
                .entry(segment)
                .or_insert_with(|| Item::Table(Table::new()));
            table = item.as_table_mut().ok_or_else(|| AppError::Config {
                path: path.clone(),
                message: format!("`{segment}` must be a table"),
            })?;
        }
        let leaf = *segments
            .last()
            .ok_or_else(|| AppError::Arguments("setting key must not be empty".to_owned()))?;
        table[leaf] = value;
        atomic_config_write(&path, document.to_string().as_bytes())?;
        *self = Self::load_paths(self.user_path.clone(), self.workspace_path.clone())?;
        Ok(())
    }

    pub fn rebind_keybinding(
        &mut self,
        scope: ConfigScope,
        current_key: &str,
        replacement_key: &str,
    ) -> Result<()> {
        let command = self
            .effective
            .keybindings
            .get(current_key)
            .cloned()
            .ok_or_else(|| {
                AppError::Arguments(format!("no command is bound to `{current_key}`"))
            })?;
        self.set_setting_from_string(scope, &format!("keybindings.{current_key}"), "disabled")?;
        self.set_setting_from_string(scope, &format!("keybindings.{replacement_key}"), &command)
    }
}

fn parsed_setting_value(dotted_key: &str, value: &str) -> Result<Value> {
    let trimmed = value.trim();
    match dotted_key {
        "editor.insert_spaces"
        | "editor.line_numbers"
        | "editor.scrollbar"
        | "editor.soft_wrap"
        | "editor.syntax_highlighting"
        | "editor.check_modified_files_on_quit"
        | "explorer.visible"
        | "mouse.enabled"
        | "clipboard.osc52"
        | "terminal.allow_child_osc52_write"
        | "ui.bottom_terminal_visible" => match trimmed {
            "true" | "yes" | "1" => Ok(Value::from(true)),
            "false" | "no" | "0" => Ok(Value::from(false)),
            _ => Err(AppError::Arguments(format!(
                "expected boolean value for `{dotted_key}`, got `{trimmed}`"
            ))),
        },
        "editor.indent_width" => trimmed.parse::<i64>().map(Value::from).map_err(|_| {
            AppError::Arguments(format!(
                "expected number for `{dotted_key}`, got `{trimmed}`"
            ))
        }),
        "editor.undo_limit_mib"
        | "editor.syntax_highlighting_max_file_size_kib"
        | "editor.autosave_interval_seconds"
        | "terminal.scrollback_lines" => trimmed.parse::<i64>().map(Value::from).map_err(|_| {
            AppError::Arguments(format!(
                "expected number for `{dotted_key}`, got `{trimmed}`"
            ))
        }),
        "workspace.minimum_pane_width"
        | "workspace.minimum_pane_height"
        | "workspace.vertical_divider_thickness"
        | "workspace.horizontal_divider_thickness"
        | "explorer.width"
        | "explorer.divider_thickness"
        | "ui.bottom_terminal_height"
        | "ui.terminal_divider_thickness" => {
            let parsed = trimmed.parse::<i64>().map_err(|_| {
                AppError::Arguments(format!(
                    "expected number for `{dotted_key}`, got `{trimmed}`"
                ))
            })?;
            if [
                "workspace.vertical_divider_thickness",
                "workspace.horizontal_divider_thickness",
                "explorer.divider_thickness",
                "ui.terminal_divider_thickness",
            ]
            .contains(&dotted_key)
                && !(0..=10).contains(&parsed)
            {
                return Err(AppError::Arguments(format!(
                    "`{dotted_key}` must be between 0 and 10, got `{trimmed}`"
                )));
            }
            Ok(Value::from(parsed))
        }
        "workspace.initial_split_ratio" => trimmed.parse::<f64>().map(Value::from).map_err(|_| {
            AppError::Arguments(format!(
                "expected number for `{dotted_key}`, got `{trimmed}`"
            ))
        }),
        "explorer.refresh_interval_seconds" => {
            trimmed.parse::<i64>().map(Value::from).map_err(|_| {
                AppError::Arguments(format!(
                    "expected number for `{dotted_key}`, got `{trimmed}`"
                ))
            })
        }
        "editor.default_line_ending" => match trimmed {
            "lf" | "LF" => Ok(Value::from("lf")),
            "crlf" | "CRLF" => Ok(Value::from("crlf")),
            _ => Ok(Value::from(trimmed)),
        },
        "editor.header_style" => match trimmed {
            "tabs" | "Tabs" => Ok(Value::from("tabs")),
            "breadcrumbs" | "Breadcrumbs" => Ok(Value::from("breadcrumbs")),
            _ => Ok(Value::from(trimmed)),
        },
        "explorer.exclusions" | "terminal.arguments" => {
            let mut array = Array::new();
            for item in trimmed
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                array.push(item);
            }
            Ok(Value::Array(array))
        }
        _ => Ok(Value::from(trimmed)),
    }
}

fn remove_dotted_key(document: &mut DocumentMut, dotted_key: &str) -> Result<()> {
    let mut segments = dotted_key.split('.').collect::<Vec<_>>();
    let leaf = segments
        .pop()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| AppError::Arguments("setting key must not be empty".to_owned()))?;
    let mut table = document.as_table_mut();
    for segment in segments {
        let Some(item) = table.get_mut(segment) else {
            return Ok(());
        };
        let Some(next) = item.as_table_mut() else {
            return Err(AppError::Arguments(format!(
                "`{segment}` is not a configuration table"
            )));
        };
        table = next;
    }
    table.remove(leaf);
    Ok(())
}

fn atomic_config_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        AppError::Arguments(format!("{} has no parent directory", path.display()))
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| AppError::io(format!("could not create {}", parent.display()), error))?;
    let counter = CONFIG_TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let temporary = parent.join(format!(
        ".{file_name}.tinlynk-{}-{counter}.tmp",
        std::process::id()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            AppError::io(format!("could not create {}", temporary.display()), error)
        })?;
    let write_result = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(AppError::io(
            format!("could not safely replace {}", path.display()),
            error,
        ));
    }
    Ok(())
}

fn user_config_path() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(CONFIG_RELATIVE_PATH))
        .ok_or_else(|| {
            AppError::Arguments("HOME is not set; user configuration is unavailable".to_owned())
        })
}

fn read_scope(
    path: &Path,
    scope: ConfigScope,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Result<PartialConfig> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PartialConfig::default());
        }
        Err(error) => {
            return Err(AppError::io(
                format!("could not read {}", path.display()),
                error,
            ));
        }
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            diagnostics.push(ConfigDiagnostic {
                scope,
                message: format!("could not parse {}: {error}", path.display()),
            });
            return Ok(PartialConfig::default());
        }
    };
    warn_unknown_keys(&value, scope, diagnostics);
    Ok(parse_partial_config(&value, path, scope, diagnostics))
}

fn parse_partial_config(
    value: &toml::Value,
    path: &Path,
    scope: ConfigScope,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> PartialConfig {
    let Some(root) = value.as_table() else {
        diagnostics.push(ConfigDiagnostic {
            scope,
            message: format!("{} must contain configuration tables", path.display()),
        });
        return PartialConfig::default();
    };
    PartialConfig {
        editor: config_table(root, "editor", path, scope, diagnostics).map(|table| {
            PartialEditorConfig {
                indent_width: config_value(
                    table,
                    "indent_width",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
                insert_spaces: config_value(
                    table,
                    "insert_spaces",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
                default_line_ending: config_value(
                    table,
                    "default_line_ending",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
                undo_limit_mib: config_value(
                    table,
                    "undo_limit_mib",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
                line_numbers: config_value(
                    table,
                    "line_numbers",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
                scrollbar: config_value(table, "scrollbar", "editor", path, scope, diagnostics),
                soft_wrap: config_value(table, "soft_wrap", "editor", path, scope, diagnostics),
                syntax_highlighting: config_value(
                    table,
                    "syntax_highlighting",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
                syntax_highlighting_max_file_size_kib: config_value(
                    table,
                    "syntax_highlighting_max_file_size_kib",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
                autosave_interval_seconds: config_value(
                    table,
                    "autosave_interval_seconds",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
                header_style: config_value(table, "header_style", "editor", path, scope, diagnostics),
                check_modified_files_on_quit: config_value(
                    table,
                    "check_modified_files_on_quit",
                    "editor",
                    path,
                    scope,
                    diagnostics,
                ),
            }
        }),
        workspace: config_table(root, "workspace", path, scope, diagnostics).map(|table| {
            PartialWorkspaceConfig {
                minimum_pane_width: config_value(
                    table,
                    "minimum_pane_width",
                    "workspace",
                    path,
                    scope,
                    diagnostics,
                ),
                minimum_pane_height: config_value(
                    table,
                    "minimum_pane_height",
                    "workspace",
                    path,
                    scope,
                    diagnostics,
                ),
                initial_split_ratio: config_value(
                    table,
                    "initial_split_ratio",
                    "workspace",
                    path,
                    scope,
                    diagnostics,
                ),
                vertical_divider_thickness: config_value(
                    table,
                    "vertical_divider_thickness",
                    "workspace",
                    path,
                    scope,
                    diagnostics,
                ),
                horizontal_divider_thickness: config_value(
                    table,
                    "horizontal_divider_thickness",
                    "workspace",
                    path,
                    scope,
                    diagnostics,
                ),
            }
        }),
        explorer: config_table(root, "explorer", path, scope, diagnostics).map(|table| {
            PartialExplorerConfig {
                visible: config_value(table, "visible", "explorer", path, scope, diagnostics),
                width: config_value(table, "width", "explorer", path, scope, diagnostics),
                exclusions: config_value(table, "exclusions", "explorer", path, scope, diagnostics),
                refresh_interval_seconds: config_value(
                    table,
                    "refresh_interval_seconds",
                    "explorer",
                    path,
                    scope,
                    diagnostics,
                ),
                divider_thickness: config_value(
                    table,
                    "divider_thickness",
                    "explorer",
                    path,
                    scope,
                    diagnostics,
                ),
            }
        }),
        mouse: config_table(root, "mouse", path, scope, diagnostics).map(|table| {
            PartialMouseConfig {
                enabled: config_value(table, "enabled", "mouse", path, scope, diagnostics),
            }
        }),
        clipboard: config_table(root, "clipboard", path, scope, diagnostics).map(|table| {
            PartialClipboardConfig {
                osc52: config_value(table, "osc52", "clipboard", path, scope, diagnostics),
            }
        }),
        terminal: config_table(root, "terminal", path, scope, diagnostics).map(|table| {
            PartialTerminalConfig {
                shell: config_value(table, "shell", "terminal", path, scope, diagnostics),
                arguments: config_value(table, "arguments", "terminal", path, scope, diagnostics),
                scrollback_lines: config_value(
                    table,
                    "scrollback_lines",
                    "terminal",
                    path,
                    scope,
                    diagnostics,
                ),
                allow_child_osc52_write: config_value(
                    table,
                    "allow_child_osc52_write",
                    "terminal",
                    path,
                    scope,
                    diagnostics,
                ),
                leader: config_value(table, "leader", "terminal", path, scope, diagnostics),
            }
        }),
        ui: config_table(root, "ui", path, scope, diagnostics).map(|table| PartialUiConfig {
            bottom_terminal_visible: config_value(
                table,
                "bottom_terminal_visible",
                "ui",
                path,
                scope,
                diagnostics,
            ),
            bottom_terminal_height: config_value(
                table,
                "bottom_terminal_height",
                "ui",
                path,
                scope,
                diagnostics,
            ),
            terminal_divider_thickness: config_value(
                table,
                "terminal_divider_thickness",
                "ui",
                path,
                scope,
                diagnostics,
            ),
        }),
        theme: config_table(root, "theme", path, scope, diagnostics).map(|table| {
            PartialThemeConfig {
                active: config_value(table, "active", "theme", path, scope, diagnostics),
            }
        }),
        keybindings: config_table(root, "keybindings", path, scope, diagnostics).map(|table| {
            table
                .iter()
                .filter_map(|(key, value)| match value.clone().try_into::<String>() {
                    Ok(command) if valid_keybinding(key, &command) => Some((key.clone(), command)),
                    Ok(command) => {
                        diagnostics.push(ConfigDiagnostic {
                            scope,
                            message: format!(
                                "invalid known value `keybindings.{key}` in {}: `{command}` is not a supported non-empty command; its default or lower-scope value was used",
                                path.display()
                            ),
                        });
                        None
                    }
                    Err(error) => {
                        invalid_config_value(
                            path,
                            scope,
                            &format!("keybindings.{key}"),
                            &error,
                            diagnostics,
                        );
                        None
                    }
                })
                .collect()
        }),
    }
}

fn config_table<'a>(
    root: &'a toml::Table,
    key: &str,
    path: &Path,
    scope: ConfigScope,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<&'a toml::Table> {
    let value = root.get(key)?;
    match value.as_table() {
        Some(table) => Some(table),
        None => {
            diagnostics.push(ConfigDiagnostic {
                scope,
                message: format!(
                    "invalid known value `{key}` in {}: expected a table",
                    path.display()
                ),
            });
            None
        }
    }
}

fn config_value<T: DeserializeOwned>(
    table: &toml::Table,
    key: &str,
    table_name: &str,
    path: &Path,
    scope: ConfigScope,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) -> Option<T> {
    let value = table.get(key)?;
    match value.clone().try_into() {
        Ok(value) => Some(value),
        Err(error) => {
            invalid_config_value(
                path,
                scope,
                &format!("{table_name}.{key}"),
                &error,
                diagnostics,
            );
            None
        }
    }
}

fn invalid_config_value(
    path: &Path,
    scope: ConfigScope,
    key: &str,
    error: &toml::de::Error,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) {
    diagnostics.push(ConfigDiagnostic {
        scope,
        message: format!(
            "invalid known value `{key}` in {}: {error}; its default or lower-scope value was used",
            path.display()
        ),
    });
}

fn warn_unknown_keys(
    value: &toml::Value,
    scope: ConfigScope,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) {
    const KNOWN: &[&str] = &[
        "editor",
        "workspace",
        "explorer",
        "mouse",
        "clipboard",
        "terminal",
        "ui",
        "theme",
        "keybindings",
    ];
    let Some(table) = value.as_table() else {
        return;
    };
    for key in table.keys().filter(|key| !KNOWN.contains(&key.as_str())) {
        diagnostics.push(ConfigDiagnostic {
            scope,
            message: format!("unknown configuration table or key `{key}`"),
        });
    }
}

fn apply_scope(
    config: &mut Config,
    partial: PartialConfig,
    scope: ConfigScope,
    provenance: &mut BTreeMap<String, ConfigScope>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) {
    if let Some(editor) = partial.editor {
        assign(&mut config.editor.indent_width, editor.indent_width);
        assign(&mut config.editor.insert_spaces, editor.insert_spaces);
        assign(
            &mut config.editor.default_line_ending,
            editor.default_line_ending,
        );
        assign(&mut config.editor.undo_limit_mib, editor.undo_limit_mib);
        assign(&mut config.editor.line_numbers, editor.line_numbers);
        assign(&mut config.editor.scrollbar, editor.scrollbar);
        assign(&mut config.editor.soft_wrap, editor.soft_wrap);
        assign(
            &mut config.editor.syntax_highlighting,
            editor.syntax_highlighting,
        );
        assign(
            &mut config.editor.syntax_highlighting_max_file_size_kib,
            editor.syntax_highlighting_max_file_size_kib,
        );
        if scope == ConfigScope::Workspace && editor.autosave_interval_seconds.is_some() {
            diagnostics.push(ConfigDiagnostic {
                scope,
                message: "workspace autosave settings are restricted and were ignored".to_owned(),
            });
        } else {
            assign(
                &mut config.editor.autosave_interval_seconds,
                editor.autosave_interval_seconds,
            );
        }
        assign(&mut config.editor.header_style, editor.header_style);
        assign(
            &mut config.editor.check_modified_files_on_quit,
            editor.check_modified_files_on_quit,
        );
        provenance.insert("editor".to_owned(), scope);
    }
    if let Some(workspace) = partial.workspace {
        assign(
            &mut config.workspace.minimum_pane_width,
            workspace.minimum_pane_width,
        );
        assign(
            &mut config.workspace.minimum_pane_height,
            workspace.minimum_pane_height,
        );
        assign(
            &mut config.workspace.initial_split_ratio,
            workspace.initial_split_ratio,
        );
        assign(
            &mut config.workspace.vertical_divider_thickness,
            workspace.vertical_divider_thickness,
        );
        assign(
            &mut config.workspace.horizontal_divider_thickness,
            workspace.horizontal_divider_thickness,
        );
        provenance.insert("workspace".to_owned(), scope);
    }
    if let Some(explorer) = partial.explorer {
        assign(&mut config.explorer.visible, explorer.visible);
        assign(&mut config.explorer.width, explorer.width);
        assign(&mut config.explorer.exclusions, explorer.exclusions);
        assign(
            &mut config.explorer.refresh_interval_seconds,
            explorer.refresh_interval_seconds,
        );
        assign(
            &mut config.explorer.divider_thickness,
            explorer.divider_thickness,
        );
        provenance.insert("explorer".to_owned(), scope);
    }
    if let Some(mouse) = partial.mouse {
        assign(&mut config.mouse.enabled, mouse.enabled);
        provenance.insert("mouse".to_owned(), scope);
    }
    if let Some(clipboard) = partial.clipboard {
        assign(&mut config.clipboard.osc52, clipboard.osc52);
        provenance.insert("clipboard".to_owned(), scope);
    }
    if let Some(ui) = partial.ui {
        assign(
            &mut config.ui.bottom_terminal_visible,
            ui.bottom_terminal_visible,
        );
        assign(
            &mut config.ui.bottom_terminal_height,
            ui.bottom_terminal_height,
        );
        assign(
            &mut config.ui.terminal_divider_thickness,
            ui.terminal_divider_thickness,
        );
        provenance.insert("ui".to_owned(), scope);
    }
    if let Some(theme) = partial.theme
        && let Some(active) = theme.active
    {
        config.theme.active = active;
        provenance.insert("theme".to_owned(), scope);
    }
    if let Some(bindings) = partial.keybindings {
        for (key, command) in bindings {
            if command == "disabled" {
                config.keybindings.remove(&key);
            } else {
                config.keybindings.insert(key, command);
            }
        }
        provenance.insert("keybindings".to_owned(), scope);
    }
    if let Some(terminal) = partial.terminal {
        if scope == ConfigScope::Workspace {
            diagnostics.push(ConfigDiagnostic {
                scope,
                message: "workspace terminal settings are restricted and were ignored".to_owned(),
            });
        } else {
            assign(&mut config.terminal.shell, terminal.shell.map(Some));
            assign(&mut config.terminal.arguments, terminal.arguments);
            assign(
                &mut config.terminal.scrollback_lines,
                terminal.scrollback_lines,
            );
            assign(
                &mut config.terminal.allow_child_osc52_write,
                terminal.allow_child_osc52_write,
            );
            assign(&mut config.terminal.leader, terminal.leader);
            provenance.insert("terminal".to_owned(), scope);
        }
    }
}

fn assign<T>(target: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *target = value;
    }
}

fn validate(config: &mut Config, diagnostics: &mut Vec<ConfigDiagnostic>) {
    let defaults = Config::default();
    let mut fallback = |condition: bool, message: &str| {
        if condition {
            diagnostics.push(ConfigDiagnostic {
                scope: ConfigScope::Default,
                message: message.to_owned(),
            });
        }
        condition
    };
    if fallback(
        !(1..=16).contains(&config.editor.indent_width),
        "editor.indent_width must be between 1 and 16; the default was used",
    ) {
        config.editor.indent_width = defaults.editor.indent_width;
    }
    if fallback(
        config.editor.undo_limit_mib == 0,
        "editor.undo_limit_mib must be greater than zero; the default was used",
    ) {
        config.editor.undo_limit_mib = defaults.editor.undo_limit_mib;
    }
    if fallback(
        !(0.1..=0.9).contains(&config.workspace.initial_split_ratio),
        "workspace.initial_split_ratio must be between 0.1 and 0.9; the default was used",
    ) {
        config.workspace.initial_split_ratio = defaults.workspace.initial_split_ratio;
    }
    if fallback(
        config.workspace.vertical_divider_thickness > 10,
        "workspace.vertical_divider_thickness must be between 0 and 10; the default was used",
    ) {
        config.workspace.vertical_divider_thickness = defaults.workspace.vertical_divider_thickness;
    }
    if fallback(
        config.workspace.horizontal_divider_thickness > 10,
        "workspace.horizontal_divider_thickness must be between 0 and 10; the default was used",
    ) {
        config.workspace.horizontal_divider_thickness =
            defaults.workspace.horizontal_divider_thickness;
    }
    if fallback(
        config.explorer.divider_thickness > 10,
        "explorer.divider_thickness must be between 0 and 10; the default was used",
    ) {
        config.explorer.divider_thickness = defaults.explorer.divider_thickness;
    }
    if fallback(
        config.ui.terminal_divider_thickness > 10,
        "ui.terminal_divider_thickness must be between 0 and 10; the default was used",
    ) {
        config.ui.terminal_divider_thickness = defaults.ui.terminal_divider_thickness;
    }
    if fallback(
        config
            .explorer
            .exclusions
            .iter()
            .any(|pattern| globset::Glob::new(pattern).is_err()),
        "explorer.exclusions contains an invalid glob; the defaults were used",
    ) {
        config.explorer.exclusions = defaults.explorer.exclusions;
    }
    if fallback(
        config.terminal.scrollback_lines == 0,
        "terminal.scrollback_lines must be greater than zero; the default was used",
    ) {
        config.terminal.scrollback_lines = defaults.terminal.scrollback_lines;
    }
    if fallback(
        config.theme.active.trim().is_empty(),
        "theme.active must not be empty; the default was used",
    ) {
        config.theme.active = defaults.theme.active;
    }
    let normalized_leader = crate::input::normalize_key_sequence(&config.terminal.leader);
    if fallback(
        normalized_leader.as_deref().is_none_or(|leader| {
            !leader
                .strip_prefix("ctrl+")
                .is_some_and(|key| key.len() == 1 && key.as_bytes()[0].is_ascii_lowercase())
                || matches!(leader, "ctrl+t" | "ctrl+g")
                || config.keybindings.contains_key(leader)
        }),
        "terminal.leader must be a non-conflicting Ctrl plus letter sequence; the default was used",
    ) {
        config.terminal.leader = defaults.terminal.leader;
    } else if let Some(normalized_leader) = normalized_leader {
        config.terminal.leader = normalized_leader;
    }
}

fn valid_keybinding(sequence: &str, command: &str) -> bool {
    const COMMANDS: &[&str] = &[
        "quit",
        "save",
        "open",
        "close-pane",
        "split-vertical",
        "split-horizontal",
        "undo",
        "redo",
        "settings",
        "toggle-terminal",
        "toggle-explorer",
    ];
    !sequence.trim().is_empty() && (command == "disabled" || COMMANDS.contains(&command))
}

fn default_provenance() -> BTreeMap<String, ConfigScope> {
    [
        "editor",
        "workspace",
        "explorer",
        "mouse",
        "clipboard",
        "terminal",
        "ui",
        "theme",
        "keybindings",
    ]
    .into_iter()
    .map(|key| (key.to_owned(), ConfigScope::Default))
    .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn defaults_are_safe() {
        let config = Config::default();
        assert_eq!(config.editor.undo_limit_mib, 32);
        assert!(!config.editor.soft_wrap);
        assert_eq!(config.editor.header_style, EditorHeaderStyle::Tabs);
        assert!(config.editor.check_modified_files_on_quit);
        assert_eq!(config.explorer.refresh_interval_seconds, 5);
        assert_eq!(config.explorer.divider_thickness, 1);
        assert_eq!(config.workspace.vertical_divider_thickness, 1);
        assert_eq!(config.workspace.horizontal_divider_thickness, 0);
        assert_eq!(config.ui.terminal_divider_thickness, 0);
        assert_eq!(config.terminal.scrollback_lines, 10_000);
        assert!(!config.terminal.allow_child_osc52_write);
    }

    #[test]
    fn divider_thicknesses_can_be_configured_and_mutated() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(
            &user_path,
            "[explorer]\ndivider_thickness = 3\n[workspace]\nvertical_divider_thickness = 2\nhorizontal_divider_thickness = 4\n[ui]\nterminal_divider_thickness = 0\n",
        )
        .unwrap();

        let mut loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();

        assert_eq!(loaded.effective.explorer.divider_thickness, 3);
        assert_eq!(loaded.effective.workspace.vertical_divider_thickness, 2);
        assert_eq!(loaded.effective.workspace.horizontal_divider_thickness, 4);
        assert_eq!(loaded.effective.ui.terminal_divider_thickness, 0);

        loaded
            .set_setting_from_string(ConfigScope::User, "explorer.divider_thickness", "5")
            .unwrap();
        assert_eq!(loaded.effective.explorer.divider_thickness, 5);

        loaded
            .set_setting_from_string(
                ConfigScope::User,
                "workspace.vertical_divider_thickness",
                "4",
            )
            .unwrap();
        assert_eq!(loaded.effective.workspace.vertical_divider_thickness, 4);

        loaded
            .set_setting_from_string(
                ConfigScope::User,
                "workspace.horizontal_divider_thickness",
                "5",
            )
            .unwrap();
        assert_eq!(loaded.effective.workspace.horizontal_divider_thickness, 5);

        loaded
            .set_setting_from_string(ConfigScope::User, "ui.terminal_divider_thickness", "2")
            .unwrap();
        assert_eq!(loaded.effective.ui.terminal_divider_thickness, 2);

        assert!(
            loaded
                .set_setting_from_string(ConfigScope::User, "explorer.divider_thickness", "15")
                .is_err()
        );
    }

    #[test]
    fn header_style_can_be_configured_to_breadcrumbs() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(&user_path, "[editor]\nheader_style = \"breadcrumbs\"\n").unwrap();

        let loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();

        assert_eq!(
            loaded.effective.editor.header_style,
            EditorHeaderStyle::Breadcrumbs
        );
        assert_eq!(loaded.provenance["editor"], ConfigScope::User);
    }

    #[test]
    fn modified_file_quit_check_can_be_configured_and_mutated() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(
            &user_path,
            "[editor]\ncheck_modified_files_on_quit = false\n",
        )
        .unwrap();

        let mut loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();

        assert!(!loaded.effective.editor.check_modified_files_on_quit);
        loaded
            .set_setting_from_string(
                ConfigScope::User,
                "editor.check_modified_files_on_quit",
                "true",
            )
            .unwrap();
        assert!(loaded.effective.editor.check_modified_files_on_quit);
    }

    #[test]
    fn soft_wrap_can_be_enabled_in_editor_configuration() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(&user_path, "[editor]\nsoft_wrap = true\n").unwrap();

        let loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();

        assert!(loaded.effective.editor.soft_wrap);
        assert_eq!(loaded.provenance["editor"], ConfigScope::User);
    }

    #[test]
    fn negative_autosave_interval_can_be_configured() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(&user_path, "[editor]\nautosave_interval_seconds = -1\n").unwrap();

        let loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();

        assert_eq!(loaded.effective.editor.autosave_interval_seconds, -1);
    }

    #[test]
    fn workspace_terminal_settings_are_restricted() {
        let mut config = Config::default();
        let mut diagnostics = Vec::new();
        let partial = PartialConfig {
            terminal: Some(PartialTerminalConfig {
                shell: Some(PathBuf::from("/untrusted/shell")),
                ..PartialTerminalConfig::default()
            }),
            ..PartialConfig::default()
        };
        apply_scope(
            &mut config,
            partial,
            ConfigScope::Workspace,
            &mut default_provenance(),
            &mut diagnostics,
        );
        assert_eq!(config.terminal.shell, None);
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn workspace_bindings_merge_individually() {
        let mut config = Config::default();
        let mut diagnostics = Vec::new();
        let partial = PartialConfig {
            keybindings: Some(BTreeMap::from([(
                "ctrl+s".to_owned(),
                "save-as".to_owned(),
            )])),
            ..PartialConfig::default()
        };
        apply_scope(
            &mut config,
            partial,
            ConfigScope::Workspace,
            &mut default_provenance(),
            &mut diagnostics,
        );
        assert_eq!(config.keybindings["ctrl+s"], "save-as");
        assert!(config.keybindings.contains_key("ctrl+q"));
    }

    #[test]
    fn partial_table_override_preserves_other_values() {
        let mut config = Config::default();
        config.editor.indent_width = 2;
        let partial = PartialConfig {
            editor: Some(PartialEditorConfig {
                line_numbers: Some(false),
                ..PartialEditorConfig::default()
            }),
            ..PartialConfig::default()
        };
        apply_scope(
            &mut config,
            partial,
            ConfigScope::Workspace,
            &mut default_provenance(),
            &mut Vec::new(),
        );
        assert_eq!(config.editor.indent_width, 2);
        assert!(!config.editor.line_numbers);
    }

    #[test]
    fn invalid_value_preserves_valid_siblings() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(
            &user_path,
            "[editor]\nindent_width = \"invalid\"\nline_numbers = false\n[explorer]\nwidth = 31\n",
        )
        .unwrap();
        let loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();
        assert_eq!(loaded.effective.editor.indent_width, 4);
        assert!(!loaded.effective.editor.line_numbers);
        assert_eq!(loaded.effective.explorer.width, 31);
        assert!(
            loaded
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("editor.indent_width"))
        );
    }

    #[test]
    fn invalid_keybinding_preserves_the_lower_scope_binding() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(&user_path, "[keybindings]\n\"ctrl+s\" = \"unknown\"\n").unwrap();
        let loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();
        assert_eq!(loaded.effective.keybindings["ctrl+s"], "save");
        assert!(
            loaded
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("keybindings.ctrl+s"))
        );
    }

    #[test]
    fn terminal_leader_is_normalized_and_conflicts_fall_back_to_default() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(&user_path, "[terminal]\nleader = \"CTRL+A\"\n").unwrap();
        let loaded = LoadedConfig::load_paths(user_path.clone(), workspace_path.clone()).unwrap();
        assert_eq!(loaded.effective.terminal.leader, "ctrl+a");

        fs::write(&user_path, "[terminal]\nleader = \"ctrl+t\"\n").unwrap();
        let loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();
        assert_eq!(loaded.effective.terminal.leader, "ctrl+b");
        assert!(loaded.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("terminal.leader must be a non-conflicting Ctrl plus letter sequence")
        }));
    }

    #[test]
    fn reset_preserves_comments_and_unrelated_values() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(
            &user_path,
            "# retained comment\n[editor]\nindent_width = 2\nline_numbers = false\n",
        )
        .unwrap();
        let mut loaded = LoadedConfig::load_paths(user_path.clone(), workspace_path).unwrap();
        loaded
            .reset_setting(ConfigScope::User, "editor.indent_width")
            .unwrap();
        let text = fs::read_to_string(user_path).unwrap();
        assert!(text.contains("# retained comment"));
        assert!(text.contains("line_numbers = false"));
        assert!(!text.contains("indent_width"));
        assert_eq!(loaded.effective.editor.indent_width, 4);
    }

    #[test]
    fn workspace_theme_selection_overrides_user_and_preserves_comments_when_changed() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(
            &user_path,
            "# retained comment\n[theme]\nactive = \"ocean\"\n",
        )
        .unwrap();
        fs::write(&workspace_path, "[theme]\nactive = \"forest\"\n").unwrap();
        let mut loaded =
            LoadedConfig::load_paths(user_path.clone(), workspace_path.clone()).unwrap();
        assert_eq!(loaded.effective.theme.active, "forest");
        assert_eq!(loaded.provenance["theme"], ConfigScope::Workspace);

        loaded
            .set_theme_active(ConfigScope::User, "midnight")
            .unwrap();
        let text = fs::read_to_string(user_path).unwrap();
        assert!(text.contains("# retained comment"));
        assert!(text.contains("active = \"midnight\""));
        assert_eq!(loaded.effective.theme.active, "forest");
    }

    #[test]
    fn empty_theme_selection_falls_back_to_default() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(&user_path, "[theme]\nactive = \"   \"\n").unwrap();
        let loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();
        assert_eq!(loaded.effective.theme.active, "default");
        assert!(
            loaded
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("theme.active"))
        );
    }

    #[test]
    fn resetting_theme_selection_restores_default() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        fs::write(&user_path, "[theme]\nactive = \"ocean\"\n").unwrap();
        let mut loaded = LoadedConfig::load_paths(user_path, workspace_path).unwrap();
        loaded
            .reset_setting(ConfigScope::User, "theme.active")
            .unwrap();
        assert_eq!(loaded.effective.theme.active, "default");
        assert_eq!(loaded.provenance["theme"], ConfigScope::Default);
    }

    #[test]
    fn rebinding_disables_the_inherited_shortcut() {
        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        let mut loaded = LoadedConfig::load_paths(user_path.clone(), workspace_path).unwrap();

        loaded
            .rebind_keybinding(ConfigScope::User, "ctrl+s", "ctrl+k")
            .unwrap();

        assert!(!loaded.effective.keybindings.contains_key("ctrl+s"));
        assert_eq!(loaded.effective.keybindings["ctrl+k"], "save");
        let text = fs::read_to_string(user_path).unwrap();
        assert!(text.contains("\"ctrl+s\" = \"disabled\""));
        assert!(text.contains("\"ctrl+k\" = \"save\""));
    }

    #[cfg(unix)]
    #[test]
    fn config_write_does_not_follow_the_legacy_temporary_path_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let user_path = directory.path().join("user.toml");
        let workspace_path = directory.path().join("workspace.toml");
        let target = directory.path().join("outside.txt");
        fs::write(&target, "preserve this").unwrap();
        symlink(&target, workspace_path.with_extension("toml.tinlynk.tmp")).unwrap();
        let mut loaded = LoadedConfig::load_paths(user_path, workspace_path.clone()).unwrap();

        loaded
            .set_setting_from_string(ConfigScope::Workspace, "editor.indent_width", "2")
            .unwrap();

        assert_eq!(fs::read_to_string(target).unwrap(), "preserve this");
        assert_eq!(loaded.effective.editor.indent_width, 2);
    }
}
