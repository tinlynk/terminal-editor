# File Handling, Terminal, and Clipboard

## Opening files

TinLynk Terminal Editor is optimized for UTF-8 text. Non-UTF-8 content opens with lossy decoding and is marked `Binary`.

Before opening a file, the editor inspects a bounded header. ZIP, gzip, RAR, 7z, and tar archives, as well as files containing NUL bytes in that header, require confirmation. Press `O` to open the file as text or Escape to cancel. This warning preserves a requested split-open action.

## Saving and recovery

Files are saved through a temporary file in the target directory followed by atomic replacement. Existing permissions are preserved. Saving through a symlink replaces its resolved target without replacing the symlink.

If a file changes externally before save, the editor offers overwrite or cancellation and leaves the external file untouched until confirmation. LF and CRLF content is preserved, and inserted lines use the detected document convention.

## Terminal and clipboard security

The internal clipboard is always available for editor operations. Bracketed paste imports text supplied by the outer terminal.

Child OSC 52 clipboard writes are ignored by default. They can be enabled only in user configuration with `terminal.allow_child_osc52_write = true`. Child clipboard reads are never serviced.

OSC 52 support depends on the terminal emulator, so external clipboard interoperability is not guaranteed.

Return to the [README](../README.md).