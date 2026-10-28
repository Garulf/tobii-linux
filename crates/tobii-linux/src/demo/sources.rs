//! Where the demo's data comes from: a live tracker over USB, or recorded logs replayed at
//! their own pace.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tobii_device::streams::{HostSuspendWatch, StreamReader, StreamValue};
use tobii_device::supervisor::Backoff;
use tobii_device::{CameraCalibration, Channel, DeviceError, StopFlag, StopSignal, TrackerSession};
use tobii_protocol::Message;
use tobii_protocol::commands::{STREAM_DATA_KIND, Stream, command, header_only_request};
use tobii_protocol::fields::Value;
use tobii_protocol::frames::ImageFrame;
use tobii_protocol::gaze::{GazeFrame, gaze_message};

use super::pipeline::DemoPipeline;
use crate::error::{Error, Result};
use crate::pyjson::Json;

pub const DEVICE_INFO_FIELDS: [&str; 4] = ["serial_number", "model", "generation", "firmware_version"];
pub const NO_TRACKER_HINT: &str =
    "On WSL attach it with 'usbipd attach --wsl --busid <id>'; on Linux check the cable and the udev rule.";
pub const MAX_REPLAY_GAP_S: f64 = 0.5;
const DRAIN_DEPTH: usize = 256;

fn printable(value: &Value) -> Json {
    match value {
        Value::Int(value) => Json::Int(*value),
        Value::UInt(value) => Json::Int(*value as i64),
        Value::Float(value) => Json::Float(*value),
        Value::Bytes(bytes) => Json::Str(String::from_utf8_lossy(bytes).into_owned()),
        Value::Struct(header) => {
            Json::Str(format!("StructHeader(item_count={}, struct_id={})", header.item_count, header.struct_id))
        }
        Value::UIntArray(values) => Json::Str(tuple_repr(values.iter().map(u32::to_string))),
        Value::FloatArray(values) => Json::Str(tuple_repr(values.iter().map(|v| crate::pyjson::float_repr(*v)))),
        Value::BytesArray(items) => Json::Str(tuple_repr(items.iter().map(|item| bytes_repr(item)))),
    }
}

fn tuple_repr(items: impl Iterator<Item = String>) -> String {
    let items: Vec<String> = items.collect();
    if items.len() == 1 { format!("({},)", items[0]) } else { format!("({})", items.join(", ")) }
}

fn bytes_repr(data: &[u8]) -> String {
    let mut out = String::from("b'");
    for &byte in data {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push('\'');
    out
}

/// GET_DEVICE_INFO (1420) reply values, named in the order the reply carries them.
pub fn device_info<C: Channel>(session: &mut TrackerSession<C>) -> Result<Vec<(String, Json)>> {
    let reply = session.try_call(|id| header_only_request(id, command::GET_DEVICE_INFO))?;
    let values = TrackerSession::<C>::reply_values(&reply)?;
    Ok(values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let name = DEVICE_INFO_FIELDS.get(index).map_or_else(|| format!("value_{index}"), |name| name.to_string());
            (name, printable(value))
        })
        .collect())
}

/// Runs a source on a thread; `stop` asks it to finish and `join` waits for it.
#[derive(Debug, Default)]
pub struct SourceThread {
    pub stop: StopFlag,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl SourceThread {
    pub fn start(&self, name: &str, run: impl FnOnce(StopFlag) + Send + 'static) {
        let stop = self.stop.clone();
        let thread = std::thread::Builder::new().name(format!("demo-{name}")).spawn(move || run(stop)).expect("spawn");
        *self.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(thread);
    }

    /// Waits up to `timeout`; true when the thread has finished.
    pub fn join(&self, timeout: Duration) -> bool {
        let Some(thread) = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take() else { return true };
        let deadline = Instant::now() + timeout;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let finished = thread.is_finished();
        if finished {
            let _ = thread.join();
        }
        finished
    }
}

/// Opens a started session: the USB tracker, or a scripted one in tests.
pub type OpenSession<C> = Box<dyn FnMut() -> Result<(Vec<(String, Json)>, TrackerSession<C>)> + Send>;

/// One session subscribed to gaze, images and presence; starts a new one after any failure.
///
/// The endpoint is read on its own thread so slow browser publishing never stalls the
/// tracker; a silent session, a restarted tracker or a host resume ends the session, and
/// the next attempt follows a growing delay.
pub fn run_live<C: Channel>(
    pipeline: &DemoPipeline,
    open: &mut OpenSession<C>,
    channel: i64,
    mut backoff: Backoff,
    stop: &StopFlag,
) {
    while !stop.is_set() {
        let started = Instant::now();
        pipeline.set_connection("connecting", "Starting the tracker");
        let outcome = open().and_then(|(usb, mut session)| live_session(pipeline, &mut session, usb, channel, stop));
        match outcome {
            Ok(()) => {}
            Err(Error::Device(DeviceError::NoTracker(message))) => {
                pipeline.set_connection("no-device", &format!("{message}. {NO_TRACKER_HINT}"));
            }
            Err(error) => {
                let message = error.described();
                pipeline.report_error(&message);
                pipeline.set_connection("error", &message);
                pipeline.on_session_lost();
            }
        }
        if !stop.is_set() {
            stop.wait(Duration::from_secs_f64(backoff.delay(started.elapsed().as_secs_f64())));
        }
    }
    pipeline.set_connection("stopped", "Stopped");
}

fn live_session<C: Channel>(
    pipeline: &DemoPipeline,
    session: &mut TrackerSession<C>,
    usb: Vec<(String, Json)>,
    channel: i64,
    stop: &StopFlag,
) -> Result<()> {
    let info = device_info(session)?;
    let calibration = session.camera_calibration(0)?;
    let mut details = usb;
    details.push(("channel".into(), channel.into()));
    details.push(("camera_translation".into(), calibration.translation.into()));
    details.push(("camera_angles".into(), calibration.angles.into()));
    pipeline.on_session(Some(calibration), info, details);
    let stats = crate::tracker::SharedStats::default();
    pipeline.set_stats(Some(Arc::clone(&stats)));
    if stop.is_set() {
        return Ok(());
    }
    let mut reader = StreamReader::new(&[Stream::Gaze, Stream::Image, Stream::Presence], &[Stream::Presence]);
    reader.subscribe(session)?;
    if reader.subscribed.contains(&Stream::Presence) {
        pipeline.on_presence_subscribed();
    }
    pipeline.set_connection("streaming", "Streaming gaze and images");
    let suspend = Box::new(HostSuspendWatch::default());
    let outcome = crate::tracker::drained_items(
        &mut reader,
        session,
        Some(suspend),
        stop,
        &stats,
        DRAIN_DEPTH,
        |item| match item.value {
            StreamValue::Image(frame) => pipeline.on_frame(frame),
            StreamValue::Gaze(frame) => pipeline.on_gaze(&frame),
            StreamValue::Presence(presence) => pipeline.on_presence(presence),
            StreamValue::Raw => {}
        },
    );
    if let Err(Error::Device(error)) = &outcome
        && error.is_device_error()
    {
        session.set_dead(true);
    }
    crate::tracker::unsubscribe_after(&mut reader, session, outcome)?;
    pipeline.set_connection("stopping", "Unsubscribed the tracker streams");
    Ok(())
}

/// The USB session of `index` and `channel`, with the bus and address the status shows.
pub fn usb_opener(index: i64, channel: i64) -> OpenSession<tobii_usb::UsbChannel> {
    Box::new(move || {
        let session = crate::cli::Session { corners: None, models: PathBuf::new(), channel, index };
        let (device, tracker) = session.open()?;
        Ok((vec![("bus".into(), device.bus.into()), ("address".into(), device.address.into())], tracker))
    })
}

/// One recorded frame row: timestamp, width, height and the file with its pixels.
pub type FrameRow = (i64, u32, u32, PathBuf);

fn json_int(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_f64().map(|v| v as i64)).or_else(|| value.as_str()?.trim().parse().ok())
}

/// Calibration and frame rows from a linux_session_probe.py log.
pub fn recorded_frames(
    log_path: &Path,
    frames_dir: Option<&Path>,
) -> Result<(Option<CameraCalibration>, Vec<FrameRow>)> {
    let frames_dir =
        frames_dir.map_or_else(|| log_path.parent().unwrap_or(Path::new("")).join("frames"), Path::to_path_buf);
    let text = std::fs::read_to_string(log_path)?;
    let mut calibration = None;
    let mut frames = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let row: serde_json::Value =
            serde_json::from_str(line).map_err(|error| Error::Value(format!("{}: {error}", log_path.display())))?;
        let kind = row.get("kind").and_then(|v| v.as_str());
        if kind == Some("step") && row.get("name").and_then(|v| v.as_str()) == Some("calibration") {
            if let (Some(translation), Some(angles)) = (row.get("translation"), row.get("angles")) {
                let triple = |value: &serde_json::Value| -> Option<[f64; 3]> {
                    let values: Vec<f64> = value.as_array()?.iter().filter_map(|v| v.as_f64()).collect();
                    values.try_into().ok()
                };
                if let (Some(translation), Some(angles)) = (triple(translation), triple(angles)) {
                    calibration = Some(CameraCalibration { translation, angles });
                }
            }
        } else if kind == Some("frame") {
            let field = |name: &str| row.get(name).and_then(json_int);
            let path = row.get("path").and_then(|v| v.as_str()).unwrap_or_default().replace('\\', "/");
            let name = Path::new(&path).file_name().map(PathBuf::from).unwrap_or_default();
            let (Some(timestamp), Some(width), Some(height)) = (field("timestamp"), field("width"), field("height"))
            else {
                return Err(Error::Value(format!("{}: malformed frame row", log_path.display())));
            };
            frames.push((timestamp, width as u32, height as u32, frames_dir.join(name)));
        }
    }
    if frames.is_empty() {
        return Err(Error::Value(format!("{} has no frame rows", log_path.display())));
    }
    Ok((calibration, frames))
}

/// Every 0x500 message in a capture JSONL file, as tools/linux_stream_capture.py writes them.
pub fn recorded_gaze_messages(log_path: &Path) -> Result<Vec<Message>> {
    let text = std::fs::read_to_string(log_path)?;
    let gaze_wire = Stream::Gaze.wire_id();
    let mut messages = Vec::new();
    for line in text.lines() {
        let row: serde_json::Value =
            serde_json::from_str(line).map_err(|error| Error::Value(format!("{}: {error}", log_path.display())))?;
        if row.get("kind").and_then(|v| v.as_str()) != Some("message")
            || row.get("command").and_then(serde_json::Value::as_u64) != Some(u64::from(gaze_wire))
        {
            continue;
        }
        let word = |name: &str, default: u64| row.get(name).and_then(serde_json::Value::as_u64).unwrap_or(default);
        let payload = row.get("payload").and_then(|v| v.as_str()).unwrap_or_default();
        let payload =
            hex_bytes(payload).ok_or_else(|| Error::Value("non-hexadecimal number found in fromhex() arg".into()))?;
        messages.push(Message::new(
            word("message_kind", u64::from(STREAM_DATA_KIND)) as u32,
            0,
            word("word2", 0) as u32,
            gaze_wire,
            word("word4", 0) as u32,
            payload,
        ));
    }
    Ok(messages)
}

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits.chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()).collect()
}

/// Gaze frames of every recorded gaze message the runtime would keep, and how many it drops.
pub fn recorded_gaze(log_path: &Path) -> Result<(Vec<GazeFrame>, usize)> {
    let messages = recorded_gaze_messages(log_path)?;
    let frames: Vec<GazeFrame> =
        messages.iter().filter_map(|message| gaze_message(message).ok().flatten()).map(GazeFrame::new).collect();
    if frames.is_empty() {
        return Err(Error::Value(format!("{} has no usable gaze messages", log_path.display())));
    }
    Ok((frames.clone(), messages.len() - frames.len()))
}

/// Seconds from the first item to each item, with long recording gaps shortened.
pub fn replay_offsets_s(timestamps: &[i64]) -> Vec<f64> {
    let mut offsets = vec![0.0];
    for pair in timestamps.windows(2) {
        let gap = ((pair[1] - pair[0]).max(0) as f64 / 1e6).min(MAX_REPLAY_GAP_S);
        offsets.push(offsets.last().expect("one offset") + gap);
    }
    offsets
}

/// Python's `statistics.median` of the positive intervals, in seconds; 0.03 without any.
pub fn typical_interval_s(timestamps: &[i64]) -> f64 {
    let mut intervals: Vec<i64> = timestamps.windows(2).filter(|p| p[1] > p[0]).map(|p| p[1] - p[0]).collect();
    if intervals.is_empty() {
        return 0.03;
    }
    intervals.sort_unstable();
    let middle = intervals.len() / 2;
    let median = if intervals.len() % 2 == 1 {
        intervals[middle] as f64
    } else {
        (intervals[middle - 1] + intervals[middle]) as f64 / 2.0
    };
    median / 1e6
}

/// Emits `items` in a loop at their recorded timing, shifting timestamps so they keep increasing.
pub fn play<T: Clone>(
    items: &[T],
    timestamp_of: impl Fn(&T) -> i64,
    mut emit: impl FnMut(T),
    stop: &dyn StopSignal,
    shift: impl Fn(&T, i64) -> T,
) {
    let timestamps: Vec<i64> = items.iter().map(&timestamp_of).collect();
    let offsets = replay_offsets_s(&timestamps);
    let typical = typical_interval_s(&timestamps);
    let loop_s = offsets.last().copied().unwrap_or(0.0) + typical;
    let loop_shift_us = timestamps.last().copied().unwrap_or(0) - timestamps[0] + (typical * 1e6) as i64;
    let mut round = 0i64;
    while !stop.is_set() {
        let start = Instant::now();
        for (item, offset) in items.iter().zip(&offsets) {
            let due = start + Duration::from_secs_f64(*offset);
            if stop.wait(due.saturating_duration_since(Instant::now())) {
                return;
            }
            emit(shift(item, round * loop_shift_us));
        }
        let due = start + Duration::from_secs_f64(loop_s);
        if stop.wait(due.saturating_duration_since(Instant::now())) {
            return;
        }
        round += 1;
    }
}

/// Recorded camera frames and gaze messages, each looped at its recorded rate.
#[derive(Debug, Clone)]
pub struct ReplaySource {
    pub calibration: Option<CameraCalibration>,
    pub frames: Vec<ImageFrame>,
    pub gaze: Vec<GazeFrame>,
    pub details: Vec<(String, Json)>,
}

impl ReplaySource {
    pub fn load(frames_log: Option<&Path>, frames_dir: Option<&Path>, gaze_log: Option<&Path>) -> Result<Self> {
        if frames_log.is_none() && gaze_log.is_none() {
            return Err(Error::Value("Replay needs a frames log, a gaze log or both".into()));
        }
        let mut source = Self { calibration: None, frames: Vec::new(), gaze: Vec::new(), details: Vec::new() };
        if let Some(log) = frames_log {
            let (calibration, rows) = recorded_frames(log, frames_dir)?;
            source.calibration = calibration;
            for (timestamp, width, height, path) in rows {
                let pixels = std::fs::read(&path)?;
                source.frames.push(ImageFrame { timestamp, pixel_format: 8, width, height, pixels });
            }
            source.details.push(("frames_log".into(), log.display().to_string().into()));
            source.details.push(("frames".into(), source.frames.len().into()));
        }
        if let Some(log) = gaze_log {
            let (gaze, skipped) = recorded_gaze(log)?;
            source.gaze = gaze;
            source.details.push(("gaze_log".into(), log.display().to_string().into()));
            source.details.push(("gaze_records".into(), source.gaze.len().into()));
            source.details.push(("gaze_skipped".into(), skipped.into()));
        }
        Ok(source)
    }

    pub fn run(&self, pipeline: &Arc<DemoPipeline>, stop: &StopFlag) {
        let device = vec![("model".into(), "Recorded session".into()), ("serial_number".into(), "replay".into())];
        pipeline.on_session(self.calibration, device, self.details.clone());
        pipeline.set_connection("streaming", "Replaying recorded data");
        std::thread::scope(|scope| {
            if !self.frames.is_empty() {
                let (pipeline, stop) = (Arc::clone(pipeline), stop.clone());
                std::thread::Builder::new()
                    .name("demo-replay-frames".into())
                    .spawn_scoped(scope, move || {
                        play(
                            &self.frames,
                            |frame| frame.timestamp,
                            |frame| pipeline.on_frame(frame),
                            &stop,
                            |frame, offset| ImageFrame { timestamp: frame.timestamp + offset, ..frame.clone() },
                        );
                    })
                    .expect("spawn");
            }
            if !self.gaze.is_empty() {
                let (pipeline, stop) = (Arc::clone(pipeline), stop.clone());
                std::thread::Builder::new()
                    .name("demo-replay-gaze".into())
                    .spawn_scoped(scope, move || {
                        play(
                            &self.gaze,
                            GazeFrame::timestamp_us,
                            |frame| pipeline.on_gaze(&frame),
                            &stop,
                            |frame, offset| frame.with_timestamp(frame.timestamp_us() + offset),
                        );
                    })
                    .expect("spawn");
            }
        });
        pipeline.set_connection("stopped", "Stopped");
    }
}
