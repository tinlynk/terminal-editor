use std::{env, path::PathBuf, process::ExitCode};

use tinlynk_terminal_editor::{AppError, Application};

fn parse_startup_path() -> Result<Option<PathBuf>, AppError> {
    let mut paths = env::args_os().skip(1);
    let path = paths.next().map(PathBuf::from);
    if paths.next().is_some() {
        return Err(AppError::Arguments(
            "expected zero or one file or directory path".to_owned(),
        ));
    }
    Ok(path)
}

fn run() -> Result<(), AppError> {
    let startup_path = parse_startup_path()?;
    let mut application = Application::new(startup_path)?;
    application.run()
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tinlynk-editor: {error}");
            ExitCode::FAILURE
        }
    }
}
