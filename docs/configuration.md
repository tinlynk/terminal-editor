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
syntax_highlighting = true
syntax_highlighting_max_file_size_kib = 1024
autosave_interval_seconds = 30
header_style = "tabs"

[workspace]
minimum_pane_width = 12
minimum_pane_height = 4
initial_split_ratio = 0.5
vertical_divider_thickness = 1
horizontal_divider_thickness = 1

[explorer]
visible = true
width = 24
exclusions = [".git", ".git/**"]
refresh_interval_seconds = 5
divider_thickness = 1

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
terminal_divider_thickness = 1

[theme]
active = "default"
```

`editor.default_line_ending` accepts `lf` or `crlf`. `editor.header_style` accepts `tabs` or `breadcrumbs`. Set `editor.soft_wrap = true` to wrap long lines and disable horizontal panning and its scrollbar.

Syntax highlighting is enabled by default for recognized source files. Set `editor.syntax_highlighting = false` to disable it, or change `editor.syntax_highlighting_max_file_size_kib` from its 1024 KiB default to control the largest file parsed for highlighting.

Modified documents are written to user-local recovery records every 30 seconds. Set `editor.autosave_interval_seconds` to `0` or a negative value to disable recovery autosave. Recovery records are stored under `$HOME/.tinlynk/terminal-editor/recovery/` and are offered at the next startup after an unclean exit.

Divider thicknesses can be customized between panels: `explorer.divider_thickness` controls the width of the vertical divider between the Explorer and editor panes, `ui.terminal_divider_thickness` controls the height of the horizontal divider above the bottom terminal panel, `workspace.vertical_divider_thickness` controls dividers in side-by-side editor pane splits, and `workspace.horizontal_divider_thickness` controls dividers in stacked editor pane splits. Thickness values accept integers from `0` to `10`; the workspace settings default to `0`. Setting thickness to `0` removes the separator gap and places panels flush against each other. Replace the legacy `workspace.divider_thickness` setting with both workspace directional settings.

Use [theme authoring](themes.md) for `theme.active` and custom theme behavior.