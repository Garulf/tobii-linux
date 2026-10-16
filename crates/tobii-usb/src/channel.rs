//! One vendor channel over the bulk pipes: open (0x41), framed writes, demultiplexed reads,
//! close (0x42). No timestamp negotiation, resets or firmware writes happen here.
//!
//! With a streaming transport the channel reads the IN endpoint continuously on its own
//! thread from open to close: the tracker stops sending for good when its endpoint is not
//! read for about a second while streams are subscribed, whatever the consumer is doing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tobii_protocol::framing::{CHANNEL_COUNT, IncomingFrames, outgoing_frames};

use crate::error::{Error, Result};
use crate::transport::{ContinuousSource, Transport};

pub const OPEN_REQUEST: u8 = 0x41;
pub const CLOSE_REQUEST: u8 = 0x42;
pub const READ_SIZE: usize = 16384;
/// Received but unread channel bytes after which the reader gives up on its consumer.
pub const MAX_BACKLOG: usize = 64 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadMode {
    /// Read the endpoint continuously on a thread when the transport can stream.
    #[default]
    Continuous,
    /// One transport read per [`UsbChannel::read`] call.
    Direct,
}

fn check_timeout(timeout_ms: u32, message: &str) -> Result<()> {
    if (1..=60_000).contains(&timeout_ms) { Ok(()) } else { Err(Error::invalid(message)) }
}

pub struct UsbChannel {
    transport: Arc<dyn Transport>,
    channel: u8,
    control_timeout_ms: u32,
    mode: ReadMode,
    frames: IncomingFrames,
    reader: Option<ChannelReader>,
    opened: bool,
    used: bool,
    failed: bool,
}

impl std::fmt::Debug for UsbChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbChannel")
            .field("channel", &self.channel)
            .field("opened", &self.opened)
            .field("failed", &self.failed)
            .field("continuous", &self.reader.is_some())
            .finish()
    }
}

impl UsbChannel {
    pub fn new(transport: Arc<dyn Transport>, channel: u32, control_timeout_ms: u32) -> Result<Self> {
        if channel >= CHANNEL_COUNT {
            return Err(Error::invalid("Channel must be between 0 and 11"));
        }
        check_timeout(control_timeout_ms, "Invalid control timeout")?;
        Ok(Self {
            transport,
            channel: channel as u8,
            control_timeout_ms,
            mode: ReadMode::default(),
            frames: IncomingFrames::default(),
            reader: None,
            opened: false,
            used: false,
            failed: false,
        })
    }

    pub fn with_read_mode(mut self, mode: ReadMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn channel(&self) -> u8 {
        self.channel
    }

    pub fn control_timeout_ms(&self) -> u32 {
        self.control_timeout_ms
    }

    pub fn is_open(&self) -> bool {
        self.opened
    }

    pub fn is_used(&self) -> bool {
        self.used
    }

    pub fn is_failed(&self) -> bool {
        self.failed
    }

    pub fn is_continuous(&self) -> bool {
        self.reader.is_some()
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    /// Sends the vendor open request; a channel opens once.
    pub fn open(&mut self) -> Result<()> {
        if self.used || self.failed {
            return Err(Error::state("Channel context is single-use or failed"));
        }
        self.used = true;
        self.transport.control_out(OPEN_REQUEST, u16::from(self.channel), 0, &[], self.control_timeout_ms)?;
        self.opened = true;
        if self.mode == ReadMode::Continuous
            && let Some(source) = self.transport.continuous_source()
        {
            self.reader = Some(ChannelReader::start(source, self.channel));
        }
        Ok(())
    }

    /// Stops the reader and sends the vendor close request; does nothing when not open.
    pub fn close(&mut self) -> Result<()> {
        if let Some(reader) = self.reader.take() {
            reader.stop();
        }
        if self.opened {
            self.opened = false;
            self.transport.control_out(CLOSE_REQUEST, u16::from(self.channel), 0, &[], self.control_timeout_ms)?;
        }
        Ok(())
    }

    fn check_open(&self) -> Result<()> {
        if !self.opened || self.failed { Err(Error::state("Channel is closed or failed")) } else { Ok(()) }
    }

    /// Writes channel frames under one total deadline, without any automatic retry.
    /// Returns the payload bytes written.
    pub fn write(&mut self, payload: &[u8], timeout_ms: u32) -> Result<usize> {
        self.check_open()?;
        check_timeout(timeout_ms, "Invalid write timeout")?;
        let result = self.write_frames(payload, Instant::now() + Duration::from_millis(u64::from(timeout_ms)));
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn write_frames(&self, payload: &[u8], deadline: Instant) -> Result<usize> {
        let mut total = 0;
        for frame in outgoing_frames(u32::from(self.channel), payload)? {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let remaining_ms = remaining.as_nanos().div_ceil(1_000_000);
            if remaining_ms == 0 {
                return Err(Error::DeadlineExpired);
            }
            let count = self.transport.write(&frame, remaining_ms.min(60_000) as u32)?;
            if count != frame.len() {
                return Err(Error::ShortWrite { transferred: count, expected: frame.len() });
            }
            total += count - 8;
        }
        Ok(total)
    }

    /// Completed payload bytes addressed to this channel; empty after a timeout, a partial
    /// frame or data for other channels. Bytes received before a timeout are kept.
    pub fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>> {
        self.check_open()?;
        if let Some(reader) = &self.reader {
            let result = reader.take(Duration::from_millis(u64::from(timeout_ms)));
            if result.is_err() {
                self.failed = true;
            }
            return result;
        }
        let data = match self.transport.read(READ_SIZE, timeout_ms) {
            Ok(data) => data,
            Err(Error::Usb(error)) if error.is_timeout() => error.partial_data,
            Err(Error::Foreign(error)) => return Err(Error::Foreign(error)),
            Err(error) => {
                self.failed = true;
                return Err(error);
            }
        };
        let frames = self.frames.feed(&data).inspect_err(|_| self.failed = true)?;
        let mut payload = Vec::new();
        for frame in frames {
            if frame.for_channel(u32::from(self.channel))? {
                payload.extend_from_slice(&frame.payload);
            }
        }
        Ok(payload)
    }
}

impl Drop for UsbChannel {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[derive(Default)]
struct Inbox {
    data: Vec<u8>,
    error: Option<Error>,
}

struct Shared {
    inbox: Mutex<Inbox>,
    ready: Condvar,
    stop: AtomicBool,
}

impl Shared {
    fn inbox(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn fail(&self, error: Error) {
        self.inbox().error.get_or_insert(error);
        self.ready.notify_all();
    }

    fn deliver(&self, frames: &mut IncomingFrames, channel: u8, data: &[u8]) -> Result<()> {
        let mut payload = Vec::new();
        for frame in frames.feed(data)? {
            if frame.for_channel(u32::from(channel))? {
                payload.extend_from_slice(&frame.payload);
            }
        }
        if payload.is_empty() {
            return Ok(());
        }
        let mut inbox = self.inbox();
        if inbox.data.len() + payload.len() > MAX_BACKLOG {
            return Err(Error::state("The channel reader's backlog is full; nothing consumes the tracker's data"));
        }
        inbox.data.extend_from_slice(&payload);
        drop(inbox);
        self.ready.notify_all();
        Ok(())
    }
}

/// The thread that drains one channel's IN endpoint from open to close.
struct ChannelReader {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<Box<dyn ContinuousSource>>>,
}

impl ChannelReader {
    fn start(mut source: Box<dyn ContinuousSource>, channel: u8) -> Self {
        let shared = Arc::new(Shared { inbox: Mutex::default(), ready: Condvar::new(), stop: AtomicBool::new(false) });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name(format!("tobii-usb-channel-{channel}"))
            .spawn(move || {
                let mut frames = IncomingFrames::default();
                while !worker.stop.load(Ordering::SeqCst) {
                    let outcome = match source.next_chunk(POLL) {
                        Ok(None) => Ok(()),
                        Ok(Some(data)) => worker.deliver(&mut frames, channel, &data),
                        Err(Error::Usb(error)) => {
                            let _ = worker.deliver(&mut frames, channel, &error.partial_data);
                            Err(Error::Usb(error))
                        }
                        Err(error) => Err(error),
                    };
                    if let Err(error) = outcome {
                        worker.fail(error);
                        break;
                    }
                }
                source
            })
            .expect("spawn the channel reader thread");
        Self { shared, thread: Some(thread) }
    }

    fn take(&self, timeout: Duration) -> Result<Vec<u8>> {
        let inbox = self.shared.inbox();
        let (mut inbox, _) = self
            .shared
            .ready
            .wait_timeout_while(inbox, timeout, |inbox| inbox.data.is_empty() && inbox.error.is_none())
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !inbox.data.is_empty() {
            return Ok(std::mem::take(&mut inbox.data));
        }
        match inbox.error.take() {
            Some(error) => Err(error),
            None => Ok(Vec::new()),
        }
    }

    fn stop(mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take()
            && let Ok(source) = thread.join()
        {
            source.shutdown();
        }
    }
}

impl Drop for ChannelReader {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests;
