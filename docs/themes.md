# Themes

TinLynk Terminal Editor embeds two themes in the executable:

- `default` is ASCII-only and portable across terminal fonts.
- `nerd-font` uses Nerd Font UI icons and Kamon file-type glyphs and colors in the Explorer.

To use `nerd-font`, install a [Nerd Font](https://www.nerdfonts.com/) and configure the terminal emulator to use it. Without a Nerd Font, its glyphs can appear as missing-character boxes or fallback symbols.

## User themes

Place direct `*.toml` files in `$HOME/.tinlynk/terminal-editor/themes/`. The filename without `.toml` is the theme identifier. Nested directories and other file types are ignored.

The identifiers `default` and `nerd-font` are reserved and cannot be overridden. User themes are partial: omitted values inherit from the embedded default theme. Theme files are limited to 256 KiB and must contain UTF-8 TOML.

Select a theme through Settings, or set `theme.active` in User or Workspace configuration. Theme files themselves are always loaded from the user theme directory. In the theme picker, use Up and Down to select, Enter to apply, `R` to rescan, and Escape to cancel. Saving a theme file from the editor triggers an immediate rescan; an invalid live change preserves the previous valid theme and reports a diagnostic.

## Example

```toml
[meta]
name = "Ocean"
schema_version = 1
border_type = "rounded"

[colors]
accent = "#0088CC"
active_border = "#0088CC"
editor_selection = "#264F78"

[icons]
folder_open = "▾"
folder_closed = "▸"
file_default = "•"
explorer_new_file = "F"
explorer_new_folder = "D"
explorer_refresh = "R"
```

Colors accept `default`, the sixteen ANSI color names, or `#RRGGBB` without alpha. Use [themes/default.toml](../themes/default.toml) as the complete reference for supported color and icon keys.

Panel controls use `_visible` and `_hidden` icon keys. Explorer root actions use `explorer_new_file`, `explorer_new_folder`, and `explorer_refresh`. Themes cannot alter child-process ANSI, indexed, RGB, inverse, or default terminal color behavior.

## Syntax highlighting

Editor syntax highlighting uses Syntect's bundled syntax definitions and detects a language from the opened file path. Files with no matching syntax, including untitled and plain-text documents, retain the editor's normal foreground color. The initial implementation uses Syntect's bundled `base16-ocean.dark` palette; custom TinLynk theme files do not yet override syntax token colors.
