//! Recovered temporal fitting caller 0x14007e440 for the single-parent model.
//!
//! VNNTrackerCore keeps one parent face model across frames. The first frame (or the first
//! after tracking is lost) runs the flag-3 initial fit 0x14007ec70 and then the flag-5
//! temporal fit 0x14007ee60; later frames run only the temporal fit, starting from the
//! previous frame's pose and coefficients.

use crate::array::Vec3;
use crate::error::{Error, Result, invalid, unsupported};
use crate::fit::{Accumulation, FitMode, mode_step, prepare_observations};
use crate::model::{FdpId, PoseModel, select_pose_landmarks};
use crate::output::PI;
use crate::policy::{INITIAL_YAW, Reset, check_pose_bounds, reset_indices};
use crate::projection::{deform_vertices, project_points};

pub const FACING_PI: f32 = 3.141590118408203;
pub const RADIANS_TO_DEGREES: f32 = 57.2957763671875;
pub const FACING_REINITIALISE_DEGREES: f32 = 30.0;
pub const REPROJECTION_ERROR_LIMIT: f32 = 0.1;
pub const FRONTAL_LIMIT: f32 = 0.15707963705062866;
pub const RECTANGLE_SCALE: f32 = 1.8;
pub const RECTANGLE_EYE_WEIGHT: f32 = 0.2;
pub const RECTANGLE_NOSE_WEIGHT: f32 = 0.8;
pub const ERROR_EXCLUDED_GROUPS: [u32; 2] = [13, 15];
pub const MINIMUM_FIT_POINTS: usize = 4;

/// The pose a lost track reports: facing the camera at the origin.
pub const LOST_POSE: [f32; 6] = [0.0, PI, 0.0, 0.0, 0.0, 0.0];

/// `cvtps2pd` then `cvtsd2si` under the default round-to-nearest-even mode.
fn round(value: f32) -> Result<i64> {
    if !value.is_finite() {
        return invalid("cannot convert a non-finite float to an integer");
    }
    Ok(value.round_ties_even() as i64)
}

/// 0x14007ee60 facing angle from the model pose, before that attempt's fit.
pub fn facing_degrees(rigid_state: &[f32; 6]) -> f32 {
    let bearing = f64::from(rigid_state[3]).atan2(f64::from(rigid_state[5])) as f32;
    ((rigid_state[1] - bearing) - FACING_PI) * RADIANS_TO_DEGREES
}

/// 0x140055470 output: `(rx, ry - pi, rz)` and the translation.
pub fn pose_output(rigid_state: &[f32; 6]) -> (Vec3, Vec3) {
    let [rx, ry, rz, tx, ty, tz] = *rigid_state;
    ([rx, ry - PI, rz], [tx, ty, tz])
}

/// COMISS + JAE: a NaN angle counts as below the limit.
pub fn is_near_frontal(output_rotation: [f32; 2]) -> bool {
    !(output_rotation[0].abs() >= FRONTAL_LIMIT || output_rotation[1].abs() >= FRONTAL_LIMIT)
}

/// A pixel rectangle `(x, y, width, height)`.
pub type Rectangle = [i64; 4];

/// 0x14007a090 square from the eye centres, nose (9.4/9.5) and FDP 8.1.
///
/// `None` when a required record is absent; the original then leaves its output
/// uninitialised. A later duplicate identifier replaces an earlier one.
pub fn eye_face_rectangle(
    normalized_points: &[[f32; 2]],
    landmark_ids: &[FdpId],
    width: i64,
    height: i64,
) -> Result<Option<Rectangle>> {
    let lookup = |id: FdpId| landmark_ids.iter().rposition(|&known| known == id).map(|i| normalized_points[i]);
    const REQUIRED: [FdpId; 7] = [(3, 11), (3, 7), (3, 12), (3, 8), (9, 5), (9, 4), (8, 1)];
    if REQUIRED.iter().any(|&id| lookup(id).is_none()) {
        return Ok(None);
    }
    let (w, h) = (width as f64 as f32, height as f64 as f32);
    let pixel = |id: FdpId| -> Result<[i64; 2]> {
        let [x, y] = lookup(id).unwrap();
        Ok([round(w * x)?, round(y * h)?])
    };
    let middle = |a: [i64; 2], b: [i64; 2]| -> Result<[i64; 2]> {
        Ok([round((a[0] + b[0]) as f64 as f32 * 0.5)?, round((a[1] + b[1]) as f64 as f32 * 0.5)?])
    };
    let left = middle(pixel((3, 11))?, pixel((3, 7))?)?;
    let right = middle(pixel((3, 12))?, pixel((3, 8))?)?;
    let nose = middle(pixel((9, 5))?, pixel((9, 4))?)?;
    let eyes = middle(right, left)?;
    let mouth = pixel((8, 1))?;
    let squared = |a: [i64; 2], b: [i64; 2]| {
        let (dx, dy) = ((a[0] - b[0]) as f64 as f32, (a[1] - b[1]) as f64 as f32);
        dy * dy + dx * dx
    };
    let (right_mouth, left_right, mut left_mouth) = (squared(right, mouth), squared(left, right), squared(left, mouth));
    let largest = if right_mouth.sqrt() >= left_right.sqrt() { right_mouth } else { left_right };
    if !(left_mouth.sqrt() >= largest.sqrt()) {
        left_mouth = if right_mouth.sqrt() < left_right.sqrt() { left_right } else { right_mouth };
    }
    let size = round(left_mouth.sqrt() * RECTANGLE_SCALE)?;
    let half = size as f64 as f32 * 0.5;
    let corner = |eye: i64, nose: i64| {
        round(((eye as f64 as f32 * RECTANGLE_EYE_WEIGHT) + (nose as f64 as f32 * RECTANGLE_NOSE_WEIGHT)) - half)
    };
    Ok(Some([corner(eyes[0], nose[0])?, corner(eyes[1], nose[1])?, size, size]))
}

/// Mesh finalization 0x140052710 projection (+0xb8) with zero eye offsets.
///
/// The divide is `focal / Z` followed by `X * (focal / Z)`, unlike the fitting prediction.
pub fn finalized_projection(
    model: &PoseModel,
    rigid_state: &[f32; 6],
    shape_coefficients: &[f32],
    animation_coefficients: &[f32],
) -> Result<Vec<[f32; 2]>> {
    let deformed = deform_vertices(
        &model.vertices,
        &model.shape_basis,
        shape_coefficients,
        &model.animation_basis,
        animation_coefficients,
    )?;
    let [rx, ry, rz, tx, ty, tz] = *rigid_state;
    let (_, camera) = project_points(&deformed, [rx, ry, rz], [tx, ty, tz], model.focal_factor, 1.0)?;
    Ok(camera.iter().map(|&[x, y, z]| [x * (model.focal_factor / z), y * (model.focal_factor / z)]).collect())
}

/// 0x1400551c0 mean pixel distance between records and model, over the rectangle width.
///
/// `flipped_points` are the Y-flipped caller copy (0x14007ac80) in `landmark_ids` order.
/// Groups 13 and 15 are skipped; summation follows numeric FDP order. No counted point
/// gives NaN.
pub fn reprojection_error(
    landmark_ids: &[FdpId],
    projected: &[[f32; 2]],
    flipped_points: &[[f32; 2]],
    width: i64,
    height: i64,
    rectangle_width: f64,
) -> f32 {
    let (w, h) = (width as f64 as f32, height as f64 as f32);
    let aspect_x = if height < width { w / h } else { 1.0 };
    let aspect_y = if width < height { h / w } else { 1.0 };
    let inverse = 1.0 / rectangle_width as f32;
    let mut order: Vec<usize> = (0..landmark_ids.len()).collect();
    order.sort_by_key(|&i| landmark_ids[i]);
    let (mut total, mut count) = (0.0f32, 0u32);
    for index in order {
        if ERROR_EXCLUDED_GROUPS.contains(&landmark_ids[index].0) {
            continue;
        }
        let model_x = 0.5 - (projected[index][0] * 0.5) / aspect_x;
        let model_y = (projected[index][1] * 0.5) / aspect_y + 0.5;
        let [record_x, record_y] = flipped_points[index];
        let dy = h * record_y - h * model_y;
        let dx = w * record_x - w * model_x;
        total += (dx * dx + dy * dy).sqrt() * inverse;
        count += 1;
    }
    total / count as f32
}

/// The temporal measures of one flag-5 fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemporalMeasure {
    pub facing: f32,
    pub error: f32,
    pub rectangle: Rectangle,
}

/// One fit of a frame: its flag, whether the bounds held and its attempts, plus the
/// temporal measures for a temporal fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitTrace {
    pub mode: FitMode,
    pub valid: bool,
    pub attempts: u32,
    pub measure: Option<TemporalMeasure>,
}

/// The output of one [`TemporalPoseFitter::process`] call.
#[derive(Debug, Clone, PartialEq)]
pub struct TemporalFit {
    pub valid: bool,
    pub rotation: Vec3,
    pub translation: Vec3,
    pub rigid_state: [f32; 6],
    pub shape_coefficients: Vec<f32>,
    pub animation_coefficients: Vec<f32>,
    pub initialised: bool,
    pub frontal: bool,
    pub trace: Vec<FitTrace>,
}

/// Stateful 0x14007e440 caller (last argument 1) for complete D1 landmarks.
///
/// Persistent state mirrors caller +0x59 (`initialised`), +0x39 (frontal shape refit done,
/// `frontal`), the parent model's pose and physical coefficients, and the fitting object
/// built by 0x1400dbba0. That object keeps its own latent state (`fit_state`) and is
/// re-encoded from the parent only when the requested flag changes, so parent pose resets
/// made between two same-flag fits do not move the next fit's start. Extra parent models
/// and the caller +0x58 external pose hint are not supported.
#[derive(Debug, Clone)]
pub struct TemporalPoseFitter {
    pub model: PoseModel,
    pub selected: PoseModel,
    pub order: Vec<usize>,
    pub rigid_state: [f32; 6],
    pub projected: Vec<[f32; 2]>,
    pub initialised: bool,
    pub frontal: bool,
    pub shape: Vec<f32>,
    pub animation: Vec<f32>,
    pub fit_mode: Option<FitMode>,
    pub fit_state: Option<Vec<f32>>,
    pub fixed_shape: Option<Vec<f32>>,
    pub fixed_animation: Option<Vec<f32>>,
    pub accumulation: Accumulation,
}

impl TemporalPoseFitter {
    pub fn new(model: PoseModel) -> Result<Self> {
        Self::with_accumulation(model, Accumulation::Double)
    }

    /// A fitter whose updates accumulate as `accumulation` says; `Single` only measures
    /// arithmetic sensitivity.
    pub fn with_accumulation(model: PoseModel, accumulation: Accumulation) -> Result<Self> {
        let (selected, order) = select_pose_landmarks(&model, &model.landmark_ids)?;
        reset_indices(&selected)?;
        let projected = vec![[0.0; 2]; selected.landmark_ids.len()];
        let mut fitter = Self {
            shape: vec![],
            animation: vec![],
            model,
            selected,
            order,
            rigid_state: [0.0; 6],
            projected,
            initialised: false,
            frontal: false,
            fit_mode: None,
            fit_state: None,
            fixed_shape: None,
            fixed_animation: None,
            accumulation,
        };
        fitter.reset();
        Ok(fitter)
    }

    /// Caller reset 0x14007e270 plus the fitting-object rebuild it triggers in 0x14007e330.
    /// The parent pose is left as it was.
    pub fn reset(&mut self) {
        self.initialised = false;
        self.frontal = false;
        self.shape = vec![0.0; self.model.shape_count()];
        self.animation = vec![0.0; self.model.animation_count()];
        self.fit_mode = None;
        self.fit_state = None;
        self.fixed_shape = None;
        self.fixed_animation = None;
    }

    /// One frame of complete normalized landmarks in `model.landmark_ids` order.
    pub fn process(&mut self, normalized_points: &[[f32; 2]], width: i64, height: i64) -> Result<TemporalFit> {
        if normalized_points.len() != self.model.landmark_ids.len()
            || !normalized_points.iter().flatten().all(|v| v.is_finite())
        {
            return invalid("Expected complete finite normalized landmarks");
        }
        let points: Vec<[f32; 2]> = self.order.iter().map(|&i| normalized_points[i]).collect();
        let (observations, weights) = prepare_observations(&points, &self.selected.landmark_ids, width, height)?;
        let reset = Reset::new(&self.selected, &points, width, height)?;
        let flipped = points.iter().map(|&[x, y]| [x, 1.0 - y]).collect();
        let mut frame =
            Frame { fitter: self, points, flipped, observations, weights, reset, width, height, trace: vec![] };
        if !frame.fitter.initialised {
            frame.fitter.initialised = frame.initial_fit()?;
            frame.fitter.frontal = frame.fitter.is_frontal();
        }
        let valid = frame.temporal_fit()?;
        if !valid {
            frame.fitter.rigid_state = LOST_POSE;
        } else if frame.fitter.is_frontal() && !frame.fitter.frontal {
            frame.fitter.initialised = frame.initial_fit()?;
            frame.temporal_fit()?;
            frame.fitter.frontal = true;
        }
        let trace = frame.trace;
        let (rotation, translation) = pose_output(&self.rigid_state);
        Ok(TemporalFit {
            valid,
            rotation,
            translation,
            rigid_state: self.rigid_state,
            shape_coefficients: self.shape.clone(),
            animation_coefficients: self.animation.clone(),
            initialised: self.initialised,
            frontal: self.frontal,
            trace,
        })
    }

    fn is_frontal(&self) -> bool {
        let (rotation, _) = pose_output(&self.rigid_state);
        is_near_frontal([rotation[0], rotation[1]])
    }
}

/// One frame's inputs and fit trace.
struct Frame<'a> {
    fitter: &'a mut TemporalPoseFitter,
    points: Vec<[f32; 2]>,
    flipped: Vec<[f32; 2]>,
    observations: Vec<[f32; 2]>,
    weights: Vec<f32>,
    reset: Reset,
    width: i64,
    height: i64,
    trace: Vec<FitTrace>,
}

impl Frame<'_> {
    /// 0x1400dbba0: a new fitting object only when the flag differs from the last one.
    fn build(&mut self, mode: FitMode) -> Option<Vec<f32>> {
        let fitter = &mut *self.fitter;
        if fitter.fit_mode == Some(mode) {
            return fitter.fit_state.clone();
        }
        fitter.fit_mode = Some(mode);
        fitter.fixed_shape = Some(fitter.shape.clone());
        fitter.fixed_animation = Some(fitter.animation.clone());
        None
    }

    /// 0x1400dbba0 + 0x1400dc3b0 without a pose hint; true when the bounds hold.
    ///
    /// Decoding writes only the active coefficient block back to the parent; the fitting
    /// object keeps the inactive block copied when it was built.
    fn fit(&mut self, mode: FitMode, iterations: u32) -> Result<bool> {
        let mut active = self.build(mode);
        let fitter = &mut *self.fitter;
        if fitter.selected.landmark_ids.len() < MINIMUM_FIT_POINTS {
            return Ok(false);
        }
        let missing = || Error::Value("The fitting object has no inactive coefficients".into());
        let (mut valid, mut attempts, mut iterations) = (false, 0, iterations);
        for attempt in 0..2 {
            if attempt > 0 {
                fitter.rigid_state = self.reset.pose(f64::from(PI))?;
                active = None;
                iterations = 3;
            }
            for _ in 0..iterations {
                let shape = match mode {
                    FitMode::Shape => &fitter.shape,
                    FitMode::Animation => fitter.fixed_shape.as_ref().ok_or_else(missing)?,
                };
                let animation = match mode {
                    FitMode::Animation => &fitter.animation,
                    FitMode::Shape => fitter.fixed_animation.as_ref().ok_or_else(missing)?,
                };
                let result = mode_step(
                    &fitter.selected,
                    &self.observations,
                    &fitter.rigid_state,
                    shape,
                    animation,
                    mode,
                    Some(&self.weights),
                    active.as_deref(),
                    fitter.accumulation,
                )?;
                fitter.rigid_state = result.rigid_state;
                active = Some(result.active_state);
                match mode {
                    FitMode::Shape => fitter.shape = result.shape_coefficients,
                    FitMode::Animation => fitter.animation = result.animation_coefficients,
                }
            }
            fitter.fit_state.clone_from(&active);
            attempts = attempt + 1;
            let selected = &fitter.selected;
            let (invalid_pose, shape, animation) = check_pose_bounds(
                &fitter.rigid_state,
                &fitter.shape,
                &fitter.animation,
                &selected.rotation_limits,
                &selected.translation_limits,
            )?;
            (fitter.shape, fitter.animation) = (shape, animation);
            if !invalid_pose {
                fitter.projected =
                    finalized_projection(selected, &fitter.rigid_state, &fitter.shape, &fitter.animation)?;
                valid = true;
                break;
            }
        }
        self.trace.push(FitTrace { mode, valid, attempts, measure: None });
        Ok(valid)
    }

    /// 0x14007ec70: zero coefficients, reset with yaw float(3), flag 3 for two updates.
    fn initial_fit(&mut self) -> Result<bool> {
        let fitter = &mut *self.fitter;
        fitter.shape.fill(0.0);
        fitter.animation.fill(0.0);
        fitter.rigid_state = self.reset.pose(INITIAL_YAW)?;
        self.fit(FitMode::Shape, 2)
    }

    /// 0x14007ee60: up to two flag-5 single-update attempts from the current pose.
    fn temporal_fit(&mut self) -> Result<bool> {
        let (mut valid, mut attempt) = (false, 0);
        while self.fitter.initialised {
            if valid || attempt >= 2 {
                break;
            }
            let facing = facing_degrees(&self.fitter.rigid_state);
            valid = self.fit(FitMode::Animation, 1)?;
            let selected = &self.fitter.selected;
            let Some(rectangle) = eye_face_rectangle(&self.points, &selected.landmark_ids, self.width, self.height)?
            else {
                return unsupported("Original rectangle output is uninitialised without eye/nose/mouth records");
            };
            let error = reprojection_error(
                &selected.landmark_ids,
                &self.fitter.projected,
                &self.flipped,
                self.width,
                self.height,
                rectangle[2] as f64,
            );
            let Some(last) = self.trace.last_mut() else {
                return invalid("No fit to measure");
            };
            last.measure = Some(TemporalMeasure { facing, error, rectangle });
            if !valid || (error > REPROJECTION_ERROR_LIMIT && attempt == 0) {
                if -FACING_REINITIALISE_DEGREES < facing && facing < FACING_REINITIALISE_DEGREES {
                    self.fitter.initialised = self.initial_fit()?;
                } else {
                    self.fitter.rigid_state = self.reset.pose(f64::from(PI))?;
                }
                valid = false;
            }
            attempt += 1;
        }
        Ok(valid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_uses_largest_eye_mouth_distance() {
        let ids = [(3, 11), (3, 7), (3, 12), (3, 8), (9, 4), (9, 5), (8, 1)];
        let points = [[0.4, 0.4], [0.3, 0.4], [0.6, 0.4], [0.7, 0.4], [0.5, 0.6], [0.5, 0.6], [0.5, 0.8]];
        assert_eq!(eye_face_rectangle(&points, &ids, 100, 100).unwrap(), Some([12, 18, 77, 77]));
        assert_eq!(eye_face_rectangle(&points[..6], &ids[..6], 100, 100).unwrap(), None);
    }

    #[test]
    fn reprojection_error_skips_groups_13_and_15() {
        let ids = [(3, 1), (13, 1), (15, 1)];
        let flipped = [[0.25, 0.75], [0.1, 0.1], [0.9, 0.9]];
        let mut projected = [[0.5, 0.5], [0.0, 0.0], [0.0, 0.0]];
        assert_eq!(reprojection_error(&ids, &projected, &flipped, 400, 400, 50.0), 0.0);
        projected[0][0] = 0.0;
        assert!((reprojection_error(&ids, &projected, &flipped, 400, 400, 50.0) - 2.0).abs() < 1e-6);
        assert!(reprojection_error(&ids[1..2], &projected[..1], &flipped[..1], 400, 400, 50.0).is_nan());
    }

    #[test]
    fn frontal_test_and_facing_angle() {
        assert!(is_near_frontal([0.15, -0.15]));
        assert!(!is_near_frontal([0.0, 0.15707963705062866]));
        assert!(is_near_frontal([f32::NAN, 0.0]));
        assert!((facing_degrees(&[0.0, PI + 0.5, 0.0, 0.0, 0.0, 0.65]) - 28.648).abs() < 0.01);
        assert!((facing_degrees(&[0.0, PI, 0.0, 0.65, 0.0, 0.65]) + 45.0).abs() < 0.001);
    }
}
