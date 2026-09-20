//! The single error type the application surfaces to `main`.

use std::fmt;
use std::path::PathBuf;

/// Anything that can stop the companion from starting or running.
#[derive(Debug)]
pub enum Error {
    /// A configuration file could not be read.
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A configuration file was read but is not valid TOML / not valid config.
    ConfigParse { path: PathBuf, message: String },
    /// A configuration value is syntactically fine but semantically impossible.
    ConfigInvalid(String),
    /// The sprite sheet is missing, undecodable, or the wrong shape (FR-01).
    Asset(String),
    /// The platform driver failed (no compositor, no layer shell, no window...).
    Platform(String),
    /// Command line arguments could not be understood.
    Cli(String),
}

impl Error {
    /// Build a platform error from any displayable cause.
    pub fn platform(context: impl fmt::Display) -> Self {
        Self::Platform(context.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigRead { path, source } => {
                write!(f, "cannot read config file '{}': {source}", path.display())
            }
            Self::ConfigParse { path, message } => {
                write!(f, "invalid config file '{}': {message}", path.display())
            }
            Self::ConfigInvalid(message) => write!(f, "invalid configuration: {message}"),
            Self::Asset(message) => write!(f, "sprite asset error: {message}"),
            Self::Platform(message) => write!(f, "platform error: {message}"),
            Self::Cli(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ConfigRead { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<Error> for std::io::Error {
    fn from(value: Error) -> Self {
        std::io::Error::other(value.to_string())
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
