use thiserror::Error;

/// Failures of the inference layer, with the messages of the Python modules.
///
/// `Value` is a ValueError and `Overflow` an OverflowError, as Python's float conversions
/// raise it. `Unavailable` means the OpenVINO library could not be loaded (Python callers
/// see an ImportError, as for a missing `openvino` package); `OpenVino` is a failed
/// OpenVINO call (a RuntimeError).
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Value(String),
    #[error("{0}")]
    Overflow(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    OpenVino(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn value<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Value(message.into()))
}
