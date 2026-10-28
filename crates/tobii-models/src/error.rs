use thiserror::Error;

/// Failures of the model and asset readers, with the messages of the Python modules.
///
/// `Format` is a ValueError in Python.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Format(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn format<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Format(message.into()))
}
