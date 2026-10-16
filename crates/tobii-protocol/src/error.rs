use thiserror::Error;

/// A TTP message, field or stream payload that the runtime's parsers would not accept.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct ProtocolError(pub String);

impl ProtocolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// Invalid USB channel framing or arguments; the Python layer raises these as ValueError.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct FramingError(pub String);

impl FramingError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

pub type Result<T, E = ProtocolError> = std::result::Result<T, E>;

pub(crate) fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(ProtocolError::new(message))
}
