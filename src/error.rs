use std::{fmt, io, path::PathBuf};

#[derive(Debug)]
pub enum AppError {
    Arguments(String),
    Config { path: PathBuf, message: String },
    Io { context: String, source: io::Error },
    Terminal(String),
}

impl AppError {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(message) | Self::Terminal(message) => formatter.write_str(message),
            Self::Config { path, message } => {
                write!(
                    formatter,
                    "invalid configuration at {}: {message}",
                    path.display()
                )
            }
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Arguments(_) | Self::Config { .. } | Self::Terminal(_) => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
