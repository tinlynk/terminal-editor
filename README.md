# TinLynk Terminal Editor

TinLynk Terminal Editor is a lightweight terminal-native text editor with an integrated terminal.

It combines Unicode-aware file editing, tiled editor panes, a file Explorer, and up to eight PTY-backed terminal sessions.

## Requirements

- Prebuilt releases: macOS 26+ on Apple Silicon, or GNU/Linux x64/ARM64 with glibc 2.35+.
- Stable Rust 1.89 or newer when building from source.
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

## Install a release

Download and review the installer before running it:

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  -o install.sh https://github.com/tinlynk/terminal-editor/releases/latest/download/install.sh
# Review install.sh, then install the latest stable release:
sh install.sh
# Or install a pinned version without administrator permission:
sh install.sh --version v0.1.0 --prefix "$HOME/.local"
```

The default destination is `/usr/local/bin`. Use `--prefix PREFIX` for `PREFIX/bin`, or `--bin-dir DIRECTORY` for an exact destination. The installer detects the platform, verifies the binary's SHA-256 checksum, and requests sudo only when the destination requires it. Put the chosen directory on your `PATH`.

Releases also provide a macOS ARM64 `.pkg` installer and Linux x64/ARM64 `.tar.gz` archives, alongside raw binaries. The macOS package is **unsigned and not notarized**; macOS may warn or block it. See [releases and installation](docs/releases.md) for manual installation, platform limitations, verification, and release-maintainer instructions.

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
| `Ctrl+T` | Create or toggle the persistent bottom terminal panel. |
| `Ctrl+G` | Open the searchable settings view. |
| `Ctrl+H` | Open Help. |
| `Ctrl+F` | Open Search and Replace for the focused document. |
| `Ctrl+Z` and `Ctrl+Y` | Undo and redo. |
| `Ctrl+C`, `Ctrl+X`, and `Ctrl+V` | Copy, cut, and paste with the internal clipboard. |

The in-app Help view reflects the active keybindings. Terminal-focused leader commands create, switch, and close terminal sessions; the complete controls reference covers those commands and tab selection in [docs/controls.md](docs/controls.md).

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

Development is validated locally on macOS. Release automation includes native tests, artifact inspection, and packaged-binary PTY smoke checks for macOS ARM64 and Linux x64/ARM64.

Linux artifacts target GNU/Linux with glibc 2.35 or newer, not Alpine/musl. macOS artifacts require macOS 26 or newer on Apple Silicon. No Windows or macOS Intel release artifacts are provided. See [release validation](docs/releases.md#validation) for the checks required before publication.
