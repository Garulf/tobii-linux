//! Tracker data to browser events: head pose on its own thread, gaze as it arrives, and status.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use tobii_device::CameraCalibration;
use tobii_device::streams::StreamStats;
use tobii_headpose::pipeline::{DisplayArea, HeadPoseSample};
use tobii_protocol::commands::Stream;
use tobii_protocol::frames::ImageFrame;
use tobii_protocol::gaze::{GazeFrame, Presence, platmod_presence};
use tobii_service::prp_service::UserPresence;

use super::events::{Tracked, frame_event, gaze_columns_event, gaze_event, mesh_topology_event, pose_event, py_round};
use super::wireframe::{FaceWireframe, display_scene};
use crate::error::Result;
use crate::pyjson::{Json, object};

pub const MAX_ERRORS: usize = 20;
pub const GAZE_COLUMNS_INTERVAL: Duration = Duration::from_millis(250);
pub const RATE_WINDOW_S: f64 = 2.0;
const SLOT_WAIT: Duration = Duration::from_millis(250);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// Publishes one event: `(topic, payload, retain)`; must be thread safe.
pub type Publish = Arc<dyn Fn(&str, Json, bool) + Send + Sync>;

/// Name of a user presence property value.
fn presence_state(value: u32) -> &'static str {
    match value {
        2 => "away",
        3 => "present",
        _ => "unknown",
    }
}

/// The gaze point games receive: the tracker's combined point (column 28) where column 27
/// marks it valid.
pub const GAZE_SOURCE: &str = "device";
pub const GAZE_SOURCE_DESCRIPTION: &str = "Combined gaze point computed by the tracker (column 28)";
pub const GAZE_SOURCES: [&str; 1] = [GAZE_SOURCE];

pub fn device_combined_gaze(frame: &GazeFrame) -> Option<[f64; 2]> {
    (frame.gaze_point.valid != 0).then_some(frame.gaze_point.point)
}

/// Events per second over the last two seconds.
#[derive(Debug, Default)]
pub struct RateMeter {
    times: Mutex<VecDeque<Instant>>,
}

impl RateMeter {
    pub fn tick(&self) {
        let now = Instant::now();
        let mut times = lock(&self.times);
        times.push_back(now);
        expire(&mut times, now);
    }

    pub fn rate(&self) -> f64 {
        let now = Instant::now();
        let mut times = lock(&self.times);
        expire(&mut times, now);
        if times.len() < 2 {
            return 0.0;
        }
        let first = times[0];
        let last = *times.back().expect("two times");
        let span = (now - first).as_secs_f64().max((last - first).as_secs_f64());
        if span > 0.0 { (times.len() - 1) as f64 / span } else { 0.0 }
    }
}

fn expire(times: &mut VecDeque<Instant>, now: Instant) {
    while times.front().is_some_and(|&first| (now - first).as_secs_f64() > RATE_WINDOW_S) {
        times.pop_front();
    }
}

/// Holds the newest frame for the processing thread; a frame replaced before it is taken
/// counts as dropped.
#[derive(Debug)]
pub struct LatestSlot<T> {
    item: Mutex<(Option<T>, u64)>,
    changed: Condvar,
}

impl<T> Default for LatestSlot<T> {
    fn default() -> Self {
        Self { item: Mutex::new((None, 0)), changed: Condvar::new() }
    }
}

impl<T> LatestSlot<T> {
    pub fn put(&self, item: T) {
        let mut state = lock(&self.item);
        if state.0.replace(item).is_some() {
            state.1 += 1;
        }
        self.changed.notify_all();
    }

    pub fn take(&self, timeout: Duration) -> Option<T> {
        let state = lock(&self.item);
        let (mut state, _) =
            self.changed.wait_timeout_while(state, timeout, |s| s.0.is_none()).unwrap_or_else(|p| p.into_inner());
        state.0.take()
    }

    pub fn dropped(&self) -> u64 {
        lock(&self.item).1
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(p, q)| (p - q).powi(2)).sum::<f64>().sqrt()
}

pub fn display_area_status(corners: &crate::service::Corners) -> Json {
    let [top_left, top_right, bottom_left] = *corners;
    object([
        ("top_left", top_left.into()),
        ("top_right", top_right.into()),
        ("bottom_left", bottom_left.into()),
        ("width_mm", Json::Float(py_round(distance(top_left, top_right), 1))),
        ("height_mm", Json::Float(py_round(distance(top_left, bottom_left), 1))),
    ])
}

/// One processed frame: what the overlay shows, the sample and the posed wireframe.
#[derive(Debug, Clone, PartialEq)]
pub struct Processed {
    pub tracked: Tracked,
    pub sample: HeadPoseSample,
    pub mesh_pixels: Option<Vec<f32>>,
    pub mesh_millimetres: Option<Vec<f32>>,
}

/// Face tracking, head pose and the fitted wireframe of one frame.
pub trait DemoProcessor: Send {
    fn calibrate(&mut self, calibration: &CameraCalibration);
    /// Whether a calibration has been applied.
    fn calibrated(&self) -> bool;
    fn topology(&self) -> Json;
    fn process(&mut self, frame: &ImageFrame) -> Result<Processed>;
    /// The face tracker's state after the last frame.
    fn tracker_state(&self) -> i64;
}

/// Loads a processor for the model directory and display area.
pub type ProcessorFactory = Box<dyn FnOnce(&std::path::Path, DisplayArea) -> Result<Box<dyn DemoProcessor>> + Send>;

/// FaceTracker and HeadPoseOutput as the head-pose stream runs them, plus the fitted wireframe.
#[derive(Debug)]
pub struct ModelDemoProcessor {
    processor: tobii_headpose::HeadPoseProcessor,
    wireframe: FaceWireframe,
}

impl ModelDemoProcessor {
    pub fn load(model_directory: &std::path::Path, area: DisplayArea) -> Result<Self> {
        let runtime = crate::paths::openvino()?;
        let processor = tobii_headpose::HeadPoseProcessor::new(&runtime, model_directory, area)?;
        let wireframe = FaceWireframe::load(&model_directory.join("support"), &processor.tracker.fitter.model)?;
        Ok(Self { processor, wireframe })
    }

    pub fn factory() -> ProcessorFactory {
        Box::new(|directory, area| Ok(Box::new(Self::load(directory, area)?) as Box<dyn DemoProcessor>))
    }
}

impl DemoProcessor for ModelDemoProcessor {
    fn calibrate(&mut self, calibration: &CameraCalibration) {
        self.processor.start(crate::tracker::vec3(calibration.translation), crate::tracker::vec3(calibration.angles));
    }

    fn calibrated(&self) -> bool {
        self.processor.output.is_some()
    }

    fn topology(&self) -> Json {
        let output = self.processor.output.as_ref().expect("calibrated");
        let (corners, tracker) = display_scene(&self.processor.display_area, &output.display);
        mesh_topology_event(&self.wireframe.edges, self.wireframe.vertices.len(), &corners, tracker)
    }

    fn process(&mut self, frame: &ImageFrame) -> Result<Processed> {
        let (width, height) = (frame.width as usize, frame.height as usize);
        let (tracked, sample) = self.processor.track(&frame.pixels, width, height, frame.timestamp)?;
        let summary = Tracked::from(&tracked);
        let Some(fit) = tracked.fit.as_ref() else {
            return Ok(Processed { tracked: summary, sample, mesh_pixels: None, mesh_millimetres: None });
        };
        let output = self.processor.output.as_ref().expect("tracking needs a calibration");
        let pixels = self.wireframe.image_points(fit, width, height)?;
        let millimetres = self.wireframe.display_points(fit, output, &sample)?;
        Ok(Processed {
            tracked: summary,
            sample,
            mesh_pixels: Some(pixels.into_iter().flatten().collect()),
            mesh_millimetres: Some(millimetres.into_iter().flatten().collect()),
        })
    }

    fn tracker_state(&self) -> i64 {
        self.processor.tracker.state as i64
    }
}

#[derive(Debug, Default, Clone)]
struct Counts {
    frames: u64,
    processed: u64,
    gaze: u64,
    gaze_valid: u64,
    gaze_errors: u64,
}

#[derive(Debug)]
struct State {
    connection: String,
    message: String,
    errors: VecDeque<(f64, String, u64)>,
    device: Vec<(String, Json)>,
    session: Vec<(String, Json)>,
    calibration: Option<CameraCalibration>,
    calibration_generation: u64,
    head_pose_state: String,
    head_pose_message: String,
    presence: Option<Json>,
    user_presence: UserPresence,
    presence_notifications: u64,
    sessions: u64,
    lost_sessions: u64,
    counts: Counts,
    processing_ms: Option<f64>,
    last_columns: Option<Instant>,
    frame_sequence: u64,
    stats: Option<crate::tracker::SharedStats>,
}

/// Receives frames and gaze records from a source thread and publishes browser events.
///
/// Image frames go through a one-frame slot to the processing thread, so frames that arrive
/// while a frame is being processed are dropped and head pose never falls behind.
pub struct DemoPipeline {
    publish: Publish,
    corners: crate::service::Corners,
    model_directory: PathBuf,
    image_interval_s: f64,
    mode: String,
    head_pose_wanted: bool,
    factory: Mutex<Option<ProcessorFactory>>,
    slot: LatestSlot<ImageFrame>,
    state: Mutex<State>,
    rates: [RateMeter; 4],
    started: Instant,
    stopping: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for DemoPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemoPipeline").field("mode", &self.mode).finish_non_exhaustive()
    }
}

const CAMERA: usize = 0;
const PROCESSED: usize = 1;
const GAZE: usize = 2;
const GAZE_VALID: usize = 3;

fn now_seconds() -> f64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl DemoPipeline {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        publish: Publish,
        corners: crate::service::Corners,
        model_directory: PathBuf,
        image_rate: f64,
        mode: &str,
        head_pose: bool,
        factory: ProcessorFactory,
    ) -> Arc<Self> {
        let (head_pose_state, head_pose_message) =
            if head_pose { ("loading", "Loading models") } else { ("disabled", "Head pose disabled") };
        Arc::new(Self {
            publish,
            corners,
            model_directory,
            image_interval_s: if image_rate > 0.0 { 1.0 / image_rate } else { f64::INFINITY },
            mode: mode.into(),
            head_pose_wanted: head_pose,
            factory: Mutex::new(Some(factory)),
            slot: LatestSlot::default(),
            state: Mutex::new(State {
                connection: "starting".into(),
                message: "Starting".into(),
                errors: VecDeque::new(),
                device: Vec::new(),
                session: Vec::new(),
                calibration: None,
                calibration_generation: 0,
                head_pose_state: head_pose_state.into(),
                head_pose_message: head_pose_message.into(),
                presence: None,
                user_presence: UserPresence::default(),
                presence_notifications: 0,
                sessions: 0,
                lost_sessions: 0,
                counts: Counts::default(),
                processing_ms: None,
                last_columns: None,
                frame_sequence: 0,
                stats: None,
            }),
            rates: Default::default(),
            started: Instant::now(),
            stopping: AtomicBool::new(false),
            thread: Mutex::new(None),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let pipeline = Arc::clone(self);
        let thread = std::thread::Builder::new()
            .name("demo-processing".into())
            .spawn(move || pipeline.run())
            .expect("spawn the processing thread");
        *lock(&self.thread) = Some(thread);
    }

    /// Asks the processing thread to finish and waits up to `timeout` for it.
    pub fn stop(&self, timeout: Duration) {
        self.stopping.store(true, Ordering::SeqCst);
        let Some(thread) = lock(&self.thread).take() else { return };
        let deadline = Instant::now() + timeout;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if thread.is_finished() {
            let _ = thread.join();
        }
    }

    pub fn set_connection(&self, connection: &str, message: &str) {
        let mut state = lock(&self.state);
        let changed = (connection, message) != (state.connection.as_str(), state.message.as_str());
        state.connection = connection.into();
        state.message = message.into();
        drop(state);
        if changed {
            log::info!("{connection}: {message}");
        }
    }

    pub fn report_error(&self, message: &str) {
        let mut state = lock(&self.state);
        if let Some(last) = state.errors.back_mut().filter(|last| last.1 == message) {
            last.2 += 1;
            last.0 = now_seconds();
            return;
        }
        state.errors.push_back((now_seconds(), message.into(), 1));
        if state.errors.len() > MAX_ERRORS {
            state.errors.pop_front();
        }
        drop(state);
        log::warn!("{message}");
    }

    pub fn on_session(
        &self,
        calibration: Option<CameraCalibration>,
        device: Vec<(String, Json)>,
        details: Vec<(String, Json)>,
    ) {
        let mut state = lock(&self.state);
        state.sessions += 1;
        state.device = device;
        state.session = details;
        state.calibration = calibration;
        state.calibration_generation += 1;
    }

    pub fn set_stats(&self, stats: Option<crate::tracker::SharedStats>) {
        lock(&self.state).stats = stats;
    }

    pub fn on_frame(&self, frame: ImageFrame) {
        lock(&self.state).counts.frames += 1;
        self.rates[CAMERA].tick();
        self.slot.put(frame);
    }

    /// A device presence record, kept as the game service keeps property 0xB.
    pub fn on_presence(&self, presence: Presence) {
        let mut state = lock(&self.state);
        if state.user_presence.record(presence.timestamp_us, platmod_presence(i64::from(presence.value))) {
            state.presence_notifications += 1;
        }
        let value = state.user_presence.value.unwrap_or_default();
        state.presence = Some(object([
            ("value", presence.value.into()),
            ("state", presence_state(value).into()),
            ("property_value", value.into()),
            ("since_us", state.user_presence.since_us.into()),
            ("notifications", state.presence_notifications.into()),
        ]));
    }

    /// A new session subscribed presence: unknown until the device reports, as on Windows.
    pub fn on_presence_subscribed(&self) {
        lock(&self.state).user_presence.clear();
        self.on_presence(Presence { timestamp_us: 0, value: 0 });
    }

    pub fn on_session_lost(&self) {
        lock(&self.state).lost_sessions += 1;
    }

    pub fn on_gaze(&self, frame: &GazeFrame) {
        lock(&self.state).counts.gaze += 1;
        self.rates[GAZE].tick();
        let point = device_combined_gaze(frame);
        if point.is_some() {
            lock(&self.state).counts.gaze_valid += 1;
            self.rates[GAZE_VALID].tick();
        }
        (self.publish)("gaze", gaze_event(frame, point, GAZE_SOURCE), false);
        let now = Instant::now();
        let due = {
            let mut state = lock(&self.state);
            let due = state.last_columns.is_none_or(|last| now - last >= GAZE_COLUMNS_INTERVAL);
            if due {
                state.last_columns = Some(now);
            }
            due
        };
        if due {
            (self.publish)("gaze_columns", gaze_columns_event(frame), false);
        }
    }

    fn head_pose(&self, state: &str, message: &str) {
        let mut locked = lock(&self.state);
        locked.head_pose_state = state.into();
        locked.head_pose_message = message.into();
    }

    fn load_processor(&self) -> Option<Box<dyn DemoProcessor>> {
        let factory = lock(&self.factory).take()?;
        if !self.head_pose_wanted {
            return None;
        }
        let area = crate::cli::display_area(&self.corners);
        match factory(&self.model_directory, area) {
            Ok(processor) => {
                self.head_pose("waiting", "Waiting for the camera calibration");
                Some(processor)
            }
            Err(error) => {
                let message = format!("Head pose unavailable: {}", error.described());
                self.head_pose("unavailable", &message);
                self.report_error(&message);
                None
            }
        }
    }

    fn run(&self) {
        let mut processor = self.load_processor();
        let mut generation = 0;
        let mut next_image: Option<Instant> = None;
        let interval = Duration::try_from_secs_f64(self.image_interval_s).ok();
        while !self.stopping.load(Ordering::SeqCst) {
            let Some(frame) = self.slot.take(SLOT_WAIT) else { continue };
            if let Some(processor) = processor.as_mut() {
                let (current, calibration) = {
                    let state = lock(&self.state);
                    (state.calibration_generation, state.calibration)
                };
                if generation != current {
                    generation = current;
                    match calibration {
                        Some(calibration) => {
                            processor.calibrate(&calibration);
                            (self.publish)("mesh_topology", processor.topology(), true);
                            self.head_pose("running", "Running");
                        }
                        None => self.head_pose("waiting", "No camera calibration for this source"),
                    }
                }
            }
            let processed = match processor.as_mut() {
                Some(processor) if processor.calibrated() => self.process(processor.as_mut(), &frame),
                _ => None,
            };
            let now = Instant::now();
            let due = next_image.is_none_or(|next| now >= next);
            if due {
                next_image = match (interval, next_image) {
                    (None, _) => Some(now + Duration::from_secs(u32::MAX.into())),
                    (Some(interval), Some(next)) if next + interval >= now => Some(next + interval),
                    (Some(interval), _) => Some(now + interval),
                };
                let sequence = {
                    let mut state = lock(&self.state);
                    state.frame_sequence += 1;
                    state.frame_sequence
                };
                let tracked = processed.as_ref().map(|(processed, _)| &processed.tracked);
                let state = processed.as_ref().map(|(_, state)| *state);
                let mesh = processed.as_ref().and_then(|(processed, _)| processed.mesh_pixels.as_deref());
                match frame_event(sequence, &frame, tracked, state, mesh) {
                    Ok(event) => (self.publish)("frame", event, true),
                    Err(error) => self.report_error(&format!("Camera image: ValueError: {error}")),
                }
            }
        }
    }

    fn process(&self, processor: &mut dyn DemoProcessor, frame: &ImageFrame) -> Option<(Processed, i64)> {
        let started = Instant::now();
        let processed = match processor.process(frame) {
            Ok(processed) => processed,
            Err(error) => {
                self.report_error(&format!("Head pose: {}", error.described()));
                return None;
            }
        };
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        {
            let mut state = lock(&self.state);
            state.processing_ms = Some(match state.processing_ms {
                None => elapsed_ms,
                Some(previous) => 0.9 * previous + 0.1 * elapsed_ms,
            });
            state.counts.processed += 1;
        }
        self.rates[PROCESSED].tick();
        let tracker_state = processor.tracker_state();
        let event = pose_event(
            &processed.sample,
            &processed.tracked,
            Some(tracker_state),
            elapsed_ms,
            processed.mesh_millimetres.as_deref(),
        );
        (self.publish)("pose", event, false);
        Some((processed, tracker_state))
    }

    pub fn status(&self) -> Json {
        let state = lock(&self.state);
        let stats: StreamStats = state.stats.as_ref().map(|stats| lock(stats).clone()).unwrap_or_default();
        let reading = state.stats.is_some();
        let errors = state
            .errors
            .iter()
            .map(|(time, message, count)| {
                object([("time", Json::Float(*time)), ("message", message.as_str().into()), ("count", (*count).into())])
            })
            .collect();
        let counts = &state.counts;
        let count = |map: &std::collections::BTreeMap<Stream, u64>, stream| map.get(&stream).copied().unwrap_or(0);
        let reader_count = |value: u64| if reading { Json::from(value) } else { Json::Int(0) };
        let rate = |meter: &RateMeter| Json::Float(py_round(meter.rate(), 1));
        object([
            ("mode", self.mode.as_str().into()),
            ("connection", state.connection.as_str().into()),
            ("message", state.message.as_str().into()),
            ("device", Json::Object(state.device.clone())),
            ("session", Json::Object(state.session.clone())),
            (
                "head_pose",
                object([
                    ("state", state.head_pose_state.as_str().into()),
                    ("message", state.head_pose_message.as_str().into()),
                ]),
            ),
            ("presence", state.presence.clone().unwrap_or(Json::Null)),
            ("errors", Json::List(errors)),
            ("sessions", state.sessions.into()),
            ("lost_sessions", state.lost_sessions.into()),
            ("uptime_s", Json::Float(py_round(self.started.elapsed().as_secs_f64(), 1))),
            ("display_area", display_area_status(&self.corners)),
            ("gaze_source", object([("name", GAZE_SOURCE.into()), ("description", GAZE_SOURCE_DESCRIPTION.into())])),
            (
                "image_rate_hz",
                if self.image_interval_s > 0.0 {
                    Json::Float(py_round(1.0 / self.image_interval_s, 2))
                } else {
                    Json::Int(0)
                },
            ),
            (
                "rates",
                object([
                    ("camera", rate(&self.rates[CAMERA])),
                    ("processed", rate(&self.rates[PROCESSED])),
                    ("gaze", rate(&self.rates[GAZE])),
                    ("gaze_valid", rate(&self.rates[GAZE_VALID])),
                ]),
            ),
            (
                "counters",
                object([
                    ("frames", counts.frames.into()),
                    ("processed", counts.processed.into()),
                    ("gaze", counts.gaze.into()),
                    ("gaze_valid", counts.gaze_valid.into()),
                    ("gaze_errors", counts.gaze_errors.into()),
                    ("dropped_frames", self.slot.dropped().into()),
                    ("decode_errors", reader_count(stats.decode_errors.values().sum())),
                    ("gaze_dropped", reader_count(count(&stats.dropped, Stream::Gaze))),
                    ("presence", reader_count(count(&stats.received, Stream::Presence))),
                    ("other_stream_messages", reader_count(stats.unexpected)),
                ]),
            ),
            ("processing_ms", state.processing_ms.map_or(Json::Null, |ms| Json::Float(py_round(ms, 1)))),
            ("last_decode_error", stats.last_error.map_or(Json::Null, Json::Str)),
        ])
    }
}
