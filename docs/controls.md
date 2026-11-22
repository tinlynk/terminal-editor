# Controls

Open the in-app Help view with `Ctrl+H` to see the active configurable keybindings. The controls below describe the built-in navigation and default interaction model.

## Global

| Control | Action |
| --- | --- |
| `Ctrl+H` | Open Help. |
| `Ctrl+G` | Open Settings. |
| `Ctrl+F` | Open Search and Replace for the focused document. |
| `Ctrl+T` | Toggle the bottom terminal. |
| Escape | Close the active dialog. |
| `Ctrl+O` | Open the file dialog. |
| `Ctrl+S` | Save the focused document. |
| `Ctrl+W` | Close the focused document. |
| `Ctrl+Q` | Exit with required confirmations. |

`Ctrl+T` and `Ctrl+G` remain application shortcuts while the terminal is focused.
Closing a tab affects only its pane and shows another tab in that pane, if available. Otherwise, a split pane closes; the last pane remains empty. A modified document is confirmed before its last tab closes.

## Editor

Arrow keys move the cursor. Home and End move to the start and end of the line, while Page Up and Page Down move by a page. Use Shift with arrows, Home, End, Page Up, or Page Down to extend a selection when the terminal reports those modified keys.

Each pane has its own tabs. A new split starts with only the active document. Click a tab to select it in its pane. Drag a tab onto a different editor pane to move it there; if it is already open there, its existing view is preserved. Moving the last tab closes the source split. Document contents are shared between panes, but each pane has its own view. Drops elsewhere or onto the source pane cancel.

`Ctrl+C`, `Ctrl+X`, and `Ctrl+V` copy, cut, and paste through the internal clipboard. Enter inserts a line, Backspace and Delete remove text, Tab indents, and Shift+Tab unindents.

Search and Replace uses literal, case-sensitive text matching in the focused document. Type the search query, press Enter for the next match or Shift+Enter for the previous match, and use Tab to move to the replacement field. `Ctrl+R` replaces the selected match; `Ctrl+Shift+R` replaces all matches.

When soft wrap is disabled, Shift plus the vertical mouse wheel pans long lines horizontally. Horizontal terminal-wheel events can also pan long lines when the terminal forwards them. Click or drag the bottom-border scrollbar to navigate the document width.

## Explorer

Use Up and Down to select an entry. Enter or Right opens a file or expands a folder. Left collapses an expanded folder, and `R` refreshes the tree. Escape returns focus to the workspace.

Select the workspace root or Open Editors header and press Enter or Right to collapse or expand it. The Explorer root action icons create a file, create a folder, or refresh the tree. `Ctrl+W` closes the active open editor.

Press Backspace or forward Delete on a selected entry, then `D` to confirm permanent deletion or Escape to cancel. On most Mac keyboards, the key labeled Delete sends Backspace; Fn+Delete sends forward Delete. Deleting a folder recursively removes all its contents, including hidden and Explorer-excluded items. Symbolic links are deleted without deleting their targets. The workspace root cannot be deleted.

## File dialogs

In Open, Up and Down select an item, Enter or Right opens the selected file or folder, and Left moves to the parent folder. Tab copies the selected path. Use `Ctrl+Enter` to submit a manually edited path and Escape to cancel.

In Save As, type a filename after the initial directory and press Enter to save. Use Right to enter the selected folder.

## Terminal and leader

Ordinary keyboard and paste input is sent to the terminal child process. Paste is wrapped in bracketed-paste markers when the child has enabled that mode. The default configurable leader is `Ctrl+B`. A pending leader cancels after 1.5 seconds, Escape, paste, mouse input, or a focus change.

| Control | Action |
| --- | --- |
| `Ctrl+B`, then `V` | Split the editor vertically. |
| `Ctrl+B`, then `H` | Split the editor horizontally. |
| `Ctrl+B`, then Arrow | Focus the nearest editor pane. |
| `Ctrl+B`, then `Q` | Exit when the terminal is focused. |
| `Ctrl+B`, then `N` | Create and select a terminal session. |
| `Ctrl+B`, then `[` or `]` | Select the previous or next terminal session. |
| `Ctrl+B`, then `W` | Terminate the active terminal session with confirmation. |
| `Ctrl+B`, then Page Up or Page Down | Navigate application terminal scrollback. |
| `Ctrl+B`, then `C` | Copy the application-owned terminal selection. |
| `Ctrl+B` twice | Send a literal `Ctrl+B` to the child. |

`Ctrl+T` creates the first session when none exists, then toggles the bottom panel without closing its sessions. Terminal sessions appear as clickable tabs in the bottom panel. Each tab shows its number and current working-directory name. The panel holds at most eight live sessions.

Editor split and focus chords are inactive while the terminal owns focus. ``Ctrl+` `` and `Ctrl+,` are accepted only when the outer terminal can report those modified punctuation keys distinctly.

## Help

Use Up and Down, Page Up and Page Down, or the mouse wheel to scroll Help. Escape closes it.
