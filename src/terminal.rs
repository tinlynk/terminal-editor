use std::{io, io::Write, panic};

use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{AppError, Result};

pub type AppTerminal = Terminal<CrosstermBackend<io::Stdout>>;

pub struct TerminalGuard {
    terminal: AppTerminal,
}

impl TerminalGuard {
    pub fn enter(mouse_enabled: bool) -> Result<Self> {
        enable_raw_mode()
            .map_err(|error| AppError::io("could not enable terminal raw mode", error))?;
        let mut stdout = io::stdout();
        let setup = if mouse_enabled {
            execute!(
                stdout,
                EnterAlternateScreen,
                EnableMouseCapture,
                EnableBracketedPaste
            )
        } else {
            execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)
        };
        if let Err(error) = setup {
            restore_terminal_state();
            return Err(AppError::io("could not enter the alternate screen", error));
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                restore_terminal_state();
                return Err(AppError::io(
                    "could not initialize terminal rendering",
                    error,
                ));
            }
        };
        Ok(Self { terminal })
    }

    pub fn terminal_mut(&mut self) -> &mut AppTerminal {
        &mut self.terminal
    }

    pub fn write_control(&mut self, bytes: &[u8]) -> Result<()> {
        self.terminal
            .backend_mut()
            .write_all(bytes)
            .and_then(|()| self.terminal.backend_mut().flush())
            .map_err(|error| AppError::io("could not write terminal control sequence", error))
    }

    pub fn set_mouse_capture(&mut self, enabled: bool) -> Result<()> {
        let result = if enabled {
            execute!(self.terminal.backend_mut(), EnableMouseCapture)
        } else {
            execute!(self.terminal.backend_mut(), DisableMouseCapture)
        };
        result.map_err(|error| AppError::io("could not update terminal mouse capture", error))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
        let _ = disable_raw_mode();
    }
}

fn restore_terminal_state() {
    let mut stdout = io::stdout();
    restore_terminal_modes(&mut stdout);
    let _ = disable_raw_mode();
}

fn restore_terminal_modes(writer: &mut impl Write) {
    let _ = execute!(
        writer,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
}

pub fn install_panic_restoration_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |information| {
        restore_terminal_state();
        previous(information);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoration_disables_every_mode_enabled_during_setup() {
        let mut output = Vec::new();
        restore_terminal_modes(&mut output);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("\x1b[?2004l"));
        assert!(output.contains("\x1b[?1000l"));
        assert!(output.contains("\x1b[?1049l"));
    }
}
