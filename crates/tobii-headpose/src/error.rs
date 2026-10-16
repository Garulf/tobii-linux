use thiserror::Error;

/// Failures of the head-pose math, with the messages of the Python modules.
///
/// `Value` is a ValueError in Python and `Unsupported` a NotImplementedError: a branch of
/// the original that needs tracker state this reconstruction does not have. `Inference`
/// carries a failure of the face finder or D1 network.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Value(String),
    #[error("{0}")]
    Unsupported(String),
    #[error(transparent)]
    Models(#[from] tobii_models::Error),
    #[error(transparent)]
    Inference(#[from] tobii_inference::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Value(message.into()))
}

pub(crate) fn unsupported<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported(message.into()))
}
