//! User gaze calibration: backup, restore and the Tobii Experience sequence.
//!
//! The device computes gaze and keeps the active calibration itself (docs/gaze-device.md).
//! Calibrating means driving its calibration commands in the order Tobii's configuration
//! app does through Stream Engine and the platform runtime; docs/gaze-calibration.md has the
//! evidence for every step. [`backup_calibration`] and [`verify_backup`] only read;
//! [`restore_calibration`] and [`Calibrator`] write to the device.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tobii_protocol::Message;
use tobii_protocol::commands::{
    STREAM_DATA_KIND, Stream, command, header_only_request, subscribe_stream, unsubscribe_stream,
};
use tobii_protocol::fields::Value;
use tobii_protocol::gaze::{DeviceGaze, gaze_message};

use crate::device::{self, CalibrationPointData, Eye, LocalCalibration, StimulusPoint, gaze_command as gc, reply_blob};
use crate::digest::sha256_hex;
use crate::error::{GazeError, Result};
use crate::math::{dist, float32, hypot, round_digits, sum};
use crate::operation::{External, Io, LogValue, OperationError, OperationResult};

pub const COMMAND_TIMEOUT_S: f64 = 3.0;
pub const TRANSFER_TIMEOUT_S: f64 = 15.0;
pub const COLLECT_TIMEOUT_S: f64 = 15.0;
pub const COMPUTE_TIMEOUT_S: f64 = 30.0;

pub const INTERACTIVE_POINTS: [[f64; 2]; 7] =
    [[0.5, 0.5], [0.1, 0.9], [0.5, 0.1], [0.9, 0.9], [0.1, 0.1], [0.5, 0.9], [0.9, 0.1]];
pub const STAGES: [&[usize]; 3] = [&[0], &[1, 2, 3], &[4, 5, 6]];

pub const MAX_CALIBRATION_AREA_MM: [f64; 2] = [600.0, 340.0];
pub const MAX_DIAGONAL_MM: f64 = 690.0;
pub const MAX_HEIGHT_MM: f64 = 350.0;
pub const DISPLAY_TOLERANCE_MM: f64 = 10.0;

pub const ZONE_RADIUS_FACTOR: f64 = 0.45;
pub const FOCUS_CHANGE_MS: f64 = 150.0;
pub const GAZE_LOST_MS: f64 = 250.0;
pub const EYE_VALID_BELOW: u32 = 2;
pub const READ_SLICE_MS: u32 = 100;
pub const FIXATION_WINDOW_MS: f64 = 300.0;
pub const FIXATION_SPREAD: f64 = 0.03;
pub const FIXATION_MIN_SAMPLES: usize = 6;
pub const FIXATION_TIMEOUT_S: f64 = 4.0;
pub const SACCADE_S: f64 = 0.2;
pub const ATTENTION_STALL_MS: f64 = 4000.0;
pub const ATTENTION_NO_POP_MS: f64 = 10000.0;
pub const USER_AWAY_MS: f64 = 500.0;
pub const MAX_STALLED_RUNS: usize = 5;
pub const STAGES_STEP: &str = "targets";
/// The device status of a compute it refuses, the Tobii app's "all points failed".
pub const OPERATION_FAILED: &str = "PROTOCOL_OPERATION_FAILED";

/// Commands a dry run forwards to the device: the gaze reads and the status queries.
pub const READ_COMMANDS: [u32; 9] = [
    gc::CALIBRATION_RETRIEVE,
    gc::GET_CALIBRATION_STIMULUS_POINTS,
    gc::GET_CONFIGURATION_KEY,
    gc::PERSISTENT_FILE_ENUMERATE,
    gc::PERSISTENT_FILE_READ,
    command::GET_DEVICE_STATUS,
    command::GET_DISPLAY_AREA,
    command::GET_DEVICE_INFO,
    command::GET_ENABLED_EYE,
];

/// Normalized (x, y, width, height) the targets span, as CalibrationAreaCalculator chooses it.
///
/// Displays up to 690 mm diagonal and 350 mm height (10 mm tolerance) use the whole screen;
/// larger ones use a centred 600 x 340 mm area.
pub fn calibration_area([width, height]: [f64; 2]) -> [f64; 4] {
    if hypot(&[width, height]) < MAX_DIAGONAL_MM + DISPLAY_TOLERANCE_MM && height < MAX_HEIGHT_MM + DISPLAY_TOLERANCE_MM
    {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let area_w = crate::math::py_min(1.0, MAX_CALIBRATION_AREA_MM[0] / width);
    let area_h = crate::math::py_min(1.0, MAX_CALIBRATION_AREA_MM[1] / height);
    [(1.0 - area_w) / 2.0, (1.0 - area_h) / 2.0, area_w, area_h]
}

/// Targets in normalized display coordinates, scaled into the calibration area like ScalePoints.
pub fn calibration_targets(display_mm: Option<[f64; 2]>, points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let [x0, y0, w, h] = display_mm.map_or([0.0, 0.0, 1.0, 1.0], calibration_area);
    points.iter().map(|[x, y]| [float32(x0 + x * w), float32(y0 + y * h)]).collect()
}

/// Width and height in mm from a 1430 reply: top-left, top-right, bottom-left corners.
pub fn display_size_mm(values: &[Value]) -> Result<[f64; 2]> {
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|value| match value {
            Value::Float(x) => Some(*x),
            _ => None,
        })
        .collect();
    if numbers.len() < 9 {
        return Err(GazeError::Calibration("Display area reply holds fewer than three corners".into()));
    }
    Ok([numbers[3] - numbers[0], dist(&numbers[0..3], &numbers[6..9])])
}

pub fn save_backup(path: &Path, calibration: &LocalCalibration, overwrite: bool) -> Result<PathBuf> {
    if path.exists() && !overwrite {
        return Err(GazeError::file_exists(path));
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    let temporary = path.with_file_name(name);
    std::fs::write(&temporary, device::calibration_setting(calibration)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(path.to_path_buf())
}

pub fn load_backup(path: &Path) -> Result<LocalCalibration> {
    Ok(device::parse_calibration_setting(&std::fs::read(path)?)?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupCheck {
    pub file_id: u32,
    pub device_id: u32,
    pub file_blob_sha256: String,
    pub device_blob_sha256: String,
}

impl BackupCheck {
    pub fn new(file: &LocalCalibration, device: &LocalCalibration) -> Self {
        Self {
            file_id: file.calibration_id,
            device_id: device.calibration_id,
            file_blob_sha256: sha256_hex(&file.blob),
            device_blob_sha256: sha256_hex(&device.blob),
        }
    }

    pub fn ids_match(&self) -> bool {
        self.file_id == self.device_id
    }

    pub fn blob_matches(&self) -> bool {
        self.file_blob_sha256 == self.device_blob_sha256
    }

    pub fn ok(&self) -> bool {
        self.ids_match() && self.blob_matches()
    }
}

fn status_request() -> Message {
    header_only_request(0, command::GET_DEVICE_STATUS).expect("1490 is header only")
}

fn gaze_request(command: u32) -> Message {
    device::header_only(0, command).expect("header-only gaze command")
}

pub async fn read_display_size<E: External>(io: &Io<E>) -> OperationResult<[f64; 2], E> {
    let request = header_only_request(0, command::GET_DISPLAY_AREA)?;
    let reply = io.call(request, Some(COMMAND_TIMEOUT_S)).await.map_err(OperationError::External)?;
    Ok(display_size_mm(&reply)?)
}

pub async fn read_device_calibration_id<E: External>(io: &Io<E>) -> OperationResult<u32, E> {
    let reply = io.call(status_request(), Some(COMMAND_TIMEOUT_S)).await.map_err(OperationError::External)?;
    Ok(device::device_calibration_id(&reply)?.unwrap_or(0))
}

pub async fn retrieve_blob<E: External>(io: &Io<E>) -> OperationResult<Vec<u8>, E> {
    let request = gaze_request(gc::CALIBRATION_RETRIEVE);
    let reply = io.call(request, Some(TRANSFER_TIMEOUT_S)).await.map_err(OperationError::External)?;
    reply_blob(&reply).ok_or_else(|| OperationError::calibration("Calibration reply must hold one blob"))
}

pub async fn read_stimulus_points<E: External>(io: &Io<E>) -> OperationResult<Vec<StimulusPoint>, E> {
    let request = gaze_request(gc::GET_CALIBRATION_STIMULUS_POINTS);
    let reply = io.call(request, Some(COMMAND_TIMEOUT_S)).await.map_err(OperationError::External)?;
    Ok(device::stimulus_points(&reply)?)
}

/// Read-only copy of the device calibration with the id the runtime would store with it.
///
/// The id is read before and after the transfer so a calibration that changes in between is
/// reported instead of saved with the wrong id.
pub async fn backup_calibration<E: External>(io: &Io<E>) -> OperationResult<LocalCalibration, E> {
    let before = read_device_calibration_id(io).await?;
    let blob = retrieve_blob(io).await?;
    let after = read_device_calibration_id(io).await?;
    if before != after {
        return Err(OperationError::calibration(format!(
            "Device calibration id changed during backup ({before:#x} -> {after:#x})"
        )));
    }
    Ok(LocalCalibration { calibration_id: after, blob })
}

/// Compares a backup with the device's current id and blob; reads only.
pub async fn verify_backup<E: External>(io: &Io<E>, calibration: &LocalCalibration) -> OperationResult<BackupCheck, E> {
    let current = backup_calibration(io).await?;
    Ok(BackupCheck::new(calibration, &current))
}

/// Applies a backup (1110) and confirms the device now reports its id; returns that id.
///
/// This is what the runtime does at start when its calibration.setpm id differs from the
/// device ("Restoring calibration from disk"). The device takes the id from the blob itself.
pub async fn restore_calibration<E: External>(io: &Io<E>, calibration: &LocalCalibration) -> OperationResult<u32, E> {
    let request = device::calibration_apply(0, &calibration.blob)?;
    io.call(request, Some(TRANSFER_TIMEOUT_S)).await.map_err(OperationError::External)?;
    let device_id = read_device_calibration_id(io).await?;
    if device_id != calibration.calibration_id {
        return Err(OperationError::calibration(format!(
            "Device reports calibration id {device_id:#x} after restoring {:#x}",
            calibration.calibration_id
        )));
    }
    Ok(device_id)
}

/// What the display shows; each event blocks until shown while the device is still read.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// Show target `index` at normalized (x, y) and return once the user's eye is settled on it.
    ShowPoint { index: usize, x: f64, y: f64 },
    /// Collection for the target ended; `accepted` is false when the data was discarded.
    PointDone { index: usize, accepted: bool },
    /// The device computed and applied the calibration from all targets so far.
    StageDone { stage: usize },
    /// A compute failed or the targets were not looked at; the calibration starts again.
    Restarting { attempt: usize },
    /// The device is ready for targets (after 1010 and 1060); may block for an introduction.
    CalibrationStarted { attempt: usize },
    /// Gaze-chosen order: show a stage's targets, (index, x, y), and return once drawn.
    ShowTargets { stage: usize, targets: Vec<(usize, f64, f64)> },
    /// Gaze-chosen order: the gaze settled on target `index` (focused) or left it.
    Focus { index: usize, focused: bool },
    /// The device computes the calibration from the targets so far (1070); StageDone follows.
    Computing { stage: usize },
}

impl UiEvent {
    /// The `CalibrationUI` method the Python wrappers call.
    pub fn name(&self) -> &'static str {
        match self {
            UiEvent::ShowPoint { .. } => "show_point",
            UiEvent::PointDone { .. } => "point_done",
            UiEvent::StageDone { .. } => "stage_done",
            UiEvent::Restarting { .. } => "restarting",
            UiEvent::CalibrationStarted { .. } => "calibration_started",
            UiEvent::ShowTargets { .. } => "show_targets",
            UiEvent::Focus { .. } => "focus",
            UiEvent::Computing { .. } => "computing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GazeSample {
    pub timestamp_us: i64,
    pub point: Option<[f64; 2]>,
}

fn decoded(message: &Message) -> Result<Option<DeviceGaze>> {
    Ok(gaze_message(message)?)
}

/// Mean of the valid per-eye display points, as CalibrationProcessViewModel.OnGazeData forms it.
pub fn gaze_sample(g: &DeviceGaze) -> GazeSample {
    let eyes: Vec<[f64; 2]> = [(g.left_gaze_point_2d, g.left_validity), (g.right_gaze_point_2d, g.right_validity)]
        .into_iter()
        .filter(|(point, validity)| *validity < EYE_VALID_BELOW && *point != [0.0, 0.0])
        .map(|(point, _)| point)
        .collect();
    let count = eyes.len() as f64;
    let point =
        (!eyes.is_empty()).then(|| [sum(eyes.iter().map(|p| p[0])) / count, sum(eyes.iter().map(|p| p[1])) / count]);
    GazeSample { timestamp_us: g.timestamp, point }
}

/// GazeSample whose point is the mean of both eyes' display points, or `None` unless both eyes are valid.
pub fn binocular_sample(g: &DeviceGaze) -> GazeSample {
    let both = g.left_validity < EYE_VALID_BELOW && g.right_validity < EYE_VALID_BELOW;
    let [l, r] = [g.left_gaze_point_2d, g.right_gaze_point_2d];
    let point = both.then(|| [(l[0] + r[0]) / 2.0, (l[1] + r[1]) / 2.0]);
    GazeSample { timestamp_us: g.timestamp, point }
}

fn elapsed_ms(later: i64, earlier: i64) -> f64 {
    (later - earlier) as f64 / 1000.0
}

/// Whether the newest samples within `window_ms` all have both eyes valid and stay within
/// `spread` of their mean.
///
/// Points are in normalized display units of whatever calibration the device holds (none
/// after 1060), so only their stability is used, not their position.
pub fn fixation(samples: &[GazeSample], window_ms: f64, spread: f64, min_samples: usize) -> bool {
    let Some(newest) = samples.last().map(|s| s.timestamp_us) else {
        return false;
    };
    let window: Vec<&GazeSample> = samples.iter().filter(|s| elapsed_ms(newest, s.timestamp_us) <= window_ms).collect();
    let points: Option<Vec<[f64; 2]>> = window.iter().map(|s| s.point).collect();
    let Some(points) = points.filter(|points| points.len() >= min_samples) else {
        return false;
    };
    let count = points.len() as f64;
    let mean = [sum(points.iter().map(|p| p[0])) / count, sum(points.iter().map(|p| p[1])) / count];
    points.iter().all(|p| dist(p, &mean) <= spread)
}

/// Whether the user's gaze left a target while the device collected it.
///
/// Mirrors DetermineFocusedCalibrationTarget and CalibrationStateManager: gaze selects the
/// closest target of the stage within a zone radius of 0.45 times half the closest target
/// distance (in pixels); focus moves to another target only after 150 ms on it, and 250 ms
/// without valid gaze counts as the user leaving. Either sets "focus changed during
/// calibration" and the data is discarded. The proximity modifier the Tobii client applies
/// when the user sits beyond the track box is not reproduced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusCheck {
    pub screen_px: [f64; 2],
}

impl Default for FocusCheck {
    fn default() -> Self {
        Self { screen_px: [2560.0, 1440.0] }
    }
}

impl FocusCheck {
    fn pixels(&self, point: [f64; 2]) -> [f64; 2] {
        [point[0] * self.screen_px[0], point[1] * self.screen_px[1]]
    }

    /// `None` for fewer than two targets.
    pub fn zone_radius(&self, targets: &[[f64; 2]]) -> Option<f64> {
        let pixels: Vec<[f64; 2]> = targets.iter().map(|&t| self.pixels(t)).collect();
        let closest = pixels
            .iter()
            .enumerate()
            .flat_map(|(i, a)| pixels[i + 1..].iter().map(move |b| dist(a, b)))
            .reduce(crate::math::py_min)?;
        Some(closest / 2.0 * ZONE_RADIUS_FACTOR)
    }

    /// Index of the closest target within `radius` of the gaze point, the first one on ties.
    pub fn closest(&self, point: [f64; 2], targets: &[[f64; 2]], radius: f64) -> Option<usize> {
        let gaze = self.pixels(point);
        let (distance, index) = targets
            .iter()
            .enumerate()
            .map(|(i, &t)| (dist(&gaze, &self.pixels(t)), i))
            .reduce(|best, candidate| if candidate.0 < best.0 { candidate } else { best })?;
        (distance < radius).then_some(index)
    }

    /// `samples` in time order; `targets` the stage's uncalibrated targets; `focused` an index into them.
    pub fn focus_changed(&self, samples: &[GazeSample], targets: &[[f64; 2]], focused: usize) -> bool {
        let (Some(radius), Some(first)) = (self.zone_radius(targets), samples.first()) else {
            return false;
        };
        let Some(first_valid) = samples.iter().find(|s| s.point.is_some()) else {
            return true;
        };
        let mut last_valid = first.timestamp_us;
        let mut last_focus = first_valid.timestamp_us;
        for sample in samples {
            if elapsed_ms(sample.timestamp_us, last_valid) > GAZE_LOST_MS {
                return true;
            }
            let Some(point) = sample.point else {
                continue;
            };
            last_valid = sample.timestamp_us;
            match self.closest(point, targets, radius) {
                Some(index) if index == focused => last_focus = sample.timestamp_us,
                Some(_) if elapsed_ms(sample.timestamp_us, last_focus) > FOCUS_CHANGE_MS => return true,
                _ => {}
            }
        }
        elapsed_ms(samples[samples.len() - 1].timestamp_us, last_valid) > GAZE_LOST_MS
    }
}

/// Which target the gaze rests on, as DetermineFocusedCalibrationTarget decides it sample by sample.
///
/// The closest candidate within the zone radius of [`FocusCheck`] gains focus; it moves to
/// another target only once the current one has gone 150 ms without a sample on it; gaze
/// outside every zone keeps the focus; 250 ms without valid gaze drops it. A stage of one
/// target is focused by the first valid sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GazeFocus {
    pub check: FocusCheck,
    pub radius: Option<f64>,
    pub focused: Option<usize>,
    pub last_focus_us: Option<i64>,
    pub last_valid_us: Option<i64>,
}

impl GazeFocus {
    pub fn new(check: FocusCheck, stage_targets: &[[f64; 2]]) -> Self {
        Self {
            check,
            radius: check.zone_radius(stage_targets),
            focused: None,
            last_focus_us: None,
            last_valid_us: None,
        }
    }

    /// `candidates`: (index, target) still to collect, in order; returns the focused index.
    pub fn observe(&mut self, sample: &GazeSample, candidates: &[(usize, [f64; 2])]) -> Option<usize> {
        let now = sample.timestamp_us;
        let Some(point) = sample.point else {
            if self.last_valid_us.is_some_and(|valid| elapsed_ms(now, valid) > GAZE_LOST_MS) {
                self.focused = None;
            }
            return self.focused;
        };
        self.last_valid_us = Some(now);
        let index = match self.radius {
            None => candidates.first().map(|(index, _)| *index),
            Some(radius) => {
                let targets: Vec<[f64; 2]> = candidates.iter().map(|(_, target)| *target).collect();
                self.check.closest(point, &targets, radius).map(|nearest| candidates[nearest].0)
            }
        };
        if let Some(index) = index {
            if Some(index) == self.focused {
                self.last_focus_us = Some(now);
            } else if self.focused.is_none()
                || self.last_focus_us.is_some_and(|last| elapsed_ms(now, last) > FOCUS_CHANGE_MS)
            {
                self.focused = Some(index);
                self.last_focus_us = Some(now);
            }
        }
        self.focused
    }
}

/// CalibrationAssessor: whether the user stopped calibrating, which restarts the calibration.
///
/// From the second stage on, every valid gaze sample is checked: after a gap of more than
/// 500 ms (the user was away) both timers start again; otherwise the calibration is stalled
/// when for 4 s neither the gaze moved to a waiting target nor a target was collected, or
/// when for 10 s no target was collected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttentionWatch {
    pub stall_ms: f64,
    pub no_pop_ms: f64,
    pub last_sample_us: Option<i64>,
    pub last_attention_us: Option<i64>,
    pub last_pop_us: Option<i64>,
}

impl Default for AttentionWatch {
    fn default() -> Self {
        Self::new(ATTENTION_STALL_MS, ATTENTION_NO_POP_MS)
    }
}

impl AttentionWatch {
    /// A watch with other limits than the client's 4 s and 10 s.
    pub fn new(stall_ms: f64, no_pop_ms: f64) -> Self {
        Self { stall_ms, no_pop_ms, last_sample_us: None, last_attention_us: None, last_pop_us: None }
    }

    pub fn observe(&mut self, sample: &GazeSample, looking_at_waiting_target: bool) {
        if sample.point.is_none() {
            return;
        }
        let now = sample.timestamp_us;
        match self.last_sample_us {
            Some(last) if elapsed_ms(now, last) <= USER_AWAY_MS => {
                if looking_at_waiting_target {
                    self.last_attention_us = Some(now);
                }
            }
            _ => {
                self.last_attention_us = Some(now);
                self.last_pop_us = Some(now);
            }
        }
        self.last_sample_us = Some(now);
    }

    pub fn popped(&mut self) {
        if let Some(last) = self.last_sample_us {
            self.last_pop_us = Some(last);
            self.last_attention_us = Some(last);
        }
    }

    pub fn stalled(&self) -> bool {
        let (Some(last), Some(pop), Some(attention)) = (self.last_sample_us, self.last_pop_us, self.last_attention_us)
        else {
            return false;
        };
        let since_pop = elapsed_ms(last, pop);
        let since_attention = elapsed_ms(last, attention);
        since_pop > self.no_pop_ms || (since_pop > self.stall_ms && since_attention > self.stall_ms)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationOutcome {
    pub calibration: LocalCalibration,
    pub stimulus_points: Vec<StimulusPoint>,
    pub points: Vec<CalibrationPointData>,
    /// (target, attempt) of every collection whose data was discarded.
    pub discarded: Vec<(usize, usize)>,
}

/// A refused compute: its stage, the attempt and the device's view of the targets (1120).
#[derive(Debug, Clone, PartialEq)]
pub struct ComputeFailure {
    pub stage: usize,
    pub attempt: usize,
    pub stimulus_points: Vec<StimulusPoint>,
}

/// What a calibration has done so far, readable while and after it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// The step that runs, for error reports.
    pub step: String,
    pub discarded: Vec<(usize, usize)>,
    pub compute_failures: Vec<ComputeFailure>,
    pub stalled_runs: usize,
}

/// The Tobii Experience calibration (CalibrationProcessViewModel, 7-point flow) on a device session.
///
/// Device commands, in order: subscribe gaze, 1010 start, 1060 clear, optionally 1110 with
/// the previous calibration (the client's "improve calibration"), then per stage 1030 for
/// each target (1080 for a target whose collection the user looked away from, then 1030
/// again) and 1070 once the stage is complete, then 1020 stop and unsubscribe. Afterwards it
/// reads the new calibration (1100), its id (1490 key 7) and the per-target quality (1120).
/// Any failure after 1010 sends 1020 and fails with [`OperationError::Failed`]; nothing is
/// applied on failure.
///
/// A compute the device refuses (PROTOCOL_OPERATION_FAILED, the Tobii app's "all points
/// failed") stops the calibration (1020) and, up to `restarts` times, starts it again from
/// the first target (1010, 1060), as the app's "try again" does. With `wait_for_fixation`,
/// each collect waits until both eyes are valid and the gaze has been still for 300 ms (at
/// most 4 s); this is a Linux addition, the app relies on its 200 ms saccade delay alone.
///
/// With `choose_by_gaze` (and a `focus` check) the targets are shown and taken as the Tobii
/// client does instead: every target of a stage is shown at once, the user picks the order
/// by looking, and a target is collected 200 ms after the gaze settled on it
/// ([`GazeFocus`]); data collected while the gaze moved away is discarded and the target
/// stays. From the second stage on an [`AttentionWatch`] restarts the whole calibration when
/// nothing happens, at most `max_stalled_runs` times (the app has no limit).
///
/// The device stops sending, and stops answering requests for the rest of the USB session,
/// once a subscribed stream has gone unread for about two seconds; drivers keep reading
/// while UI events are shown (docs/gaze-calibration.md).
#[derive(Debug, Clone, PartialEq)]
pub struct Calibrator {
    pub targets: Vec<[f64; 2]>,
    pub stages: Vec<Vec<usize>>,
    pub previous: Option<LocalCalibration>,
    pub focus: Option<FocusCheck>,
    pub max_attempts: usize,
    pub subscribe_gaze: bool,
    pub restarts: usize,
    pub wait_for_fixation: bool,
    pub choose_by_gaze: bool,
    pub max_stalled_runs: usize,
    /// The attention watch's limits, [`ATTENTION_STALL_MS`] and [`ATTENTION_NO_POP_MS`].
    pub attention_stall_ms: f64,
    pub attention_no_pop_ms: f64,
}

impl Default for Calibrator {
    fn default() -> Self {
        Self {
            targets: INTERACTIVE_POINTS.to_vec(),
            stages: STAGES.iter().map(|stage| stage.to_vec()).collect(),
            previous: None,
            focus: None,
            max_attempts: 3,
            subscribe_gaze: true,
            restarts: 2,
            wait_for_fixation: false,
            choose_by_gaze: false,
            max_stalled_runs: MAX_STALLED_RUNS,
            attention_stall_ms: ATTENTION_STALL_MS,
            attention_no_pop_ms: ATTENTION_NO_POP_MS,
        }
    }
}

enum StageError<E> {
    ComputeRefused { stage: usize, error: E },
    TargetsIgnored,
    Failed(OperationError<E>),
}

impl<E> From<OperationError<E>> for StageError<E> {
    fn from(error: OperationError<E>) -> Self {
        StageError::Failed(error)
    }
}

fn external<E>(error: E) -> OperationError<E> {
    OperationError::External(error)
}

fn int(value: usize) -> LogValue {
    LogValue::Int(value as i64)
}

/// A calibration in progress: the procedure of [`Calibrator::run`] with its state.
struct Run<E> {
    io: Io<E>,
    config: Calibrator,
    progress: Arc<Mutex<Progress>>,
}

impl<E: External> Run<E> {
    fn update<R>(&self, change: impl FnOnce(&mut Progress) -> R) -> R {
        change(&mut self.progress.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn step(&self) -> String {
        self.update(|p| p.step.clone())
    }

    fn set_step(&self, step: String) {
        self.update(|p| p.step = step);
    }

    async fn call(&self, step: &str, message: Message, timeout_s: f64) -> Result<Vec<Value>, E> {
        self.io.log("command", vec![("step", LogValue::Text(step.to_string()))]).await;
        self.io.call(message, Some(timeout_s)).await
    }

    /// A cleanup call: a failure is only logged under `failure_kind`.
    async fn cleanup(&self, step: &str, message: Message, failure_kind: &'static str) {
        self.io.log("command", vec![("step", LogValue::Text(step.to_string()))]).await;
        let call = crate::operation::Call { message, timeout_s: Some(COMMAND_TIMEOUT_S), tolerant: true };
        if let Err(error) = self.io.send(call).await {
            self.io.log(failure_kind, vec![("error", LogValue::Text(error.message()))]).await;
        }
    }

    async fn unsubscribe(&self) {
        if self.config.subscribe_gaze {
            let request = unsubscribe_stream(0, Stream::Gaze.index()).expect("gaze stream");
            self.cleanup("unsubscribe gaze", request, "unsubscribe_failed").await;
        }
    }

    async fn stop_after_failure(&self) {
        let stop = gaze_request(gc::CALIBRATION_STOP);
        self.cleanup("calibration stop after failure (1020)", stop, "stop_failed").await;
        self.unsubscribe().await;
    }

    async fn run(self) -> OperationResult<CalibrationOutcome, E> {
        if self.config.subscribe_gaze {
            let request = subscribe_stream(0, Stream::Gaze.index(), &[])?;
            self.call("subscribe gaze", request, COMMAND_TIMEOUT_S).await.map_err(external)?;
        }
        let start = gaze_request(gc::CALIBRATION_START);
        if let Err(error) = self.call("calibration start (1010)", start, COMMAND_TIMEOUT_S).await {
            if error.is_interrupt() {
                return Err(external(error));
            }
            self.unsubscribe().await;
            let cause = Box::new(external(error));
            return Err(OperationError::Failed { step: "calibration start (1010)".into(), cause });
        }
        let mut step = String::from("calibration clear (1060)");
        if let Err(error) = self.attempts(&mut step).await {
            self.stop_after_failure().await;
            let interrupted = matches!(&error, OperationError::External(e) if e.is_interrupt());
            if interrupted {
                return Err(error);
            }
            let step = if step == STAGES_STEP { self.step() } else { step };
            return Err(OperationError::Failed { step, cause: Box::new(error) });
        }
        self.unsubscribe().await;
        self.outcome().await
    }

    async fn attempts(&self, step: &mut String) -> OperationResult<(), E> {
        let mut attempt = 0;
        loop {
            if attempt > 0 {
                *step = "restart: calibration start (1010)".into();
                self.call(step, gaze_request(gc::CALIBRATION_START), COMMAND_TIMEOUT_S).await.map_err(external)?;
            }
            *step = "calibration clear (1060)".into();
            self.call(step, gaze_request(gc::CALIBRATION_CLEAR), COMMAND_TIMEOUT_S).await.map_err(external)?;
            if let Some(previous) = &self.config.previous {
                *step = "apply previous calibration (1110)".into();
                let request = device::calibration_apply(0, &previous.blob)?;
                self.call(step, request, TRANSFER_TIMEOUT_S).await.map_err(external)?;
            }
            self.io.ui(UiEvent::CalibrationStarted { attempt }).await.map_err(external)?;
            *step = STAGES_STEP.into();
            match self.stages().await {
                Ok(()) => break,
                Err(StageError::Failed(error)) => return Err(error),
                Err(StageError::ComputeRefused { stage, error }) => {
                    if attempt >= self.config.restarts + self.update(|p| p.stalled_runs) {
                        *step = self.step();
                        return Err(external(error));
                    }
                    self.record_compute_failure(stage, attempt).await?;
                }
                Err(StageError::TargetsIgnored) => {
                    let runs = self.update(|p| {
                        p.stalled_runs += 1;
                        p.stalled_runs
                    });
                    self.io.log("targets_ignored", vec![("attempt", int(attempt)), ("runs", int(runs))]).await;
                    if runs > self.config.max_stalled_runs {
                        *step = self.step();
                        return Err(OperationError::calibration("the targets were not looked at"));
                    }
                }
            }
            *step = "restart: calibration stop (1020)".into();
            self.call(step, gaze_request(gc::CALIBRATION_STOP), COMMAND_TIMEOUT_S).await.map_err(external)?;
            attempt += 1;
            self.io.ui(UiEvent::Restarting { attempt }).await.map_err(external)?;
        }
        *step = "calibration stop (1020)".into();
        self.call(step, gaze_request(gc::CALIBRATION_STOP), COMMAND_TIMEOUT_S).await.map_err(external)?;
        Ok(())
    }

    /// Every stage once: collect its targets, then 1070.
    async fn stages(&self) -> Result<(), StageError<E>> {
        let mut watch = AttentionWatch::new(self.config.attention_stall_ms, self.config.attention_no_pop_ms);
        let choose_by_gaze = self.config.choose_by_gaze && self.config.focus.is_some();
        for (number, stage) in self.config.stages.iter().enumerate() {
            self.set_step(format!("stage {number} targets"));
            if choose_by_gaze {
                self.collect_by_gaze(number, stage, (number > 0).then_some(&mut watch)).await?;
            } else {
                for &index in stage {
                    self.set_step(format!("target {index}"));
                    self.collect(number, stage, index).await?;
                }
            }
            let step = format!("compute stage {number} (1070)");
            self.set_step(step.clone());
            self.io.ui(UiEvent::Computing { stage: number }).await.map_err(external)?;
            let compute = gaze_request(gc::CALIBRATION_COMPUTE_AND_APPLY);
            if let Err(error) = self.call(&step, compute, COMPUTE_TIMEOUT_S).await {
                if error.is_interrupt() || !error.message().contains(OPERATION_FAILED) {
                    return Err(external(error).into());
                }
                return Err(StageError::ComputeRefused { stage: number, error });
            }
            self.io.ui(UiEvent::StageDone { stage: number }).await.map_err(external)?;
        }
        Ok(())
    }

    fn gaze_messages(messages: &[Message]) -> impl Iterator<Item = &Message> {
        let gaze_id = Stream::Gaze.wire_id();
        messages.iter().filter(move |m| m.kind == STREAM_DATA_KIND && m.command == gaze_id)
    }

    /// Samples of the gaze messages among `messages`, in arrival order.
    fn decode_samples(messages: &[Message], sample: fn(&DeviceGaze) -> GazeSample) -> Result<Vec<GazeSample>> {
        let mut samples = Vec::new();
        for message in Self::gaze_messages(messages) {
            if let Some(g) = decoded(message)? {
                samples.push(sample(&g));
            }
        }
        Ok(samples)
    }

    /// Mean-gaze samples of the gaze messages among `messages`, in time order.
    fn samples(messages: &[Message], sample: fn(&DeviceGaze) -> GazeSample) -> Result<Vec<GazeSample>> {
        let mut samples = Self::decode_samples(messages, sample)?;
        samples.sort_by_key(|s| s.timestamp_us);
        Ok(samples)
    }

    async fn take_gaze_samples(&self) -> Result<Vec<GazeSample>, StageError<E>> {
        let messages = self.io.pending_streams(true).await;
        Ok(Self::samples(&messages, gaze_sample).map_err(OperationError::from)?)
    }

    /// Shows the stage's targets and collects each one the user looks at, in the order they choose.
    async fn collect_by_gaze(
        &self,
        number: usize,
        stage: &[usize],
        mut watch: Option<&mut AttentionWatch>,
    ) -> Result<(), StageError<E>> {
        let focus_check = self.config.focus.expect("choose_by_gaze needs a focus check");
        let mut waiting: Vec<(usize, [f64; 2])> = stage.iter().map(|&i| (i, self.config.targets[i])).collect();
        let shown = waiting.iter().map(|&(i, [x, y])| (i, x, y)).collect();
        self.io.ui(UiEvent::ShowTargets { stage: number, targets: shown }).await.map_err(external)?;
        let targets: Vec<[f64; 2]> = waiting.iter().map(|(_, target)| *target).collect();
        let mut focus = GazeFocus::new(focus_check, &targets);
        self.io.clear_streams().await;
        while !waiting.is_empty() {
            self.io.check().await.map_err(external)?;
            self.io.read().await.map_err(external)?;
            for sample in self.take_gaze_samples().await? {
                let focused = focus.observe(&sample, &waiting);
                if let Some(watch) = watch.as_deref_mut() {
                    watch.observe(&sample, focused.is_some());
                }
            }
            if watch.as_deref().is_some_and(AttentionWatch::stalled) {
                return Err(StageError::TargetsIgnored);
            }
            let Some(index) = focus.focused else {
                continue;
            };
            if self.collect_focused(number, index, &waiting, &focus_check).await? {
                waiting.retain(|(i, _)| *i != index);
                if let Some(watch) = watch.as_deref_mut() {
                    watch.popped();
                }
            }
            focus.focused = None;
        }
        Ok(())
    }

    /// Waits for the saccade, collects (1030) and keeps the data only if the gaze stayed; true when kept.
    async fn collect_focused(
        &self,
        number: usize,
        index: usize,
        waiting: &[(usize, [f64; 2])],
        focus_check: &FocusCheck,
    ) -> Result<bool, StageError<E>> {
        let [x, y] = self.config.targets[index];
        self.io.ui(UiEvent::Focus { index, focused: true }).await.map_err(external)?;
        let until = self.io.now() + SACCADE_S;
        while self.io.now() < until {
            self.io.read().await.map_err(external)?;
        }
        self.io.clear_streams().await;
        let started = self.io.now();
        let collect = device::calibration_collect_2d(0, x, y, Eye::Both).map_err(OperationError::from)?;
        self.call(&format!("collect target {index} (1030)"), collect, COLLECT_TIMEOUT_S).await.map_err(external)?;
        let seconds = round_digits(self.io.now() - started, 3);
        let messages = self.io.pending_streams(true).await;
        self.log_collected(index, seconds, messages.len()).await;
        let samples = Self::samples(&messages, gaze_sample).map_err(OperationError::from)?;
        let targets: Vec<[f64; 2]> = waiting.iter().map(|(_, target)| *target).collect();
        let position = waiting.iter().position(|(i, _)| *i == index).expect("focused target is waiting");
        let changed = number > 0 && focus_check.focus_changed(&samples, &targets, position);
        self.log_focus(index, samples.len(), changed).await;
        if !changed {
            self.io.ui(UiEvent::PointDone { index, accepted: true }).await.map_err(external)?;
            return Ok(true);
        }
        self.update(|p| p.discarded.push((index, p.discarded.len() + 1)));
        let discard = device::calibration_discard_2d(0, x, y, Eye::Both).map_err(OperationError::from)?;
        self.call(&format!("discard target {index} (1080)"), discard, COMMAND_TIMEOUT_S).await.map_err(external)?;
        self.io.ui(UiEvent::PointDone { index, accepted: false }).await.map_err(external)?;
        Ok(false)
    }

    async fn log_collected(&self, index: usize, seconds: f64, stream_messages: usize) {
        let values = vec![
            ("target", int(index)),
            ("seconds", LogValue::Float(seconds)),
            ("stream_messages", int(stream_messages)),
        ];
        self.io.log("collected", values).await;
    }

    async fn log_focus(&self, index: usize, samples: usize, changed: bool) {
        let values = vec![("target", int(index)), ("samples", int(samples)), ("changed", LogValue::Bool(changed))];
        self.io.log("focus", values).await;
    }

    /// Logs the device's view of the collected targets (1120, read only) before restarting.
    async fn record_compute_failure(&self, stage: usize, attempt: usize) -> OperationResult<(), E> {
        let points = match read_stimulus_points(&self.io).await {
            Ok(points) => points,
            Err(OperationError::External(error)) if error.is_interrupt() => return Err(external(error)),
            Err(error) => {
                self.io.log("stimulus_points_failed", vec![("error", LogValue::Text(error.to_string()))]).await;
                Vec::new()
            }
        };
        self.update(|p| p.compute_failures.push(ComputeFailure { stage, attempt, stimulus_points: points.clone() }));
        let values = vec![
            ("stage", int(stage)),
            ("attempt", int(attempt)),
            ("stimulus_points", LogValue::StimulusPoints(points)),
        ];
        self.io.log("compute_failed", values).await;
        Ok(())
    }

    /// Reads until both eyes are valid and the gaze has been still, or FIXATION_TIMEOUT_S passes.
    async fn await_fixation(&self, index: usize) -> Result<bool, StageError<E>> {
        self.io.clear_streams().await;
        let mut samples = Vec::new();
        let deadline = self.io.now() + FIXATION_TIMEOUT_S;
        while self.io.now() < deadline {
            self.io.read().await.map_err(external)?;
            let messages = self.io.pending_streams(true).await;
            samples.extend(Self::decode_samples(&messages, binocular_sample).map_err(OperationError::from)?);
            if fixation(&samples, FIXATION_WINDOW_MS, FIXATION_SPREAD, FIXATION_MIN_SAMPLES) {
                self.log_fixation(index, samples.len(), true).await;
                return Ok(true);
            }
        }
        self.log_fixation(index, samples.len(), false).await;
        Ok(false)
    }

    async fn log_fixation(&self, index: usize, samples: usize, stable: bool) {
        let values = vec![("target", int(index)), ("samples", int(samples)), ("stable", LogValue::Bool(stable))];
        self.io.log("fixation", values).await;
    }

    async fn collect(&self, stage_number: usize, stage: &[usize], index: usize) -> Result<(), StageError<E>> {
        let [x, y] = self.config.targets[index];
        for attempt in 1..=self.config.max_attempts {
            self.io.ui(UiEvent::ShowPoint { index, x, y }).await.map_err(external)?;
            if self.config.wait_for_fixation {
                self.await_fixation(index).await?;
            }
            self.io.clear_streams().await;
            let started = self.io.now();
            let collect = device::calibration_collect_2d(0, x, y, Eye::Both).map_err(OperationError::from)?;
            self.call(&format!("collect target {index} (1030)"), collect, COLLECT_TIMEOUT_S).await.map_err(external)?;
            let seconds = round_digits(self.io.now() - started, 3);
            let messages = self.io.pending_streams(false).await;
            self.log_collected(index, seconds, messages.len()).await;
            if !self.focus_changed(stage_number, stage, index, &messages).await? {
                self.io.ui(UiEvent::PointDone { index, accepted: true }).await.map_err(external)?;
                return Ok(());
            }
            self.update(|p| p.discarded.push((index, attempt)));
            let discard = device::calibration_discard_2d(0, x, y, Eye::Both).map_err(OperationError::from)?;
            self.call(&format!("discard target {index} (1080)"), discard, COMMAND_TIMEOUT_S).await.map_err(external)?;
            self.io.ui(UiEvent::PointDone { index, accepted: false }).await.map_err(external)?;
        }
        let max_attempts = self.config.max_attempts;
        Err(OperationError::calibration(format!("target {index} lost focus {max_attempts} times")).into())
    }

    async fn focus_changed(
        &self,
        stage_number: usize,
        stage: &[usize],
        index: usize,
        messages: &[Message],
    ) -> Result<bool, StageError<E>> {
        let Some(focus) = self.config.focus.filter(|_| stage_number > 0) else {
            return Ok(false);
        };
        let samples = Self::samples(messages, gaze_sample).map_err(OperationError::from)?;
        let targets: Vec<[f64; 2]> = stage.iter().map(|&i| self.config.targets[i]).collect();
        let position = stage.iter().position(|&i| i == index).expect("target of the stage");
        let changed = focus.focus_changed(&samples, &targets, position);
        self.log_focus(index, samples.len(), changed).await;
        Ok(changed)
    }

    async fn outcome(&self) -> OperationResult<CalibrationOutcome, E> {
        let blob = retrieve_blob(&self.io).await?;
        let calibration_id = read_device_calibration_id(&self.io).await?;
        let stimulus_points = read_stimulus_points(&self.io).await?;
        let points = if blob.is_empty() { Vec::new() } else { device::calibration_points(&blob)? };
        let discarded = self.update(|p| p.discarded.clone());
        Ok(CalibrationOutcome {
            calibration: LocalCalibration { calibration_id, blob },
            stimulus_points,
            points,
            discarded,
        })
    }
}

impl Calibrator {
    /// The calibration as an operation, and the progress it reports.
    pub fn operation<E: External>(self) -> (crate::operation::Operation<CalibrationOutcome, E>, Arc<Mutex<Progress>>) {
        let progress = Arc::new(Mutex::new(Progress {
            step: STAGES_STEP.into(),
            discarded: Vec::new(),
            compute_failures: Vec::new(),
            stalled_runs: 0,
        }));
        let shared = Arc::clone(&progress);
        let operation = crate::operation::Operation::new(move |io| Run { io, config: self, progress: shared }.run());
        (operation, progress)
    }
}

#[cfg(test)]
mod tests;
