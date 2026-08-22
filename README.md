# TinLynk Terminal Editor

TinLynk Terminal Editor is a lightweight terminal-native text editor with an integrated terminal for macOS.

It combines Unicode-aware file editing, tiled editor panes, a file Explorer, and one PTY-backed terminal session.

## Requirements

- macOS.
- Stable Rust 1.89 or newer.
- A terminal emulator with Unicode support.

The optional `nerd-font` theme requires a [Nerd Font](https://www.nerdfonts.com/) configured in the terminal emulator. Use the portable `default` theme when a Nerd Font is unavailable.

## Build and run

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

Files that appear to be binary data or a supported archive format require confirmation before they are opened as text. Press `O` to continue or Escape to cancel.

## Essential controls

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
| `Ctrl+H` | Open Help. |
| `Ctrl+Z` and `Ctrl+Y` | Undo and redo. |
| `Ctrl+C`, `Ctrl+X`, and `Ctrl+V` | Copy, cut, and paste with the internal clipboard. |

The in-app Help view reflects the active keybindings. See [the complete controls reference](docs/controls.md) for Explorer, terminal, dialog, mouse, scroll, and leader-key behavior.

## Configuration

User configuration is loaded from `$HOME/.tinlynk/terminal-editor/config.toml`.

Workspace configuration is loaded from `<explorer-root>/.tinlynk/terminal-editor/config.toml`.

Workspace configuration can override editor, workspace, Explorer, mouse, clipboard, UI, and keybinding values. Shell selection, shell arguments, and child OSC 52 permission remain user-level settings.

The Settings view shows effective values and supports User and Workspace scopes. See [configuration](docs/configuration.md) for the complete schema, scope rules, and editing workflow.

## Themes

The embedded `default` theme is ASCII-only. The optional `nerd-font` theme adds Nerd Font icons and uses Kamon to show file-type-specific explorer glyphs and colors.

Additional user themes are loaded from `$HOME/.tinlynk/terminal-editor/themes/`. See [theme authoring](docs/themes.md) for installation, inheritance, the theme picker, and the supported schema.

## Files, terminal, and clipboard

See [file handling](docs/file-handling.md) for text encoding, archive/binary confirmation, safe save behavior, line endings, and external-change recovery. The same page documents terminal, clipboard, and OSC 52 security behavior.

## Platform scope

The first release is implemented and validated on macOS.

The platform boundaries are structured for later Linux support, but this release makes no Linux or Windows compatibility claim.
