use std::fmt;

use thiserror::Error;
use tobii_protocol::FramingError;

pub const LIBUSB_ERROR_IO: i32 = -1;
pub const LIBUSB_ERROR_INVALID_PARAM: i32 = -2;
pub const LIBUSB_ERROR_ACCESS: i32 = -3;
pub const LIBUSB_ERROR_NO_DEVICE: i32 = -4;
pub const LIBUSB_ERROR_NOT_FOUND: i32 = -5;
pub const LIBUSB_ERROR_BUSY: i32 = -6;
pub const LIBUSB_ERROR_TIMEOUT: i32 = -7;
pub const LIBUSB_ERROR_PIPE: i32 = -9;
pub const LIBUSB_ERROR_NOT_SUPPORTED: i32 = -12;
pub const LIBUSB_ERROR_OTHER: i32 = -99;

/// A failed USB operation with a libusb error code, so logs and callers keep one
/// vocabulary whatever the backend. A read that times out keeps the bytes it received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbError {
    pub operation: String,
    pub code: i32,
    pub transferred: usize,
    pub partial_data: Vec<u8>,
}

impl UsbError {
    pub fn new(operation: impl Into<String>, code: i32) -> Self {
        Self { operation: operation.into(), code, transferred: 0, partial_data: Vec::new() }
    }

    pub fn is_timeout(&self) -> bool {
        self.code == LIBUSB_ERROR_TIMEOUT
    }
}

impl fmt::Display for UsbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: libusb error {} ({} bytes transferred)", self.operation, self.code, self.transferred)
    }
}

impl std::error::Error for UsbError {}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Usb(#[from] UsbError),
    /// An argument outside the local limits; the Python layer raises ValueError.
    #[error("{0}")]
    Invalid(String),
    /// Use of a closed, failed or busy object; the Python layer raises RuntimeError.
    #[error("{0}")]
    State(String),
    #[error("Partial channel frame write: {transferred}/{expected}; do not retry blindly")]
    ShortWrite { transferred: usize, expected: usize },
    #[error("Channel write deadline expired")]
    DeadlineExpired,
    #[error(transparent)]
    Framing(#[from] FramingError),
    /// An error raised by a transport implemented outside this crate (a Python object).
    #[error("{0}")]
    Foreign(Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Error::Invalid(message.into())
    }

    pub fn state(message: impl Into<String>) -> Self {
        Error::State(message.into())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
