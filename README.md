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

- No path opens an untitled editor rooted at the current directory.
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

``Ctrl+` `` and `Ctrl+,` remain accepted when the outer terminal can report those modified punctuation keys, but many terminals cannot encode them distinctly.

The Explorer uses Up and Down to navigate, Enter or Right to expand or open, `R` to refresh, and Escape or Left to return to the workspace. Click the Open Editors header to collapse or expand its document list. The workspace row shows only the root folder name, and the file tree refreshes automatically every five seconds by default.

Startup without a file shows an empty editor surface. Typing creates one Untitled document, Save As keeps that document identity under its new filename, and `Ctrl+W` removes the focused document from Open Editors.

The Open dialog supports Up and Down selection, mouse-wheel scrolling, Enter or Right to open a file or folder, Left to visit the parent folder, Tab to copy the selected path, Ctrl+Enter to submit a manually edited path, and Escape to cancel. In Save As, type the filename after the initial directory and press Enter to save; use Right to enter the selected folder.

The terminal passes ordinary keyboard and paste input to the child process.

`Ctrl+T` and `Ctrl+G` remain workbench shortcuts while the terminal is focused.

`Ctrl+B` is the application leader. Editor split and focus chords are inactive while the terminal owns focus, preserving the single-terminal-session model.

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

Use Tab to change scope, Up and Down to select a setting, `Ctrl+R` to reset it in the selected scope, and Enter to open the raw TOML configuration as an editor document.

```toml
[editor]
indent_width = 4
insert_spaces = true
default_line_ending = "lf"
undo_limit_mib = 32
line_numbers = true
scrollbar = true

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
```

Unknown configuration keys produce non-blocking diagnostics.

Invalid configuration text is left untouched and valid defaults remain available.

## Files and recovery

Files are saved through a same-directory temporary file and atomic replacement.

Existing permissions are preserved, and saving through a symlink replaces its resolved target without replacing the symlink.

If a file changes externally before save, TinLynk offers overwrite or cancellation and leaves the external file untouched until confirmation.

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
