use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::Duration,
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::{AppError, Result, terminal_emulator::TerminalEmulator};

const PTY_OUTPUT_CHANNEL_CAPACITY: usize = 64;
#[cfg(test)]
const TERMINATION_TEST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct TerminalSession {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Receiver<Vec<u8>>,
    emulator: TerminalEmulator,
    pub working_directory: PathBuf,
    scroll_offset: usize,
    selection_anchor: Option<(usize, usize)>,
    selection_cursor: Option<(usize, usize)>,
    selection_rows: usize,
}

impl std::fmt::Debug for TerminalSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSession")
            .field("scrollback_lines", &self.emulator.scrollback_len())
            .field("working_directory", &self.working_directory)
            .finish_non_exhaustive()
    }
}

impl TerminalSession {
    pub fn spawn(
        shell: &Path,
        arguments: &[String],
        working_directory: &Path,
        size: PtySize,
        scrollback_limit: usize,
        allow_child_osc52: bool,
    ) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|error| AppError::Terminal(format!("could not create PTY: {error}")))?;
        let mut command = CommandBuilder::new(shell);
        command.args(arguments);
        command.cwd(working_directory);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env_remove("NVIM");
        command.env_remove("NVIM_LISTEN_ADDRESS");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| AppError::Terminal(format!("could not start shell: {error}")))?;
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| AppError::Terminal(format!("could not read PTY: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| AppError::Terminal(format!("could not write PTY: {error}")))?;
        let (sender, receiver) = mpsc::sync_channel(PTY_OUTPUT_CHANNEL_CAPACITY);
        spawn_reader(reader, sender);
        Ok(Self {
            writer,
            master: pair.master,
            child,
            output: receiver,
            emulator: TerminalEmulator::new(
                size.rows,
                size.cols,
                scrollback_limit,
                allow_child_osc52,
            ),
            working_directory: working_directory.to_owned(),
            scroll_offset: 0,
            selection_anchor: None,
            selection_cursor: None,
            selection_rows: 0,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer
            .write_all(bytes)
            .and_then(|()| self.writer.flush())
            .map_err(|error| AppError::io("could not write terminal input", error))
    }

    pub fn apply_runtime_config(&mut self, scrollback_limit: usize, allow_child_osc52: bool) {
        self.emulator
            .apply_runtime_config(scrollback_limit, allow_child_osc52);
        self.scroll_offset = self.scroll_offset.min(self.emulator.scrollback_len());
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| AppError::Terminal(format!("could not resize PTY: {error}")))
    }

    pub fn drain_output(&mut self) -> Result<bool> {
        let mut changed = false;
        while let Ok(bytes) = self.output.try_recv() {
            changed = true;
            self.consume(&bytes)?;
        }
        Ok(changed)
    }

    pub fn visible_lines(&self, rows: usize) -> Vec<String> {
        let lines = self.emulator.visible_lines();
        lines[lines.len().saturating_sub(rows)..].to_vec()
    }

    pub fn visible_cells(&self, rows: usize) -> Vec<Vec<crate::terminal_emulator::TerminalCell>> {
        self.emulator.viewport_cells(rows, self.scroll_offset)
    }

    pub fn scroll(&mut self, delta: isize) {
        self.scroll_offset = self
            .scroll_offset
            .saturating_add_signed(delta)
            .min(self.emulator.scrollback_len());
        self.clear_selection();
    }

    pub fn at_live_bottom(&self) -> bool {
        self.scroll_offset == 0
    }

    pub fn start_selection(&mut self, row: usize, column: usize, rows: usize) {
        self.selection_anchor = Some((row, column));
        self.selection_cursor = Some((row, column));
        self.selection_rows = rows;
    }

    pub fn update_selection(&mut self, row: usize, column: usize) {
        if self.selection_anchor.is_some() {
            self.selection_cursor = Some((row, column));
        }
    }

    pub fn selection(&self) -> Option<((usize, usize), (usize, usize))> {
        Some((self.selection_anchor?, self.selection_cursor?))
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = ordered_selection(self.selection()?);
        if start == end {
            return None;
        }
        let rows = self.visible_cells(self.selection_rows);
        let mut selected = Vec::new();
        let final_row = end.0.min(rows.len().saturating_sub(1));
        for (row, cells) in rows
            .iter()
            .enumerate()
            .take(final_row.saturating_add(1))
            .skip(start.0)
        {
            let from = if row == start.0 { start.1 } else { 0 };
            let to = if row == end.0 {
                end.1.saturating_add(1)
            } else {
                cells.len()
            };
            let line = cells[from.min(cells.len())..to.min(cells.len())]
                .iter()
                .filter(|cell| !cell.continuation)
                .map(|cell| cell.character)
                .collect::<String>()
                .trim_end()
                .to_owned();
            selected.push(line);
        }
        Some(selected.join("\n"))
    }

    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_cursor = None;
    }

    pub fn mouse_reporting(&self) -> bool {
        self.emulator.modes().mouse_tracking()
            != crate::terminal_emulator::MouseTrackingMode::Disabled
    }

    fn consume(&mut self, bytes: &[u8]) -> Result<()> {
        self.emulator.feed(bytes);
        while let Some(response) = self.emulator.take_response() {
            self.write(&response)?;
        }
        if let Some(directory) = self.emulator.working_directory() {
            self.working_directory.clone_from(directory);
        }
        Ok(())
    }

    pub fn resize_screen(&mut self, rows: u16, columns: u16) -> Result<()> {
        self.resize(rows, columns)?;
        self.emulator.resize(rows, columns);
        Ok(())
    }

    pub fn is_alive(&mut self) -> Result<bool> {
        self.child
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|error| AppError::io("could not inspect terminal child", error))
    }

    pub fn begin_termination(&mut self) {
        let Some(process_id) = self.child.process_id() else {
            let _ = self.child.kill();
            return;
        };
        #[cfg(unix)]
        begin_unix_termination(process_id);
        #[cfg(not(unix))]
        let _ = self.child.kill();
    }

    pub fn modes(&self) -> crate::terminal_emulator::TerminalModes {
        self.emulator.modes()
    }

    pub fn cursor(&self) -> (u16, u16) {
        self.emulator.cursor()
    }

    pub fn take_child_clipboard_write(&mut self) -> Option<String> {
        self.emulator.take_child_clipboard_write()
    }
}

#[cfg(unix)]
fn begin_unix_termination(process_id: u32) {
    thread::spawn(move || {
        signal_process(process_id, "HUP");
        thread::sleep(Duration::from_secs(2));
        if process_group_alive(process_id) {
            signal_process(process_id, "TERM");
            thread::sleep(Duration::from_secs(2));
        }
        if process_group_alive(process_id) {
            signal_process(process_id, "KILL");
        }
    });
}

fn ordered_selection(
    selection: ((usize, usize), (usize, usize)),
) -> ((usize, usize), (usize, usize)) {
    if selection.0 <= selection.1 {
        selection
    } else {
        (selection.1, selection.0)
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, sender: SyncSender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buffer = vec![0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(length) if sender.send(buffer[..length].to_vec()).is_err() => break,
                Ok(_) => {}
            }
        }
    });
}

#[cfg(unix)]
fn signal_process(process_id: u32, signal: &str) {
    if let Some(process_group) = child_process_group(process_id)
        && run_kill(signal, &format!("-{process_group}"))
    {
        return;
    }
    run_kill(signal, &process_id.to_string());
}

#[cfg(unix)]
fn process_group_alive(process_id: u32) -> bool {
    child_process_group(process_id).is_some_and(|group| run_kill("0", &format!("-{group}")))
        || run_kill("0", &process_id.to_string())
}

#[cfg(unix)]
fn child_process_group(process_id: u32) -> Option<u32> {
    let child_group = process_group_id(process_id)?;
    let own_group = process_group_id(std::process::id())?;
    (child_group != own_group).then_some(child_group)
}

#[cfg(unix)]
fn process_group_id(process_id: u32) -> Option<u32> {
    let output = Command::new("/bin/ps")
        .args(["-o", "pgid=", "-p", &process_id.to_string()])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(unix)]
fn run_kill(signal: &str, target: &str) -> bool {
    Command::new("/bin/kill")
        .args([format!("-{signal}"), target.to_owned()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn scrollback_is_bounded() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 1,
                cols: 1,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new("/usr/bin/true");
        command.cwd("/");
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let writer = pair.master.take_writer().unwrap();
        let mut session = TerminalSession {
            writer,
            master: pair.master,
            child,
            output: receiver,
            emulator: TerminalEmulator::new(2, 16, 2, false),
            working_directory: PathBuf::new(),
            scroll_offset: 0,
            selection_anchor: None,
            selection_cursor: None,
            selection_rows: 0,
        };
        session.consume(b"abc").unwrap();
        session.start_selection(0, 0, 2);
        session.update_selection(0, 2);
        assert_eq!(session.selected_text().as_deref(), Some("abc"));
        session.clear_selection();
        session.consume(b"one\ntwo\nthree\n").unwrap();
        assert_eq!(session.emulator.scrollback_len(), 2);
        session.scroll(1);
        assert!(!session.at_live_bottom());
    }

    #[test]
    fn shell_startup_input_output_resize_and_exit() {
        let shell = if Path::new("/bin/zsh").exists() {
            Path::new("/bin/zsh")
        } else {
            Path::new("/bin/sh")
        };
        let mut session = TerminalSession::spawn(
            shell,
            &[],
            Path::new("/tmp"),
            PtySize {
                rows: 10,
                cols: 40,
                pixel_width: 0,
                pixel_height: 0,
            },
            100,
            false,
        )
        .unwrap();
        session.resize_screen(12, 50).unwrap();
        session
            .write(b"printf 'tinlynk-pty-ok\\n'\nexit\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut output = String::new();
        while Instant::now() < deadline {
            session.drain_output().unwrap();
            output = session.visible_lines(12).join("\n");
            if output.contains("tinlynk-pty-ok") && !session.is_alive().unwrap() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(output.contains("tinlynk-pty-ok"));
        assert!(!session.is_alive().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_termination_stops_the_child_with_silent_helpers() {
        let mut session = TerminalSession::spawn(
            Path::new("/bin/sh"),
            &[],
            Path::new("/tmp"),
            PtySize {
                rows: 10,
                cols: 40,
                pixel_width: 0,
                pixel_height: 0,
            },
            100,
            false,
        )
        .unwrap();
        assert!(session.is_alive().unwrap());
        session.begin_termination();
        let deadline = Instant::now() + TERMINATION_TEST_TIMEOUT;
        while Instant::now() < deadline && session.is_alive().unwrap() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!session.is_alive().unwrap());
    }
}
