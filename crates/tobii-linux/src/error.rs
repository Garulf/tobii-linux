//! Failures of a subcommand, with the Python exception each one was in the Python command.
//!
//! The messages are those of the core crates, which the Python bindings passed through
//! unchanged; [`Error::python_type`] names the exception class the bindings raised, which
//! the command printed in status messages and session reports.

use std::io;

use thiserror::Error;
use tobii_device::DeviceError;
use tobii_device::supervisor::LinkFailure;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Usb(#[from] tobii_usb::Error),
    #[error(transparent)]
    Protocol(#[from] tobii_protocol::ProtocolError),
    #[error(transparent)]
    HeadPose(#[from] tobii_headpose::Error),
    #[error(transparent)]
    Inference(#[from] tobii_inference::Error),
    #[error(transparent)]
    Models(#[from] tobii_models::Error),
    #[error(transparent)]
    Service(#[from] tobii_service::ServiceError),
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A RuntimeError of the Python command.
    #[error("{0}")]
    Runtime(String),
    /// A ValueError of the Python command.
    #[error("{0}")]
    Value(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

fn io_type(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::AlreadyExists => "FileExistsError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        io::ErrorKind::ConnectionRefused => "ConnectionRefusedError",
        io::ErrorKind::ConnectionReset => "ConnectionResetError",
        io::ErrorKind::BrokenPipe => "BrokenPipeError",
        io::ErrorKind::TimedOut => "TimeoutError",
        _ => "OSError",
    }
}

fn usb_type(error: &tobii_usb::Error) -> &'static str {
    match error {
        tobii_usb::Error::Usb(_) => "USBError",
        tobii_usb::Error::Invalid(_) | tobii_usb::Error::Framing(_) => "ValueError",
        tobii_usb::Error::State(_) | tobii_usb::Error::Foreign(_) => "RuntimeError",
        tobii_usb::Error::ShortWrite { .. } => "ShortWriteError",
        tobii_usb::Error::DeadlineExpired => "TimeoutError",
    }
}

fn device_type(error: &DeviceError) -> &'static str {
    match error {
        DeviceError::NoTracker(_) => "NoTrackerError",
        DeviceError::NoCamera { .. } => "IndexError",
        DeviceError::Protocol(_) => "ProtocolError",
        DeviceError::Usb(error) => usb_type(error),
        DeviceError::Channel(channel) => match channel.source.downcast_ref::<tobii_usb::Error>() {
            Some(error) => usb_type(error),
            None => "RuntimeError",
        },
        _ => "DeviceError",
    }
}

fn models_type(error: &tobii_models::Error) -> &'static str {
    match error {
        tobii_models::Error::Archive(_) => "ArchiveError",
        tobii_models::Error::Format(_) => "ValueError",
        tobii_models::Error::Install(_) => "InstallError",
        tobii_models::Error::Io(error) => io_type(error),
    }
}

fn inference_type(error: &tobii_inference::Error) -> &'static str {
    match error {
        tobii_inference::Error::Value(_) => "ValueError",
        tobii_inference::Error::Overflow(_) => "OverflowError",
        tobii_inference::Error::Unavailable(_) => "ImportError",
        tobii_inference::Error::OpenVino(_) => "RuntimeError",
        tobii_inference::Error::Io(error) => io_type(error),
    }
}

impl Error {
    pub fn python_type(&self) -> &'static str {
        match self {
            Error::Device(error) => device_type(error),
            Error::Usb(error) => usb_type(error),
            Error::Protocol(_) => "ProtocolError",
            Error::HeadPose(tobii_headpose::Error::Value(_)) => "ValueError",
            Error::HeadPose(tobii_headpose::Error::Unsupported(_)) => "NotImplementedError",
            Error::HeadPose(tobii_headpose::Error::Models(error)) | Error::Models(error) => models_type(error),
            Error::HeadPose(tobii_headpose::Error::Inference(error)) | Error::Inference(error) => inference_type(error),
            Error::HeadPose(tobii_headpose::Error::Io(error)) | Error::Io(error) => io_type(error),
            Error::Service(tobii_service::ServiceError::Sesp(_)) => "SespError",
            Error::Service(tobii_service::ServiceError::Invalid(_)) => "ValueError",
            Error::Service(tobii_service::ServiceError::Truncated(_)) => "error",
            Error::Service(_) => "ProtocolError",
            Error::Runtime(_) => "RuntimeError",
            Error::Value(_) => "ValueError",
        }
    }

    /// `Type: message`, as the Python command reported failed sessions.
    pub fn described(&self) -> String {
        format!("{}: {self}", self.python_type())
    }
}

impl LinkFailure for Error {
    fn is_no_tracker(&self) -> bool {
        matches!(self, Error::Device(DeviceError::NoTracker(_)))
    }

    fn kind(&self) -> String {
        self.python_type().into()
    }
}
