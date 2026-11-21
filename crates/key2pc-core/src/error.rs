use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum Key2PcError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidConfig(String),
    Protocol(String),
    Input(String),
    Tray(String),
}

pub type Result<T> = std::result::Result<T, Key2PcError>;

impl fmt::Display for Key2PcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "IO error: {err}"),
            Self::Json(err) => write!(f, "JSON error: {err}"),
            Self::InvalidConfig(msg) => write!(f, "Configuration error: {msg}"),
            Self::Protocol(msg) => write!(f, "Protocol error: {msg}"),
            Self::Input(msg) => write!(f, "Input error: {msg}"),
            Self::Tray(msg) => write!(f, "Tray error: {msg}"),
        }
    }
}

impl Error for Key2PcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::InvalidConfig(_) | Self::Protocol(_) | Self::Input(_) | Self::Tray(_) => None,
        }
    }
}

impl From<std::io::Error> for Key2PcError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for Key2PcError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
