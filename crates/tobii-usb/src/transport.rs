use std::time::Duration;

use crate::error::Result;

/// Synchronous bulk and vendor control access to interface 0.
///
/// Implementations validate nothing beyond what the device or OS enforces; callers apply
/// the local limits. `read` returns an error with code -7 and the bytes received so far
/// when its timeout expires.
pub trait Transport: Send + Sync {
    fn read(&self, size: usize, timeout_ms: u32) -> Result<Vec<u8>>;
    fn write(&self, data: &[u8], timeout_ms: u32) -> Result<usize>;
    fn control_out(&self, request: u8, value: u16, index: u16, data: &[u8], timeout_ms: u32) -> Result<usize>;
    fn control_in(&self, request: u8, value: u16, index: u16, length: u16, timeout_ms: u32) -> Result<Vec<u8>>;

    /// Exclusive streaming access to the IN endpoint, for a channel's continuous reader.
    /// `None` when the transport cannot stream (direct reads are used instead).
    fn continuous_source(&self) -> Option<Box<dyn ContinuousSource>> {
        None
    }
}

/// A bulk IN endpoint kept busy with queued transfers.
pub trait ContinuousSource: Send {
    /// Bytes of the next completed transfer; `Ok(None)` when none completed in `timeout`.
    fn next_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>>;

    /// Cancels every queued transfer and hands the endpoint back to its transport.
    fn shutdown(self: Box<Self>);
}
