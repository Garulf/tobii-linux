use thiserror::Error;
use tobii_protocol::ProtocolError;

use crate::channel::ChannelError;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("Command {command} timed out")]
    Timeout { command: u32 },
    #[error("Command {command} failed: {name}")]
    Failed { command: u32, status: u32, name: String },
    #[error("The tracker sent no stream data for {} s", format_seconds(*.seconds))]
    Silent { seconds: f64 },
    #[error("The host was suspended; the tracker must be started again")]
    Suspended,
    #[error("The tracker clock went back; the tracker restarted")]
    ClockRewind,
    #[error("{0}")]
    NoTracker(String),
    #[error("Camera {index} requested, the tracker reports {count}")]
    NoCamera { index: usize, count: usize },
    #[error("{0}")]
    UnexpectedReply(String),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error(transparent)]
    Usb(#[from] tobii_usb::Error),
}

impl DeviceError {
    /// Errors that the Python layer raises as `DeviceError` (or `NoTrackerError`).
    pub fn is_device_error(&self) -> bool {
        matches!(
            self,
            DeviceError::Timeout { .. }
                | DeviceError::Failed { .. }
                | DeviceError::Silent { .. }
                | DeviceError::Suspended
                | DeviceError::ClockRewind
                | DeviceError::NoTracker(_)
                | DeviceError::UnexpectedReply(_)
        )
    }
}

/// Seconds as Python's `{:g}` prints them for the values used here (3, 0.5, 1.5).
pub fn format_seconds(seconds: f64) -> String {
    format!("{seconds}")
}

pub type Result<T, E = DeviceError> = std::result::Result<T, E>;
