use thiserror::Error;
use tobii_protocol::ProtocolError;

/// Errors of the gaze configuration layer, with the messages of the Python modules.
#[derive(Debug, Error)]
pub enum GazeError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// `calibration.CalibrationError`.
    #[error("{0}")]
    Calibration(String),
    /// `calibration_profiles.ProfileError`.
    #[error("{0}")]
    Profile(String),
    /// Invalid arguments, raised as `ValueError`.
    #[error("{0}")]
    Value(String),
    /// A number outside the float32 range, raised as `OverflowError` like `struct.pack('<f')`.
    #[error("float too large to pack with f format")]
    Overflow,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl GazeError {
    pub fn protocol(message: &str) -> Self {
        GazeError::Protocol(ProtocolError::new(message))
    }

    pub fn file_exists(path: &std::path::Path) -> Self {
        GazeError::Io(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} exists", path.display())))
    }
}

pub type Result<T, E = GazeError> = std::result::Result<T, E>;
