use thiserror::Error;

/// Failures of the model and asset readers, with the messages of the Python modules.
///
/// `Format` is a ValueError in Python.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Archive(String),
    #[error("{0}")]
    Format(String),
    #[error("{0}")]
    Install(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn format<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Format(message.into()))
}

pub(crate) fn archive<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Archive(message.into()))
}

pub(crate) fn install<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Install(message.into()))
}
