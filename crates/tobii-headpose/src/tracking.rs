//! The VisionSDK::VNNTrackerCore frame loop (0x140049450) for grayscale frames, followed by
//! the per-object step 0x14004a530.
//!
//! Core states: searching (full-frame detection), recovering (re-detection inside the
//! working image), tracking (box from the previous landmarks). The core finds the face, runs
//! D1 in the working view and applies the confidence gate. Its own fit is disabled in the
//! shipped configuration: 0x140049e60 clears the core flag +0x15 because jk_300_rigid.fdp
//! maps no FDP group 10 point, which also skips the LBF eye refinement. The object step then
//! smooths the frame-normalised records ([`smooth_records`]), copies them, flips Y back and
//! fits them with 0x14007e440 at full-frame dimensions (0x14004a2c0).
//!
//! The geometry is float32 in the original's order; integer conversions keep the Python
//! `round` and `math.floor` semantics the reconstruction was checked with.

use std::path::Path;

use tobii_inference::d1::Point;
use tobii_inference::d1_quality::{d1_quality_score, passes_d1_quality};
use tobii_inference::face_detect::{DetectOptions, Detection, FaceFinder, resize_bilinear};
use tobii_inference::numeric::{py_max, py_min, py_round};
use tobii_inference::{D1LandmarkModel, Runtime};
use tobii_models::Shapes;

use crate::error::{Result, invalid};
use crate::model::{FdpId, load_pose_model, numeric_config};
use crate::smoothing::{
    LandmarkFilter, LandmarkRecord, Records, TRACKER_TABLE, TableEntry, flip_y, smooth_records, table_entries,
};
use crate::temporal::{Rectangle, TemporalFit, TemporalPoseFitter};

pub const EXPANSION: f32 = 2.5;
pub const TARGET_FACE_WIDTH: i64 = 96;
pub const REDETECT_SENSITIVITY: f64 = 0.949999988079071;
pub const REDETECT_MIN_OVERLAP: f32 = 0.10000000149011612;
pub const RADIANS_TO_DEGREES: f32 = 57.2957763671875;
/// Brow and nostril records whose midpoints give the roll: `(4.1, 4.2)` and `(9.4, 9.5)`.
pub const ROLL_POINTS: [FdpId; 4] = [(4, 1), (4, 2), (9, 4), (9, 5)];
/// Records 0x140055a20 fills from others after the core: `(alias, source)`.
pub const ALIASED_RECORDS: [(FdpId, FdpId); 4] =
    [((2, 1), (15, 17)), ((13, 17), (15, 17)), ((14, 21), (9, 3)), ((9, 12), (14, 23))];
/// The face no longer tracked: the core's reset box.
pub const NO_FACE: Rectangle = [-1; 4];
pub const OUTSIDE_MESSAGE: &str = "Face region lies outside the frame";

/// Python's `round(float)` of a float32 value.
fn round(value: f32) -> Result<i64> {
    Ok(py_round(f64::from(value))?)
}

/// Python's `int(math.floor(float))` of a float32 value.
fn floor(value: f32) -> Result<i64> {
    Ok(py_round(f64::from(value).floor())?)
}

/// 0x140079ce0 expansion around the box centre, clamped by 0x14007a740.
pub fn expanded_region(face: Rectangle, frame_width: i64, frame_height: i64, expansion: f32) -> Result<Rectangle> {
    let [x, y, width, height] = face;
    let (w, h) = (width as f32, height as f32);
    let grown_w = floor(w * expansion)?;
    let grown_h = floor(h * expansion)?;
    let left = floor((w * 0.5 + x as f32) - grown_w as f32 * 0.5)?;
    let top = floor((h * 0.5 + y as f32) - grown_h as f32 * 0.5)?;
    let (clamped_x, clamped_y) = (left.max(0), top.max(0));
    Ok([
        clamped_x,
        clamped_y,
        (left + grown_w).min(frame_width) - clamped_x,
        (top + grown_h).min(frame_height) - clamped_y,
    ])
}

/// The expanded, clamped face region resized so the face is [`TARGET_FACE_WIDTH`] wide.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingView {
    pub image: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub region: Rectangle,
    pub ratio: f32,
    pub face: Rectangle,
}

fn size(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

/// The crop and resizing adjustments of 0x140079ce0 applied to one `columns`×`rows` frame.
pub fn working_view(
    pixels: &[u8],
    columns: usize,
    rows: usize,
    face: Rectangle,
    expansion: f32,
    target: i64,
) -> Result<WorkingView> {
    if pixels.len() != columns * rows {
        return invalid("Expected one byte per frame pixel");
    }
    let region = expanded_region(face, columns as i64, rows as i64, expansion)?;
    let [rx, ry, rw, rh] = region;
    if rw <= 0 || rh <= 0 {
        return invalid(OUTSIDE_MESSAGE);
    }
    let ratio = target as f32 / face[2] as f32;
    let shifted = [face[0] - rx, face[1] - ry, face[2], face[3]];
    let mut scaled = [0; 4];
    for (scaled, value) in scaled.iter_mut().zip(shifted) {
        *scaled = round(value as f32 * ratio)?;
    }
    let (width, height) = (round(rw as f32 * ratio)?, round(rh as f32 * ratio)?);
    let (left, top, crop_width, crop_height) = (rx as usize, ry as usize, rw as usize, rh as usize);
    let crop: Vec<u8> = (top..top + crop_height)
        .flat_map(|row| &pixels[row * columns + left..row * columns + left + crop_width])
        .copied()
        .collect();
    let image = if face[2] < target * 8 {
        resize_bilinear(&crop, crop_width, crop_height, size(width), size(height))?
    } else {
        let doubled = resize_bilinear(&crop, crop_width, crop_height, size(width * 2), size(height * 2))?;
        resize_bilinear(&doubled, size(width * 2), size(height * 2), size(width), size(height))?
    };
    Ok(WorkingView { image, width: size(width), height: size(height), region, ratio, face: scaled })
}

/// The inverse adjustments 0x140079c00 then 0x1400799c0 for a working-image face box.
pub fn working_face_in_frame(face: Rectangle, region: Rectangle, ratio: f32) -> Result<Rectangle> {
    let inverse = 1.0 / ratio;
    let mut mapped = [0; 4];
    for (mapped, value) in mapped.iter_mut().zip(face) {
        *mapped = round(value as f32 * inverse)?;
    }
    let [x, y, width, height] = mapped;
    Ok([x + region[0], y + region[1], width, height])
}

/// 0x14007aa30 mapping of working-normalised records into the full frame.
pub fn to_frame_points(points: &[Point], region: Rectangle, frame_width: i64, frame_height: i64) -> Vec<Point> {
    let [rx, ry, rw, rh] = region.map(|v| v as f32);
    let (width, height) = (frame_width as f32, frame_height as f32);
    points.iter().map(|&[x, y]| [(rw * x - (0.0 - rx)) / width, (rh * y - (0.0 - ry)) / height]).collect()
}

/// 0x14007a4b0: the square box around all landmarks for the next frame.
pub fn landmark_box(frame_points: &[Point], frame_width: i64, frame_height: i64) -> Result<Rectangle> {
    let (mut low_x, mut low_y) = (f32::MAX, f32::MAX);
    let (mut high_x, mut high_y) = (0.0f32, 0.0f32);
    for &[x, y] in frame_points {
        let (px, py) = (frame_width as f32 * x, frame_height as f32 * y);
        (low_x, high_x) = (py_min(low_x, px), py_max(high_x, px));
        (low_y, high_y) = (py_min(low_y, py), py_max(high_y, py));
    }
    let side = ((high_y - low_y) + (high_x - low_x)) * 0.5;
    let side_pixels = round(side)?;
    Ok([
        round(((high_x - low_x) - side) * 0.5 + low_x)?,
        round(((high_y - low_y) - side) * 0.5 + low_y)?,
        side_pixels,
        side_pixels,
    ])
}

/// The roll in degrees 0x14007dc50 stores for the next crop, from the brow and nostril
/// midpoints of [`ROLL_POINTS`] in working-normalised coordinates.
pub fn roll_degrees(roll_points: [Point; 4], width: i64, height: i64) -> f32 {
    let [brow_a, brow_b, nose_a, nose_b] = roll_points;
    let (w, h) = (width as f32, height as f32);
    let dy = (nose_b[1] + nose_a[1]) * h * 0.5 - (brow_b[1] + brow_a[1]) * h * 0.5;
    let dx = (nose_b[0] + nose_a[0]) * w * 0.5 - (brow_b[0] + brow_a[0]) * w * 0.5;
    (f64::from(dy).atan2(f64::from(dx)) as f32) * RADIANS_TO_DEGREES - 90.0
}

/// [`roll_degrees`] of the landmarks whose identifiers are `landmark_ids`; a later
/// duplicate identifier replaces an earlier one.
pub fn temporal_roll(points: &[Point], landmark_ids: &[FdpId], width: i64, height: i64) -> Result<f32> {
    let mut roll_points = [[0.0; 2]; 4];
    for (point, id) in roll_points.iter_mut().zip(ROLL_POINTS) {
        let Some(index) = landmark_ids.iter().take(points.len()).rposition(|&known| known == id) else {
            return invalid(format!("Missing roll landmark {id:?}"));
        };
        *point = points[index];
    }
    Ok(roll_degrees(roll_points, width, height))
}

/// The inclusive-pixel overlap ratio 0x1400499d0 uses, of box corners `[x1, y1, x2, y2]`
/// and a face rectangle.
pub fn overlap(corners: [f32; 4], face: Rectangle) -> f32 {
    let [box_x1, box_y1, box_x2, box_y2] = corners;
    let (x1, y1) = (face[0] as f32, face[1] as f32);
    let (x2, y2) = ((face[2] + face[0]) as f32, (face[3] + face[1]) as f32);
    let wide = py_max((py_min(box_x2, x2) - py_max(box_x1, x1)) + 1.0, 0.0);
    let tall = py_max((py_min(box_y2, y2) - py_max(box_y1, y1)) + 1.0, 0.0);
    let shared = tall * wide;
    let area = ((box_y2 - box_y1) + 1.0) * ((box_x2 - box_x1) + 1.0);
    shared / ((area + ((y2 - y1) + 1.0) * ((x2 - x1) + 1.0)) - shared)
}

/// 0x1400499d0 after detection: the rectangle of the first best-overlapping detection,
/// unless its overlap is not above [`REDETECT_MIN_OVERLAP`].
pub fn best_redetection(found: &[Detection], face: Rectangle) -> Option<Rectangle> {
    let mut best = None;
    let mut best_overlap = 0.0f32;
    for detection in found {
        let ratio = overlap([detection.x1, detection.y1, detection.x2, detection.y2], face);
        if ratio > best_overlap {
            (best, best_overlap) = (Some(detection), ratio);
        }
    }
    best.filter(|_| best_overlap > REDETECT_MIN_OVERLAP).map(Detection::rectangle)
}

/// The detector settings of the re-detection inside the working image.
pub const REDETECT_OPTIONS: DetectOptions =
    DetectOptions { sensitivity: REDETECT_SENSITIVITY, min_face: 0.0, max_face: 1.0, region: None };

/// The core records after 0x140049450: frame-normalised D1 points plus the 0x140055a20
/// aliases.
pub fn frame_records(frame_points: &[Point], quality: &[f32], landmark_ids: &[FdpId]) -> Result<Records> {
    let mut records: Records = landmark_ids
        .iter()
        .zip(frame_points)
        .zip(quality)
        .map(|((&id, &[x, y]), &quality)| (id, LandmarkRecord { x, y, present: true, quality }))
        .collect();
    for (alias, source) in ALIASED_RECORDS {
        let Some(&original) = records.get(&source) else { return invalid(format!("Missing record {source:?}")) };
        records.insert(alias, LandmarkRecord { present: true, ..original });
    }
    Ok(records)
}

/// The core's search state; the discriminant is the caller quality mode of the D1 gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreState {
    Searching = 0,
    Recovering = 1,
    Tracking = 2,
}

/// The per-object tracking state of 0x14004a530.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectState {
    Tracking = 1,
    Lost = 2,
    Restarted = 3,
}

/// One frame through the core and the object step.
///
/// `state` and `face` are what the frame started from; after a failed core they are
/// searching and `None`. Working points are normalised to the working image, frame points
/// to the frame. `fit` and `fitted_points` (the smoothed records the fit used) are present
/// only for a valid fit.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackedFrame {
    pub state: CoreState,
    pub face: Option<Rectangle>,
    pub working_size: Option<[usize; 2]>,
    pub working_points: Option<Vec<Point>>,
    pub frame_points: Option<Vec<Point>>,
    pub quality_score: f32,
    pub accepted: bool,
    pub fit: Option<TemporalFit>,
    pub roll_degrees: f32,
    pub fitted_points: Option<Vec<Point>>,
}

/// What 0x140049450 hands back: the frame and, when it succeeded, the frame-normalised
/// records.
struct CoreFrame {
    tracked: TrackedFrame,
    records: Option<Records>,
}

pub(crate) const INIT_SHAPE: &str = "support/bdtsdata/NN/vnn_nir/init_shape.bin";
const TRACKER_CONFIG: &str = "support/Head Tracker.cfg";

/// The VNNTrackerCore frame loop followed by the per-object step 0x14004a530.
#[derive(Debug)]
pub struct FaceTracker {
    landmarks: D1LandmarkModel,
    finder: FaceFinder,
    quality_ids: Vec<(i64, i64)>,
    pub fitter: TemporalPoseFitter,
    pub smoothing_entries: Vec<TableEntry>,
    pub recovery_timeout_ms: i64,
    pub landmark_filter: LandmarkFilter,
    /// Frame region `(x, y, width, height)` the searched face's centre must lie in.
    pub region: Option<[f64; 4]>,
    pub roll: f32,
    pub state: CoreState,
    pub face: Rectangle,
    pub object_state: ObjectState,
    pub smoothing_epoch: i64,
    pub lost_since: i64,
    pub smoothed: Option<Records>,
}

impl FaceTracker {
    /// The D1, face finder and fitting models of `model_directory` (models/tobii-et5).
    /// `smoothing_factors` replaces the `Head Tracker.cfg` factors.
    pub fn new(
        runtime: &Runtime,
        model_directory: &Path,
        region: Option<[f64; 4]>,
        smoothing_factors: Option<&[f32]>,
    ) -> Result<Self> {
        let reference = Shapes::read(&model_directory.join(INIT_SHAPE), 2)?;
        let landmarks =
            D1LandmarkModel::new(runtime, &model_directory.join("d1"), reference.values.as_chunks::<2>().0)?;
        let model = load_pose_model(&model_directory.join("support"))?;
        let finder = FaceFinder::new(runtime, &model_directory.join("ff"))?;
        let quality_ids = model.landmark_ids.iter().map(|&(g, i)| (i64::from(g), i64::from(i))).collect();
        let fitter = TemporalPoseFitter::new(model)?;
        let config = model_directory.join(TRACKER_CONFIG);
        let configured: Vec<f32>;
        let factors = match smoothing_factors {
            Some(factors) => factors,
            None => {
                configured = numeric_config(&config, "smoothing_factors", 7)?.iter().map(|&v| v as f32).collect();
                &configured
            }
        };
        let smoothing_entries = table_entries(factors, &TRACKER_TABLE)?;
        let recovery_timeout_ms = py_round(numeric_config(&config, "recovery_timeout", 1)?[0].trunc())?;
        let mut tracker = Self {
            landmarks,
            finder,
            quality_ids,
            fitter,
            smoothing_entries,
            recovery_timeout_ms,
            landmark_filter: LandmarkFilter::default(),
            region,
            roll: 0.0,
            state: CoreState::Searching,
            face: NO_FACE,
            object_state: ObjectState::Restarted,
            smoothing_epoch: 0,
            lost_since: -1,
            smoothed: None,
        };
        tracker.reset();
        Ok(tracker)
    }

    pub fn landmark_ids(&self) -> &[FdpId] {
        &self.fitter.model.landmark_ids
    }

    /// Back to searching with fresh smoothing and fitting state; the roll is kept.
    pub fn reset(&mut self) {
        self.reset_core();
        self.fitter.reset();
        self.landmark_filter.reset();
        self.object_state = ObjectState::Restarted;
        self.smoothing_epoch = 0;
        self.lost_since = -1;
        self.smoothed = None;
    }

    /// 0x140049400 (vtable slot 2): back to searching with no box.
    fn reset_core(&mut self) {
        self.state = CoreState::Searching;
        self.face = NO_FACE;
    }

    fn failed_core(&mut self) -> CoreFrame {
        self.reset_core();
        let tracked = TrackedFrame {
            state: self.state,
            face: None,
            working_size: None,
            working_points: None,
            frame_points: None,
            quality_score: f32::NAN,
            accepted: false,
            fit: None,
            roll_degrees: self.roll,
            fitted_points: None,
        };
        CoreFrame { tracked, records: None }
    }

    /// 0x140049450 with the core fit disabled; records are frame-normalised with Y down.
    fn track(&mut self, pixels: &[u8], width: usize, height: usize) -> Result<CoreFrame> {
        let state = self.state;
        let face = if state == CoreState::Searching {
            let options = DetectOptions { region: self.region, ..DetectOptions::default() };
            match self.finder.detect(pixels, width, height, &options)?.first() {
                Some(detection) => detection.rectangle(),
                None => return Ok(self.failed_core()),
            }
        } else {
            self.face
        };
        let mut view = working_view(pixels, width, height, face, EXPANSION, TARGET_FACE_WIDTH)?;
        if state == CoreState::Recovering {
            let found = self.finder.detect(&view.image, view.width, view.height, &REDETECT_OPTIONS)?;
            match best_redetection(&found, view.face) {
                Some(refreshed) => view.face = refreshed,
                None => return Ok(self.failed_core()),
            }
        }
        let (work_w, work_h) = (view.width, view.height);
        let compensate_roll = state == CoreState::Tracking;
        let landmarks = self.landmarks.predict_frame(
            &view.image,
            work_w,
            work_h,
            view.face,
            f64::from(self.roll),
            compensate_roll,
        )?;
        let ids = &self.fitter.model.landmark_ids;
        self.roll = temporal_roll(&landmarks.normalized_points, ids, work_w as i64, work_h as i64)?;
        let score = d1_quality_score(&landmarks.quality, &self.quality_ids)?;
        let accepted = passes_d1_quality(score, state as i64);
        let frame_points = to_frame_points(&landmarks.normalized_points, view.region, width as i64, height as i64);
        if accepted {
            self.face = landmark_box(&frame_points, width as i64, height as i64)?;
            self.state = CoreState::Tracking;
        } else if state == CoreState::Tracking {
            self.face = working_face_in_frame(view.face, view.region, view.ratio)?;
            self.state = CoreState::Recovering;
        } else {
            return Ok(self.failed_core());
        }
        let records = frame_records(&frame_points, &landmarks.quality, ids)?;
        let tracked = TrackedFrame {
            state,
            face: Some(face),
            working_size: Some([work_w, work_h]),
            working_points: Some(landmarks.normalized_points),
            frame_points: Some(frame_points),
            quality_score: score,
            accepted,
            fit: None,
            roll_degrees: self.roll,
            fitted_points: None,
        };
        Ok(CoreFrame { tracked, records: Some(records) })
    }

    /// 0x14004a530 from the core result up to the copy that 0x14004a2c0 fits.
    fn smooth(&mut self, core: &CoreFrame, time_ms: i64) -> Result<Option<Records>> {
        let filter = &mut self.landmark_filter;
        filter.enabled = core.records.is_some();
        filter.begin_frame();
        if let Some(records) = &core.records {
            let mut records = records.clone();
            flip_y(&mut records);
            if self.object_state != ObjectState::Tracking {
                self.smoothing_epoch = time_ms;
                filter.reset();
            }
            smooth_records(filter, &mut records, time_ms - self.smoothing_epoch, &self.smoothing_entries)?;
            self.smoothed = Some(records);
        }
        let fitted = self.smoothed.clone().map(|mut records| {
            flip_y(&mut records);
            records
        });
        if self.object_state == ObjectState::Lost {
            if self.lost_since == -1 {
                self.lost_since = time_ms;
            } else if self.recovery_timeout_ms <= time_ms - self.lost_since {
                self.fitter.reset();
                self.lost_since = -1;
                self.landmark_filter.reset();
                self.object_state = ObjectState::Restarted;
            }
        } else {
            self.lost_since = -1;
        }
        Ok(fitted)
    }

    /// 0x14004a2c0: 0x14007e440 on the flipped copy at frame size; a failure resets the core
    /// and the fitter. Returns the valid fit and the points it fitted.
    fn fit(
        &mut self,
        core: &CoreFrame,
        fitted: Option<&Records>,
        width: usize,
        height: usize,
    ) -> Result<Option<(TemporalFit, Vec<Point>)>> {
        let mut fit = None;
        if let (Some(_), Some(fitted)) = (&core.records, fitted) {
            let mut points = Vec::with_capacity(self.landmark_ids().len());
            for id in self.landmark_ids() {
                let Some(record) = fitted.get(id) else { return invalid(format!("Missing record {id:?}")) };
                points.push([record.x, record.y]);
            }
            fit = Some((self.fitter.process(&points, width as i64, height as i64)?, points));
        }
        match fit {
            Some((fit, points)) if fit.valid => Ok(Some((fit, points))),
            _ => {
                self.reset_core();
                self.fitter.reset();
                Ok(None)
            }
        }
    }

    /// One `width`×`height` grayscale frame taken at `time_ms` milliseconds.
    pub fn process(&mut self, pixels: &[u8], width: usize, height: usize, time_ms: i64) -> Result<TrackedFrame> {
        if pixels.len() != width * height {
            return invalid("Expected one byte per frame pixel");
        }
        let core = self.track(pixels, width, height)?;
        let fitted = self.smooth(&core, time_ms)?;
        let fit = self.fit(&core, fitted.as_ref(), width, height)?;
        if fit.is_some() {
            self.object_state = ObjectState::Tracking;
        } else if self.object_state == ObjectState::Tracking {
            self.object_state = ObjectState::Lost;
        }
        let mut tracked = core.tracked;
        if let Some((fit, points)) = fit {
            tracked.fit = Some(fit);
            tracked.fitted_points = Some(points);
        }
        Ok(tracked)
    }
}

#[cfg(test)]
mod tests;
