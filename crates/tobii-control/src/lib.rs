//! The private control channel of `tobii-linux serve`, shared by the service, the
//! Tobii Experience app and `tobii-linux-setup`.
//!
//! It rides on the service's relay TCP port with the relay framing and is tobii-linux's own
//! protocol, not Tobii's (docs/control-channel.md). This crate holds the messages and a
//! blocking client; it pulls in no async runtime and no toolkit.

pub mod client;
pub mod protocol;

pub use client::{ControlClient, DEFAULT_ADDR, Incoming};
pub use protocol::{Event, GazeSample, Hello, Reply, Request, Status, TrackedEyes};

/// Why a control message could not be built, read or answered.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Invalid control message: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Frame(#[from] tobii_service::ServiceError),
    #[error("Invalid control message: {0}")]
    Invalid(String),
    #[error("The service closed the control connection")]
    Closed,
    #[error("The service did not reply in time")]
    Timeout,
    #[error("The service refused the request: {0}")]
    Refused(String),
}
