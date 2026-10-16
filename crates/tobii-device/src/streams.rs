//! Several subscribed streams read from one session and split by stream.
//!
//! The reader decodes images, gaze (dropping a message exactly where the runtime does)
//! and presence; other stream messages pass through undecoded. The tracker stops sending
//! for good (stream data and replies) when its endpoint is not read for about a second
//! while streams are subscribed, so a session that falls silent is treated as dead
//! rather than waited on.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use tobii_protocol::Message;
use tobii_protocol::commands::{STREAM_DATA_KIND, Stream, unsubscribe_stream};
use tobii_protocol::frames::{ImageFrame, image_frame};
use tobii_protocol::gaze::{GazeFrame, Presence, decode_gaze_message, presence_message};

use crate::channel::Channel;
use crate::error::{DeviceError, Result};
use crate::session::TrackerSession;
use crate::stop::StopSignal;

pub const READ_SLICE_MS: u32 = 100;
pub const SILENCE_TIMEOUT: Duration = Duration::from_secs(3);
/// A suspended host keeps CLOCK_BOOTTIME running while CLOCK_MONOTONIC stops.
pub const SUSPEND_GAP_S: f64 = 2.0;
/// The device clock running back by more than this means the tracker restarted.
pub const DEVICE_CLOCK_REWIND_US: i64 = 1_000_000;

/// Seconds the host spent suspended since boot: CLOCK_BOOTTIME minus CLOCK_MONOTONIC.
pub fn suspended_time_s() -> f64 {
    use rustix::time::{ClockId, clock_gettime};
    let seconds = |t: rustix::time::Timespec| t.tv_sec as f64 + t.tv_nsec as f64 * 1e-9;
    seconds(clock_gettime(ClockId::Boottime)) - seconds(clock_gettime(ClockId::Monotonic))
}

/// Notices that the host was suspended since the last check (the tracker loses power then).
pub trait SuspendWatch: Send {
    fn resumed(&mut self) -> bool;
}

pub struct HostSuspendWatch<F = fn() -> f64> {
    suspended_time: F,
    threshold_s: f64,
    last: f64,
}

impl<F> std::fmt::Debug for HostSuspendWatch<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSuspendWatch").field("threshold_s", &self.threshold_s).field("last", &self.last).finish()
    }
}

impl Default for HostSuspendWatch {
    fn default() -> Self {
        Self::new(suspended_time_s, SUSPEND_GAP_S)
    }
}

impl<F: FnMut() -> f64> HostSuspendWatch<F> {
    pub fn new(mut suspended_time: F, threshold_s: f64) -> Self {
        let last = suspended_time();
        Self { suspended_time, threshold_s, last }
    }
}

impl<F: FnMut() -> f64 + Send> SuspendWatch for HostSuspendWatch<F> {
    fn resumed(&mut self) -> bool {
        let now = (self.suspended_time)();
        let jumped = now - self.last > self.threshold_s;
        self.last = now;
        jumped
    }
}

/// Fails when device timestamps jump back, which happens when the tracker restarts.
#[derive(Debug, Clone)]
pub struct DeviceClockWatch {
    pub rewind_us: i64,
    pub latest: Option<i64>,
}

impl Default for DeviceClockWatch {
    fn default() -> Self {
        Self { rewind_us: DEVICE_CLOCK_REWIND_US, latest: None }
    }
}

impl DeviceClockWatch {
    pub fn observe(&mut self, device_us: i64) -> Result<()> {
        if let Some(latest) = self.latest {
            if device_us < latest - self.rewind_us {
                return Err(DeviceError::ClockRewind);
            }
            self.latest = Some(latest.max(device_us));
        } else {
            self.latest = Some(device_us);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamValue {
    Image(ImageFrame),
    Gaze(Box<GazeFrame>),
    Presence(Presence),
    /// A stream without a decoder; the value is the message itself.
    Raw,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamItem {
    pub stream: Stream,
    pub value: StreamValue,
    pub message: Message,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamStats {
    pub received: BTreeMap<Stream, u64>,
    pub dropped: BTreeMap<Stream, u64>,
    pub decode_errors: BTreeMap<Stream, u64>,
    pub unexpected: u64,
    pub last_error: Option<String>,
}

/// Subscribes a set of streams on a started session and decodes their messages.
///
/// Streams listed as optional may refuse the subscription; they are then left out and
/// listed in `refused`.
#[derive(Debug, Clone)]
pub struct StreamReader {
    streams: Vec<Stream>,
    optional: Vec<Stream>,
    pub subscribed: Vec<Stream>,
    pub refused: Vec<(Stream, String)>,
    pub stats: StreamStats,
}

impl StreamReader {
    pub fn new(streams: &[Stream], optional: &[Stream]) -> Self {
        Self {
            streams: streams.to_vec(),
            optional: optional.to_vec(),
            subscribed: Vec::new(),
            refused: Vec::new(),
            stats: StreamStats::default(),
        }
    }

    pub fn streams(&self) -> &[Stream] {
        &self.streams
    }

    pub fn subscribe<C: Channel>(&mut self, session: &mut TrackerSession<C>) -> Result<()> {
        for &stream in &self.streams {
            match session.subscribe(stream) {
                Ok(_) => self.subscribed.push(stream),
                Err(error) if error.is_device_error() && self.optional.contains(&stream) => {
                    self.refused.push((stream, error.to_string()));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Unsubscribes every subscribed stream, newest first. Device and channel errors are
    /// collected so each stream gets its request; the first is returned. A dead session is
    /// not asked: its device no longer answers. Returns the streams unsubscribed.
    pub fn unsubscribe<C: Channel>(&mut self, session: &mut TrackerSession<C>) -> Result<Vec<Stream>> {
        if session.is_dead() {
            log::info!("not unsubscribing {} stream(s) of a dead session", self.subscribed.len());
            self.subscribed.clear();
            session.pending_streams.clear();
            return Ok(Vec::new());
        }
        let mut first_error = None;
        let mut unsubscribed = Vec::new();
        while let Some(stream) = self.subscribed.pop() {
            match session.try_call(|id| unsubscribe_stream(id, stream.index())) {
                Ok(_) => unsubscribed.push(stream),
                Err(error @ (DeviceError::Protocol(_) | DeviceError::NoCamera { .. })) => return Err(error),
                Err(DeviceError::Channel(error)) if !error.fatal => return Err(DeviceError::Channel(error)),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        session.pending_streams.clear();
        let names: Vec<&str> = unsubscribed.iter().map(|s| s.name()).collect();
        log::info!("unsubscribed {}", if names.is_empty() { "nothing".to_string() } else { names.join(", ") });
        match first_error {
            Some(error) => Err(error),
            None => Ok(unsubscribed),
        }
    }

    fn requested_stream(&self, wire_id: u32) -> Option<Stream> {
        self.streams.iter().copied().find(|stream| stream.wire_id() == wire_id)
    }

    /// The item for one stream message; `None` when it is not ours, dropped or malformed.
    pub fn decode(&mut self, message: Message) -> Option<StreamItem> {
        if message.kind != STREAM_DATA_KIND {
            return None;
        }
        let Some(stream) = self.requested_stream(message.command) else {
            self.stats.unexpected += 1;
            return None;
        };
        *self.stats.received.entry(stream).or_default() += 1;
        let decoded = match stream {
            Stream::Image => image_frame(&message).map(|frame| Some(StreamValue::Image(frame))),
            Stream::Gaze => decode_gaze_message(&message.payload)
                .map(|gaze| gaze.map(|device| StreamValue::Gaze(Box::new(GazeFrame::new(device))))),
            Stream::Presence => presence_message(&message.payload).map(|presence| presence.map(StreamValue::Presence)),
            _ => Ok(Some(StreamValue::Raw)),
        };
        match decoded {
            Ok(Some(value)) => Some(StreamItem { stream, value, message }),
            Ok(None) => {
                *self.stats.dropped.entry(stream).or_default() += 1;
                None
            }
            Err(error) => {
                *self.stats.decode_errors.entry(stream).or_default() += 1;
                self.stats.last_error = Some(format!("{} message: {error}", stream.name()));
                None
            }
        }
    }
}

#[derive(Debug)]
pub enum Pending {
    Item(Box<StreamItem>),
    Stopped,
    Empty,
}

/// The reading loop behind [`StreamReader`] items, resumable one item at a time.
#[derive(Debug)]
pub struct ItemPump {
    pending: VecDeque<Message>,
    silence: Duration,
    last_data: Instant,
}

impl ItemPump {
    /// Starts with the stream messages that arrived during earlier calls.
    pub fn new(pending: Vec<Message>, silence: Duration) -> Self {
        Self { pending: pending.into(), silence, last_data: Instant::now() }
    }

    /// The next item among messages already received; `Stopped` when `stop` is set first.
    pub fn pending_item(&mut self, reader: &mut StreamReader, stop: &dyn StopSignal) -> Pending {
        while let Some(message) = self.pending.pop_front() {
            if stop.is_set() {
                return Pending::Stopped;
            }
            if let Some(item) = reader.decode(message) {
                return Pending::Item(Box::new(item));
            }
        }
        Pending::Empty
    }

    /// Bytes from one read of [`READ_SLICE_MS`]. Fails, marking the session dead, when the
    /// host was suspended (`resumed`) or no stream data arrived for the silence timeout.
    pub fn received<C: Channel>(&mut self, session: &mut TrackerSession<C>, data: &[u8], resumed: bool) -> Result<()> {
        self.pending = if data.is_empty() { VecDeque::new() } else { session.decode(data)?.into() };
        if resumed {
            session.set_dead(true);
            return Err(DeviceError::Suspended);
        }
        if !self.pending.is_empty() {
            self.last_data = Instant::now();
        } else if self.last_data.elapsed() > self.silence {
            session.set_dead(true);
            return Err(DeviceError::Silent { seconds: self.silence.as_secs_f64() });
        }
        Ok(())
    }

    /// The next decoded item; `None` once `stop` is set. Fails, marking the session dead,
    /// when no stream data arrives for the silence timeout or the host was suspended.
    pub fn next<C: Channel>(
        &mut self,
        reader: &mut StreamReader,
        session: &mut TrackerSession<C>,
        mut suspend: Option<&mut dyn SuspendWatch>,
        stop: &dyn StopSignal,
    ) -> Result<Option<StreamItem>> {
        loop {
            if stop.is_set() {
                return Ok(None);
            }
            match self.pending_item(reader, stop) {
                Pending::Item(item) => return Ok(Some(*item)),
                Pending::Stopped => return Ok(None),
                Pending::Empty => {}
            }
            let data = session.read(READ_SLICE_MS)?;
            let resumed = suspend.as_deref_mut().is_some_and(|watch| watch.resumed());
            self.received(session, &data, resumed)?;
        }
    }
}

/// Items of every subscribed stream until `stop` is set or the session fails.
pub struct Items<'a, C> {
    reader: &'a mut StreamReader,
    session: &'a mut TrackerSession<C>,
    pump: ItemPump,
    suspend: Option<Box<dyn SuspendWatch>>,
    stop: &'a dyn StopSignal,
    finished: bool,
}

impl<C> std::fmt::Debug for Items<'_, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Items").field("pump", &self.pump).field("finished", &self.finished).finish()
    }
}

impl StreamReader {
    pub fn items<'a, C: Channel>(
        &'a mut self,
        session: &'a mut TrackerSession<C>,
        silence: Duration,
        suspend: Option<Box<dyn SuspendWatch>>,
        stop: &'a dyn StopSignal,
    ) -> Items<'a, C> {
        let pump = ItemPump::new(session.take_pending_streams(), silence);
        Items { reader: self, session, pump, suspend, stop, finished: false }
    }
}

impl<C: Channel> Iterator for Items<'_, C> {
    type Item = Result<StreamItem>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let suspend = self.suspend.as_deref_mut().map(|watch| watch as &mut dyn SuspendWatch);
        let outcome = self.pump.next(self.reader, self.session, suspend, self.stop).transpose();
        self.finished = !matches!(outcome, Some(Ok(_)));
        outcome
    }
}
