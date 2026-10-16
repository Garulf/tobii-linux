use std::error::Error as StdError;

use thiserror::Error;
use tobii_usb::UsbChannel;

/// Default deadline for one framed channel write, as the channel's own default.
pub const WRITE_TIMEOUT_MS: u32 = 3000;

/// The byte pipe a session runs on.
pub trait Channel: Send {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError>;
    /// Bytes of zero or more TTP messages; empty when nothing arrived within `timeout_ms`.
    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError>;
}

/// A channel failure. `fatal` failures (transfer, state and deadline errors) end the
/// session: a tracker that fails this way stays silent for the rest of its session.
#[derive(Debug, Error)]
#[error("{source}")]
pub struct ChannelError {
    pub fatal: bool,
    #[source]
    pub source: Box<dyn StdError + Send + Sync>,
}

impl ChannelError {
    pub fn new(fatal: bool, source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self { fatal, source: source.into() }
    }
}

impl From<tobii_usb::Error> for ChannelError {
    fn from(error: tobii_usb::Error) -> Self {
        let fatal = !matches!(
            error,
            tobii_usb::Error::Invalid(_) | tobii_usb::Error::Framing(_) | tobii_usb::Error::Foreign(_)
        );
        ChannelError::new(fatal, error)
    }
}

impl Channel for UsbChannel {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        UsbChannel::write(self, data, WRITE_TIMEOUT_MS).map(drop).map_err(Into::into)
    }

    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        UsbChannel::read(self, timeout_ms).map_err(Into::into)
    }
}

impl<C: Channel + ?Sized> Channel for Box<C> {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        (**self).write(data)
    }

    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        (**self).read(timeout_ms)
    }
}
