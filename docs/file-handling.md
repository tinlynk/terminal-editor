# File Handling, Terminal, and Clipboard

## Opening files

TinLynk Terminal Editor is optimized for UTF-8 text. Files containing invalid UTF-8 require confirmation and open with lossy decoding; the status bar marks them `Lossy UTF-8 (read-only)`. Save and Save As are blocked for these documents because the original bytes cannot be reconstructed from the displayed replacement characters. Recovery records preserve this read-only state.

Before opening a file, the editor checks for ZIP, gzip, RAR, and 7z signatures (and the tar signature at its standard header offset), NUL bytes in the header, and invalid UTF-8 anywhere in the file. These files require confirmation. Press `O` to open the file as text or Escape to cancel. This warning preserves a requested split-open action.

## Saving and recovery

Files are saved through a temporary file in the target directory followed by atomic replacement. Existing permissions are preserved. Saving through a symlink replaces its resolved target without replacing the symlink.

Modified documents are captured in private local recovery records every 30 seconds by default. A recovery record is limited to 8 MiB of serialized TOML; larger records are not written and the failure is reported. At the next startup after an unclean exit, press `R` to restore a record, `D` to discard it, or Escape to defer it until the next launch. Configure or disable this behavior with `editor.autosave_interval_seconds`.

The editor also checks open named files for external changes every two seconds. A pristine document reloads automatically. For a modified document, press `R` to reload and discard local edits, `K` to keep local edits and acknowledge the external version, or Escape to defer that version. A later external version or deletion prompts again. When a file is removed, `R` acknowledges the removal while retaining the local contents. Saving a changed file still offers overwrite or cancellation and leaves the external file untouched until confirmation. LF and CRLF content is preserved, and inserted lines use the detected document convention.

Deleting an Explorer entry is permanent and requires confirmation. Deleting a folder recursively removes its contents, including hidden and Explorer-excluded items. Open documents remain available, including unsaved edits; the missing-file workflow applies, and saving a retained document may recreate its file. Explorer deletion removes a symbolic link itself, not its target.

## Terminal and clipboard security

The internal clipboard is always available for editor operations. Bracketed paste imports text supplied by the outer terminal.

Child OSC 52 clipboard writes are ignored by default. They can be enabled only in user configuration with `terminal.allow_child_osc52_write = true`. Child clipboard reads are never serviced.

OSC 52 support depends on the terminal emulator, so external clipboard interoperability is not guaranteed.
