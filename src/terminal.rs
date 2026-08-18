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
            let _ = disable_raw_mode();
            return Err(AppError::io("could not enter the alternate screen", error));
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)
            .map_err(|error| AppError::io("could not initialize terminal rendering", error))?;
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
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

pub fn install_panic_restoration_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |information| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        previous(information);
    }));
}
