//! One tracker session for the game service and the head-pose command.
//!
//! The tracker stops sending for good (stream data and replies) when its endpoint is not
//! read for about a second while streams are subscribed, so the session is read on its own
//! thread whatever its consumer does: gaze and presence go to their callbacks as they
//! arrive and only the newest image frame waits for the head-pose consumer. A session
//! that falls silent, a restarted tracker or a host resume ends the session.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use tobii_device::channel::ChannelError;
use tobii_device::clock::{ClockMapper, HostClock};
use tobii_device::streams::{
    DeviceClockWatch, ItemPump, Pending, SILENCE_TIMEOUT, StreamItem, StreamReader, StreamStats, StreamValue,
    SuspendWatch,
};
use tobii_device::{CameraCalibration, Channel, DeviceError, StopFlag, StopSignal, TrackerSession};
use tobii_headpose::pipeline::{DisplayArea, HeadPoseSample};
use tobii_protocol::commands::{Stream, command, header_only_request, request};
use tobii_protocol::fields::Value;
use tobii_protocol::frames::ImageFrame;
use tobii_protocol::gaze::{GazeFrame, PLATMOD_UNKNOWN, platmod_presence};
use tobii_protocol::{Message, ProtocolError};
use tobii_service::prp_service::presence_subscribe_time;
use tobii_service::tet_tcp::CAPTURED_GETTERS;

use crate::error::{Error, Result};

const READ_SLICE_MS: u32 = tobii_device::streams::READ_SLICE_MS;
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(1);
/// The head-pose command ends once the tracker sends no image for this long.
pub const FRAME_SILENCE: Duration = Duration::from_secs(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// A host clock shared by the reader thread, the consumer and the service.
#[derive(Clone)]
pub struct HostTime(pub Arc<dyn HostClock + Send + Sync>);

impl std::fmt::Debug for HostTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("HostTime").field(&self.0.now_us()).finish()
    }
}

impl HostClock for HostTime {
    fn now_us(&self) -> i64 {
        self.0.now_us()
    }

    fn generation(&self) -> Option<i64> {
        self.0.generation()
    }
}

/// Image frame to head-pose sample; `start` begins a tracker session with its camera.
pub trait FrameProcessor: Send {
    fn start(&mut self, calibration: &CameraCalibration);
    fn process(&mut self, frame: &ImageFrame) -> Result<HeadPoseSample>;
}

/// The models' face tracking, fitting and output stages.
#[derive(Debug)]
pub struct ModelProcessor(pub tobii_headpose::HeadPoseProcessor);

impl ModelProcessor {
    pub fn load(model_directory: &std::path::Path, area: DisplayArea) -> Result<Self> {
        let runtime = crate::paths::openvino()?;
        Ok(Self(tobii_headpose::HeadPoseProcessor::new(&runtime, model_directory, area)?))
    }
}

pub fn vec3(values: [f64; 3]) -> [f32; 3] {
    values.map(|v| v as f32)
}

impl FrameProcessor for ModelProcessor {
    fn start(&mut self, calibration: &CameraCalibration) {
        self.0.start(vec3(calibration.translation), vec3(calibration.angles));
    }

    fn process(&mut self, frame: &ImageFrame) -> Result<HeadPoseSample> {
        Ok(self.0.process(&frame.pixels, frame.width as usize, frame.height as usize, frame.timestamp)?)
    }
}

/// The calibration id in the device status (1490), as the runtime reads it at startup.
pub fn device_calibration_id<C: Channel>(session: &mut TrackerSession<C>) -> Result<Option<u32>> {
    let reply = session.try_call(|id| header_only_request(id, command::GET_DEVICE_STATUS))?;
    let values = TrackerSession::<C>::reply_values(&reply)?;
    tobii_gaze::device::device_calibration_id(&values).map_err(|error| Error::Value(error.to_string()))
}

/// Device replies by command, and the wire streams the service has subscribed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceReplies {
    pub replies: HashMap<u32, Message>,
    pub streams: BTreeSet<u32>,
}

impl DeviceReplies {
    /// The session's startup replies plus one reply per getter; call before streams start.
    pub fn capture<C: Channel>(session: &mut TrackerSession<C>, commands: &[u32], timeout: Duration) -> Result<Self> {
        let mut replies: HashMap<u32, Message> = session.startup_replies().iter().cloned().collect();
        for &command in commands {
            match session.exchange(|id| request(id, command, &[]).expect("a header-only getter"), timeout)? {
                None => log::info!("tracker gave no reply to {command}; tet-tcp clients get an error"),
                Some(reply) => {
                    replies.insert(command, reply);
                }
            }
        }
        Ok(Self { replies, streams: BTreeSet::new() })
    }

    pub fn capture_getters<C: Channel>(session: &mut TrackerSession<C>) -> Result<Self> {
        Self::capture(session, &CAPTURED_GETTERS, CAPTURE_TIMEOUT)
    }

    /// Top-left, top-right and bottom-left corners of the device's display area reply.
    pub fn display_area(&self) -> Option<[[f64; 3]; 3]> {
        let reply = self.replies.get(&command::GET_DISPLAY_AREA)?;
        if reply.word4 != 0 {
            return None;
        }
        let values: Vec<Value> = tobii_protocol::fields::values(&reply.payload).ok()?;
        let area = tobii_gaze::display::display_area_from_reply(&values).ok()?;
        Some([area.top_left, area.top_right, area.bottom_left])
    }
}

/// Stop requests of the caller and of the session itself.
struct EitherStop<'a> {
    outer: &'a dyn StopSignal,
    inner: &'a StopFlag,
}

impl StopSignal for EitherStop<'_> {
    fn is_set(&self) -> bool {
        self.outer.is_set() || self.inner.is_set()
    }

    fn wait(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.is_set() {
                return true;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            self.inner.wait((deadline - now).min(Duration::from_millis(50)));
        }
    }
}

/// The newest item of a reader thread for its consumer, and how the reader ended.
#[derive(Debug)]
pub struct Newest<T> {
    state: Mutex<NewestState<T>>,
    changed: Condvar,
}

#[derive(Debug)]
struct NewestState<T> {
    item: Option<T>,
    done: bool,
    error: Option<Error>,
    replaced: u64,
}

impl<T> Default for Newest<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(NewestState { item: None, done: false, error: None, replaced: 0 }),
            changed: Condvar::new(),
        }
    }
}

impl<T> Newest<T> {
    pub fn put(&self, item: T) {
        let mut state = lock(&self.state);
        if state.item.replace(item).is_some() {
            state.replaced += 1;
        }
        self.changed.notify_all();
    }

    pub fn finish(&self, error: Option<Error>) {
        let mut state = lock(&self.state);
        state.done = true;
        state.error = error;
        self.changed.notify_all();
    }

    /// Items replaced before the consumer took them.
    pub fn replaced(&self) -> u64 {
        lock(&self.state).replaced
    }

    /// The newest item; `None` once the reader is done and nothing waits.
    pub fn take(&self) -> Option<T> {
        let state = lock(&self.state);
        let mut state =
            self.changed.wait_while(state, |s| s.item.is_none() && !s.done).unwrap_or_else(|p| p.into_inner());
        state.item.take()
    }

    /// Waits up to `timeout` for an item.
    pub fn take_timeout(&self, timeout: Duration) -> Option<T> {
        let state = lock(&self.state);
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |s| s.item.is_none() && !s.done)
            .unwrap_or_else(|p| p.into_inner());
        state.item.take()
    }

    pub fn take_error(&self) -> Option<Error> {
        lock(&self.state).error.take()
    }
}

/// A request for the running session; `build` gets the request id to send it with.
pub struct DeviceRequest {
    pub build: Box<dyn FnOnce(u32) -> Result<Message, ProtocolError> + Send>,
    pub timeout: Duration,
    pub reply: tokio::sync::oneshot::Sender<Result<Message, DeviceError>>,
}

impl std::fmt::Debug for DeviceRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceRequest").field("timeout", &self.timeout).finish_non_exhaustive()
    }
}

/// The requests a running session answers between its reads.
pub type DeviceRequests = std::sync::mpsc::Receiver<DeviceRequest>;

/// The same failure for a second recipient; only the session's own error keeps its source.
fn reported(error: &DeviceError) -> DeviceError {
    match error {
        DeviceError::Timeout { command } => DeviceError::Timeout { command: *command },
        other => DeviceError::Channel(ChannelError::new(true, other.to_string())),
    }
}

type GazeCallback<'a> = &'a (dyn Fn(GazeFrame) + Sync);
type PresenceCallback<'a> = &'a (dyn Fn(i64, i64) + Sync);
type DeviceStreamCallback<'a> = &'a (dyn Fn(&Message, i64) + Sync);

/// Gaze, image and optionally presence streams of one session, for a live service.
///
/// Every gaze frame goes to `gaze` and every presence record to `presence` as it arrives,
/// restamped onto `clock` (microseconds), with the device value mapped as
/// receive_presence_data does. When the presence subscription succeeds, an unknown record
/// stamped 30 ms before the current time comes first, as the runtime's presence subscribe
/// emits one until the device reports (0x140276dd0). Image frames reach the consumer
/// newest first, dropping frames that arrive while it is busy, with a function mapping
/// their device timestamps onto `clock`. Gaze and images use separate clock mappers, so
/// head pose timing is the same as without gaze. A device clock that jumps back, as
/// `device_clock` judges it, ends the session. `device_stream`, when given, also receives
/// each gaze and presence message as the device sent it, with its time on `clock`.
/// `requests` are answered on the reading thread between reads, also while no stream data
/// arrives; while `paused` is set the session is not taken for dead when it falls silent.
pub struct LiveStreams<'a> {
    pub clock: HostTime,
    pub gaze: GazeCallback<'a>,
    pub presence: Option<PresenceCallback<'a>>,
    pub device_stream: Option<DeviceStreamCallback<'a>>,
    pub silence: Duration,
    pub suspend: Option<Box<dyn SuspendWatch>>,
    pub device_clock: DeviceClockWatch,
    pub requests: Option<DeviceRequests>,
    pub paused: Arc<AtomicBool>,
}

impl std::fmt::Debug for LiveStreams<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveStreams")
            .field("clock", &self.clock)
            .field("silence", &self.silence)
            .finish_non_exhaustive()
    }
}

impl<'a> LiveStreams<'a> {
    pub fn new(clock: HostTime, gaze: GazeCallback<'a>) -> Self {
        Self {
            clock,
            gaze,
            presence: None,
            device_stream: None,
            silence: SILENCE_TIMEOUT,
            suspend: None,
            device_clock: DeviceClockWatch::default(),
            requests: None,
            paused: Arc::default(),
        }
    }

    /// Subscribes, runs `started` with the reader, then hands frames to `consume` until
    /// `stop` is set or the session fails; unsubscribes on the way out.
    pub fn run<C: Channel>(
        self,
        session: &mut TrackerSession<C>,
        stop: &dyn StopSignal,
        started: impl FnOnce(&StreamReader),
        mut consume: impl FnMut(ImageFrame, &dyn Fn(i64) -> i64) -> Result<()>,
    ) -> Result<()> {
        let mut streams = vec![Stream::Gaze, Stream::Image];
        if self.presence.is_some() {
            streams.push(Stream::Presence);
        }
        let mut reader = StreamReader::new(&streams, &[Stream::Presence]);
        reader.subscribe(session)?;
        if reader.subscribed.contains(&Stream::Presence)
            && let Some(presence) = self.presence
        {
            presence(presence_subscribe_time(self.clock.now_us()), i64::from(PLATMOD_UNKNOWN));
        }
        started(&reader);
        let image_time = Mutex::new(ClockMapper::new(self.clock.clone()));
        let frames = Newest::default();
        let inner = StopFlag::new();
        let reader_stop = EitherStop { outer: stop, inner: &inner };
        let outcome = std::thread::scope(|scope| {
            let (frames, image_time, reader_stop) = (&frames, &image_time, &reader_stop);
            let mut reading = Reading {
                gaze: self.gaze,
                presence: self.presence,
                device_stream: self.device_stream,
                gaze_time: ClockMapper::new(self.clock.clone()),
                device_clock: self.device_clock,
                image_time,
                frames,
                requests: self.requests,
                paused: self.paused,
            };
            let mut suspend = self.suspend;
            let silence = self.silence;
            let (reader, session) = (&mut reader, &mut *session);
            let worker = std::thread::Builder::new()
                .name("tracker-frames".into())
                .spawn_scoped(scope, move || {
                    let error = reading.run(reader, session, silence, &mut suspend, reader_stop).err();
                    frames.finish(error);
                })
                .expect("spawn the tracker reader");
            let to_host = |device_us: i64| lock(image_time).map(device_us);
            let mut consumed = Ok(());
            while let Some(frame) = frames.take() {
                if stop.is_set() {
                    break;
                }
                if let Err(error) = consume(frame, &to_host) {
                    consumed = Err(error);
                    break;
                }
            }
            inner.set();
            let _ = worker.join();
            consumed.and_then(|()| frames.take_error().map_or(Ok(()), Err))
        });
        unsubscribe_after(&mut reader, session, outcome)
    }
}

/// Unsubscribes; when an error is already propagating, a failed unsubscribe does not replace it.
pub fn unsubscribe_after<C: Channel>(
    reader: &mut StreamReader,
    session: &mut TrackerSession<C>,
    outcome: Result<()>,
) -> Result<()> {
    let unsubscribed = reader.unsubscribe(session);
    outcome?;
    unsubscribed.map(drop).map_err(Error::from)
}

struct Reading<'a, 's> {
    gaze: GazeCallback<'a>,
    presence: Option<PresenceCallback<'a>>,
    device_stream: Option<DeviceStreamCallback<'a>>,
    gaze_time: ClockMapper<HostTime>,
    device_clock: DeviceClockWatch,
    image_time: &'s Mutex<ClockMapper<HostTime>>,
    frames: &'s Newest<ImageFrame>,
    requests: Option<DeviceRequests>,
    paused: Arc<AtomicBool>,
}

impl Reading<'_, '_> {
    fn run<C: Channel>(
        &mut self,
        reader: &mut StreamReader,
        session: &mut TrackerSession<C>,
        silence: Duration,
        suspend: &mut Option<Box<dyn SuspendWatch>>,
        stop: &dyn StopSignal,
    ) -> Result<()> {
        let mut pump = ItemPump::with_pause(session.take_pending_streams(), silence, Arc::clone(&self.paused));
        let failed = |session: &mut TrackerSession<C>, error: DeviceError| {
            if error.is_device_error() {
                session.set_dead(true);
            }
            Error::from(error)
        };
        loop {
            match pump.pending_item(reader, stop) {
                Pending::Item(item) => {
                    if let Err(error) = self.dispatch(*item) {
                        return Err(failed(session, error));
                    }
                    continue;
                }
                Pending::Stopped => return Ok(()),
                Pending::Empty => {}
            }
            if stop.is_set() {
                return Ok(());
            }
            if let Err(error) = self.answer_requests(session) {
                return Err(failed(session, error));
            }
            if !session.pending_streams.is_empty() {
                pump.extend(session.take_pending_streams());
                continue;
            }
            let data = session.read(READ_SLICE_MS).map_err(|error| failed(session, error))?;
            let resumed = suspend.as_deref_mut().is_some_and(|watch| watch.resumed());
            pump.received(session, &data, resumed).map_err(|error| failed(session, error))?;
        }
    }

    /// Answers every waiting request; fails when one leaves the session dead.
    fn answer_requests<C: Channel>(&self, session: &mut TrackerSession<C>) -> std::result::Result<(), DeviceError> {
        let Some(requests) = &self.requests else { return Ok(()) };
        while let Ok(request) = requests.try_recv() {
            let outcome = (request.build)(session.next_id()).map_err(DeviceError::from).and_then(|message| {
                let reply = session.request(&message, request.timeout)?;
                session.finish_call(&message, reply)
            });
            match outcome {
                Err(error) if session.is_dead() => {
                    let _ = request.reply.send(Err(reported(&error)));
                    return Err(error);
                }
                outcome => {
                    let _ = request.reply.send(outcome);
                }
            }
        }
        Ok(())
    }

    fn device_time(&mut self, device_us: i64) -> i64 {
        self.gaze_time.observe(device_us);
        self.gaze_time.map(device_us)
    }

    fn forward(&self, message: &Message, host_us: i64) {
        if let Some(device_stream) = self.device_stream {
            device_stream(message, host_us);
        }
    }

    fn dispatch(&mut self, item: StreamItem) -> std::result::Result<(), DeviceError> {
        match item.value {
            StreamValue::Image(frame) => {
                self.device_clock.observe(frame.timestamp)?;
                lock(self.image_time).observe(frame.timestamp);
                self.frames.put(frame);
            }
            StreamValue::Gaze(frame) => {
                self.device_clock.observe(frame.timestamp_us())?;
                let host_us = self.device_time(frame.timestamp_us());
                (self.gaze)(frame.with_timestamp(host_us));
                self.forward(&item.message, host_us);
            }
            StreamValue::Presence(presence) => {
                let host_us = self.device_time(presence.timestamp_us);
                if let Some(callback) = self.presence {
                    callback(host_us, platmod_presence(i64::from(presence.value)));
                }
                self.forward(&item.message, host_us);
            }
            StreamValue::Raw => {}
        }
        Ok(())
    }
}

/// Where one service session delivers its output; every callback must be thread safe.
///
/// `started(calibration_id)` runs once the streams are subscribed, before the first item,
/// with the device's calibration id (`None` when it reports none). With `device` the
/// session first reads the device replies tet-tcp clients get, passed with the subscribed
/// streams before `started`; `device_stream` then receives every raw gaze and presence
/// message.
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct ServiceFeed {
    pub sample: Option<Box<dyn Fn(HeadPoseSample) + Send + Sync>>,
    pub gaze: Option<Box<dyn Fn(GazeFrame) + Send + Sync>>,
    pub presence: Option<Box<dyn Fn(i64, i64) + Send + Sync>>,
    pub started: Option<Box<dyn Fn(Option<u32>) + Send + Sync>>,
    pub device: Option<Box<dyn Fn(DeviceReplies) + Send + Sync>>,
    pub device_stream: Option<Box<dyn Fn(&Message, i64) + Send + Sync>>,
}

impl std::fmt::Debug for ServiceFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceFeed").field("device", &self.device.is_some()).finish_non_exhaustive()
    }
}

/// One service session on a started tracker: calibration, streams, then samples until it ends.
///
/// `requests` wait until the streams run and are then answered between reads. Returns when
/// `stop` is set; fails when the tracker fails or goes silent.
pub fn serve_tracker<C: Channel>(
    session: &mut TrackerSession<C>,
    processor: &mut dyn FrameProcessor,
    clock: HostTime,
    feed: &ServiceFeed,
    stop: &dyn StopSignal,
    suspend: Option<Box<dyn SuspendWatch>>,
    requests: Option<DeviceRequests>,
) -> Result<()> {
    processor.start(&session.camera_calibration(0)?);
    let calibration_id = device_calibration_id(session)?;
    let replies = if feed.device.is_some() { Some(DeviceReplies::capture_getters(session)?) } else { None };
    let ignore = |_: GazeFrame| {};
    let gaze: GazeCallback = match &feed.gaze {
        Some(gaze) => gaze.as_ref(),
        None => &ignore,
    };
    let mut streams = LiveStreams::new(clock, gaze);
    streams.presence = feed.presence.as_deref().map(|p| p as PresenceCallback);
    streams.device_stream = feed.device_stream.as_deref().map(|d| d as DeviceStreamCallback);
    streams.suspend = suspend;
    streams.requests = requests;
    streams.run(
        session,
        stop,
        |reader| {
            if let (Some(device), Some(mut replies)) = (&feed.device, replies) {
                replies.streams = reader.subscribed.iter().map(|stream| stream.wire_id()).collect();
                device(replies);
            }
            if let Some(started) = &feed.started {
                started(calibration_id);
            }
        },
        |frame, to_host| {
            let sample = processor.process(&frame)?;
            if let Some(publish) = &feed.sample {
                publish(HeadPoseSample { timestamp_us: to_host(sample.timestamp_us), ..sample });
            }
            Ok(())
        },
    )
}

/// Head-pose samples of a started session's image stream, on device time, until the tracker
/// sends no frame for [`FRAME_SILENCE`] or `emit` fails.
pub fn head_pose_samples<C: Channel>(
    session: &mut TrackerSession<C>,
    processor: &mut dyn FrameProcessor,
    mut emit: impl FnMut(HeadPoseSample) -> Result<()>,
) -> Result<()> {
    processor.start(&session.camera_calibration(0)?);
    session.subscribe_images()?;
    let frames = Newest::default();
    let stop = StopFlag::new();
    std::thread::scope(|scope| {
        let session = &mut *session;
        let (frames, stop) = (&frames, &stop);
        let worker = std::thread::Builder::new()
            .name("tracker-frames".into())
            .spawn_scoped(scope, move || {
                let mut images = tobii_device::session::ImageFrames::new(session.take_pending_streams(), FRAME_SILENCE);
                let error = loop {
                    if stop.is_set() {
                        break None;
                    }
                    match images.pending_frame() {
                        Ok(Some(frame)) => {
                            frames.put(frame);
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => break Some(Error::from(error)),
                    }
                    let Some(remaining) = images.remaining_ms() else { break None };
                    match session.read(remaining.min(READ_SLICE_MS)) {
                        Ok(data) => {
                            if let Err(error) = images.received(session, &data) {
                                break Some(Error::from(error));
                            }
                        }
                        Err(error) => break Some(Error::from(error)),
                    }
                };
                frames.finish(error);
            })
            .expect("spawn the tracker reader");
        let mut outcome = Ok(());
        while let Some(frame) = frames.take() {
            if let Err(error) = processor.process(&frame).and_then(&mut emit) {
                outcome = Err(error);
                break;
            }
        }
        stop.set();
        let _ = worker.join();
        outcome.and_then(|()| frames.take_error().map_or(Ok(()), Err))
    })
}

/// The stream statistics of a running reader, shared with status reports.
pub type SharedStats = Arc<Mutex<StreamStats>>;

/// Items of a subscribed reader on a thread of their own, for consumers that want every item.
///
/// When the consumer falls `depth` items behind, the oldest waiting items are dropped.
pub fn drained_items<C: Channel>(
    reader: &mut StreamReader,
    session: &mut TrackerSession<C>,
    suspend: Option<Box<dyn SuspendWatch>>,
    stop: &dyn StopSignal,
    stats: &SharedStats,
    depth: usize,
    mut consume: impl FnMut(StreamItem),
) -> Result<()> {
    let queue: Mutex<(std::collections::VecDeque<StreamItem>, bool, Option<Error>)> =
        Mutex::new((std::collections::VecDeque::new(), false, None));
    let changed = Condvar::new();
    let inner = StopFlag::new();
    let reader_stop = EitherStop { outer: stop, inner: &inner };
    std::thread::scope(|scope| {
        let (queue, changed, reader_stop) = (&queue, &changed, &reader_stop);
        let (reader, session) = (&mut *reader, &mut *session);
        let mut suspend = suspend;
        let worker = std::thread::Builder::new()
            .name("tracker-drain".into())
            .spawn_scoped(scope, move || {
                let mut pump = ItemPump::new(session.take_pending_streams(), SILENCE_TIMEOUT);
                let error = loop {
                    let next = loop {
                        match pump.pending_item(reader, reader_stop) {
                            Pending::Item(item) => break Ok(Some(*item)),
                            Pending::Stopped => break Ok(None),
                            Pending::Empty => {}
                        }
                        if reader_stop.is_set() {
                            break Ok(None);
                        }
                        let data = match session.read(READ_SLICE_MS) {
                            Ok(data) => data,
                            Err(error) => break Err(error),
                        };
                        let resumed = suspend.as_deref_mut().is_some_and(|watch| watch.resumed());
                        if let Err(error) = pump.received(session, &data, resumed) {
                            break Err(error);
                        }
                    };
                    *lock(stats) = reader.stats.clone();
                    match next {
                        Ok(Some(item)) => {
                            let mut state = lock(queue);
                            if state.0.len() == depth {
                                state.0.pop_front();
                            }
                            state.0.push_back(item);
                            changed.notify_all();
                        }
                        Ok(None) => break None,
                        Err(error) => break Some(Error::from(error)),
                    }
                };
                let mut state = lock(queue);
                state.1 = true;
                state.2 = error;
                changed.notify_all();
            })
            .expect("spawn the tracker reader");
        loop {
            let state = lock(queue);
            let mut state = changed.wait_while(state, |s| s.0.is_empty() && !s.1).unwrap_or_else(|p| p.into_inner());
            let Some(item) = state.0.pop_front() else { break };
            drop(state);
            consume(item);
        }
        inner.set();
        let _ = worker.join();
        lock(queue).2.take().map_or(Ok(()), Err)
    })
}
