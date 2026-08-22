# Configuration

TinLynk Terminal Editor loads user configuration from `$HOME/.tinlynk/terminal-editor/config.toml` and workspace configuration from `<explorer-root>/.tinlynk/terminal-editor/config.toml`.

Workspace configuration can override editor, workspace, Explorer, mouse, clipboard, UI, theme, and keybinding values. Shell selection, shell arguments, and permission for child OSC 52 writes are user-level settings.

## Settings view

Open Settings with `Ctrl+G`. Tab switches scope, Up and Down select a setting, Enter edits it, and `Ctrl+R` resets the selected value in the selected scope. The view shows effective values and defaults. Selecting `theme.active` opens the theme picker.

Unknown configuration keys produce non-blocking diagnostics. Invalid configuration text remains untouched and valid defaults remain available.

## Example

```toml
[editor]
indent_width = 4
insert_spaces = true
default_line_ending = "lf"
undo_limit_mib = 32
line_numbers = true
scrollbar = true
soft_wrap = false
header_style = "tabs"

[workspace]
minimum_pane_width = 12
minimum_pane_height = 4
initial_split_ratio = 0.5

[explorer]
visible = true
width = 24
exclusions = [".git", ".git/**"]
refresh_interval_seconds = 5

[mouse]
enabled = true

[clipboard]
osc52 = true

[terminal]
scrollback_lines = 10000
allow_child_osc52_write = false
leader = "ctrl+b"

[ui]
bottom_terminal_visible = false

[theme]
active = "default"
```

`editor.default_line_ending` accepts `lf` or `crlf`. `editor.header_style` accepts `tabs` or `breadcrumbs`. Set `editor.soft_wrap = true` to wrap long lines and disable horizontal panning and its scrollbar.

Use [theme authoring](themes.md) for `theme.active` and custom theme behavior. Return to the [README](../README.md).