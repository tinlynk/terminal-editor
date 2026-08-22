# TinLynk Terminal Editor

TinLynk Terminal Editor is a lightweight terminal-native text editor with an integrated terminal for macOS.

It combines Unicode-aware file editing, nested tiled editor panes, a file Explorer, and one PTY-backed terminal session.

## Build and run

Use stable Rust 1.88 or newer.

```sh
cargo build --release
cargo run --release -- [file-or-directory]
```

The installed executable is named `tinlynk-editor`.

The command accepts zero or one path.

- No path opens an empty editor surface rooted at the current directory.
- A file path opens that UTF-8 file.
- A directory opens the file dialog at that directory.
- A missing file path creates a new document that will be written on save.

## Default controls

| Control | Action |
| --- | --- |
| `Ctrl+Q` | Exit, with unsaved-document and live-terminal confirmation. |
| `Ctrl+S` | Save, or open Save As for an untitled document. |
| `Ctrl+O` | Open the path-entry and filesystem-browsing dialog. |
| `Ctrl+W` | Close the focused editor with required confirmation. |
| `Ctrl+B`, then `V` | Split the editor left and right. |
| `Ctrl+B`, then `H` | Split the editor top and bottom. |
| `Ctrl+B`, then Arrow | Focus the nearest editor pane in that direction. |
| `Ctrl+E` | Toggle the Explorer. |
| `Ctrl+T` | Toggle the persistent bottom terminal. |
| `Ctrl+G` | Open the searchable settings view. |
| `Ctrl+Z` and `Ctrl+Y` | Undo and redo. |
| `Ctrl+C`, `Ctrl+X`, and `Ctrl+V` | Copy, cut, and paste with the internal clipboard. |
| `Shift+Arrow`, `Shift+Home`, `Shift+End`, `Shift+Page Up`, and `Shift+Page Down` | Extend the editor selection when reported by the terminal. |
| Mouse click and drag | Focus panes, place the cursor, select text, and resize splitters. |
| Shift+vertical mouse-wheel | Pan long lines horizontally when soft wrapping is disabled. |
| Bottom-border scrollbar | Click or drag to navigate the complete document width when soft wrapping is disabled. |

``Ctrl+` `` and `Ctrl+,` remain accepted when the outer terminal can report those modified punctuation keys, but many terminals cannot encode them distinctly.

The Explorer uses Up and Down to navigate, Enter or Right to expand or open, `R` to refresh, and Escape or Left to return to the workspace. Move above the first file-tree entry and press Enter, or click the Open Editors header, to collapse or expand its document list. `Ctrl+W` closes the active open editor. The workspace row shows only the root folder name, and the file tree refreshes automatically every five seconds by default.

The title row uses `◧` and `◫` for the visible and hidden Explorer, and `⬓` and `⊟` for the visible and hidden bottom Terminal. The muted `◫` is a disabled placeholder for a future right panel, and the far-right stateless `⚙` control opens Settings.

When the terminal reports mouse movement, hovering an available title control shows its action in the status bar.

Long editor lines show `›` in the reserved right-edge cell when content continues beyond the viewport, and the status bar reports `HScroll` while the horizontal viewport is offset. The editor calculates the complete document width when the file opens and updates it after every edit, undo, and redo. When any line exceeds the viewport, a proportional scrollbar appears in the editor's bottom border; click its track or drag its thumb without moving the cursor. Shift plus vertical mouse-wheel also pans when reported by the outer terminal. A native trackpad left or right gesture pans when the outer terminal reports horizontal-wheel events, but some macOS terminal emulators do not forward either gesture. Set `editor.soft_wrap = true` to wrap long lines into visual rows instead; soft wrapping defaults to `false` and disables horizontal panning, the horizontal scrollbar, and overflow markers. Set `editor.header_style = "breadcrumbs"` to switch editor pane headers from tab strips to breadcrumbs.

Editor scrollbars use continuous proportional thumbs with a one-cell minimum. The horizontal thumb uses the theme's foreground-only `icons.scrollbar_horizontal` glyph so it remains visually as slim as the vertical thumb.

Startup without a file shows an empty editor surface. Typing creates one Untitled document, Save As keeps that document identity under its new filename, and `Ctrl+W` removes the focused document from Open Editors.

The Open dialog supports Up and Down selection, mouse-wheel scrolling, Enter or Right to open a file or folder, Left to visit the parent folder, Tab to copy the selected path, Ctrl+Enter to submit a manually edited path, and Escape to cancel. In Save As, type the filename after the initial directory and press Enter to save; use Right to enter the selected folder.

The terminal passes ordinary keyboard and paste input to the child process.

`Ctrl+T` and `Ctrl+G` remain workbench shortcuts while the terminal is focused.

`Ctrl+B` is the default configurable application leader. A pending leader is shown in the status bar and cancels after 1.5 seconds, Escape, paste, mouse input, or a focus change. Editor split and focus chords are inactive while the terminal owns focus, preserving the single-terminal-session model.

- `Ctrl+B`, then `W` terminates the terminal with confirmation.
- `Ctrl+B`, then `Q` exits.
- `Ctrl+B`, then `Page Up` or `Page Down` navigates application scrollback.
- `Ctrl+B`, then `C` copies the application-owned terminal selection.
- `Ctrl+B` twice sends a literal `Ctrl+B` to the child.

## Configuration

User configuration is loaded from `$HOME/.tinlynk/terminal-editor/config.toml`.

Workspace configuration is loaded from `<explorer-root>/.tinlynk/terminal-editor/config.toml`.

Workspace configuration can override editor, workspace, Explorer, mouse, clipboard, UI, and application keybinding values.

Shell selection, shell arguments, and child OSC 52 permission remain user-level settings.

The settings view shows effective values, defaults, and the selected User or Workspace scope.

Use Tab to change scope, Up and Down to select a setting, and `Ctrl+R` to reset it in the selected scope.

Enter opens the raw TOML configuration for ordinary settings and opens the theme picker for `theme.active`.

```toml
[editor]
indent_width = 4
insert_spaces = true
default_line_ending = "lf"
undo_limit_mib = 32
line_numbers = true
scrollbar = true
soft_wrap = false

[workspace]
minimum_pane_width = 12
minimum_pane_height = 4
initial_split_ratio = 0.5

[explorer]
visible = true
width = 24
exclusions = [".git", ".git/**"]
refresh_interval_seconds = 5

[terminal]
scrollback_lines = 10000
allow_child_osc52_write = false
leader = "ctrl+b"

[theme]
active = "default"
```

Unknown configuration keys produce non-blocking diagnostics.

Invalid configuration text is left untouched and valid defaults remain available.

## Themes

The complete default theme is stored in `themes/default.toml` and embedded in the executable, so it remains available without external files.

Additional themes are loaded from `$HOME/.tinlynk/terminal-editor/themes/`.

Each direct `*.toml` file defines a theme whose identifier is its filename without the extension, while nested directories and other file types are ignored.

The `default` identifier is reserved and cannot be replaced by a user theme.

User themes may define only the values they change because omitted values inherit from the embedded default.

```toml
[meta]
name = "Ocean"
schema_version = 1
border_type = "rounded"

[colors]
accent = "#0088CC"
active_border = "#0088CC"
editor_selection = "#264F78"
tab_active_background = "#1E1E1E"
tab_inactive_background = "#2D2D30"
status_mode_background = "#0088CC"

[icons]
modified = "●"
folder_open = "▾"
folder_closed = "▸"
file_default = "•"
```

Colors accept `default`, the sixteen ANSI color names, or `#RRGGBB` values without alpha.

The available color and icon keys are documented by the complete repository default theme. Panel controls have `_visible` and `_hidden` icon values, while the Settings action has one `settings` value. Explorer root actions use `explorer_new_file`, `explorer_new_folder`, and `explorer_refresh`; their defaults are portable ASCII glyphs.

Theme files are limited to 256 KiB, must contain UTF-8 TOML, and cannot change child-process ANSI, indexed, RGB, inverse, or default terminal color behavior.

Set `theme.active` in User or Workspace configuration to select an installed theme, but theme files are loaded only from the user theme directory.

In the theme picker, use Up and Down to select a theme, Enter to save and apply it in the selected scope, `R` to rescan the theme directory, and Escape to cancel.

Saving a theme file from TinLynk Terminal Editor rescans the directory immediately, while invalid live changes retain the previous valid active theme and report a diagnostic.

## Files and recovery

Files are saved through a same-directory temporary file and atomic replacement.

Existing permissions are preserved, and saving through a symlink replaces its resolved target without replacing the symlink.

If a file changes externally before save, TinLynk Terminal Editor offers overwrite or cancellation and leaves the external file untouched until confirmation.

UTF-8 is required, and invalid UTF-8 is reported without modifying the file.

LF and CRLF content is preserved, and inserted lines use the detected document convention.

## Clipboard and terminal security

The internal clipboard is always available for editor operations.

Bracketed paste imports text supplied by the outer terminal.

Child OSC 52 clipboard writes are ignored by default and can be enabled only in user configuration.

Child clipboard reads are never serviced.

OSC 52 support varies by outer terminal, so external clipboard interoperability is not guaranteed.

## Platform scope

The first release is implemented and validated on macOS.

The platform boundaries are structured for later Linux support, but this release makes no Linux or Windows compatibility claim.
