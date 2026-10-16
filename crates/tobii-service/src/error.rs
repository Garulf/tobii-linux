use thiserror::Error;

/// Data the game service refuses, with the messages and exception classes of the Python
/// modules: `prp.ProtocolError`, `sesp.SespError`, `ValueError`, `struct.error` and the
/// TTP `ProtocolError`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServiceError {
    /// Malformed or unsupported PRP data, or a value outside its PRP field.
    #[error("{0}")]
    Prp(String),
    /// Malformed SESP framing, buffer or handshake.
    #[error("{0}")]
    Sesp(String),
    /// An argument or report the service cannot use.
    #[error("{0}")]
    Invalid(String),
    /// A fixed-layout read past the end of its buffer, where the Python codec let
    /// `struct.unpack_from` fail.
    #[error("{0}")]
    Truncated(String),
    #[error(transparent)]
    Protocol(#[from] tobii_protocol::ProtocolError),
}

pub type Result<T, E = ServiceError> = std::result::Result<T, E>;

pub(crate) fn prp<T>(message: &str) -> Result<T> {
    Err(ServiceError::Prp(message.to_string()))
}

pub(crate) fn sesp<T>(message: &str) -> Result<T> {
    Err(ServiceError::Sesp(message.to_string()))
}
