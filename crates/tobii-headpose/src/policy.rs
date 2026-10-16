//! Recovered pose validation and the bounded retry policy around the fitting updates.

use crate::error::{Result, invalid};
use crate::fit::{Accumulation, FitMode, ModeStep, NEUTRAL_YAW, initial_rigid_state, mode_step, prepare_observations};
use crate::model::{PoseModel, RESET_POINTS, coefficient_latents, select_pose_landmarks};
use crate::projection::bounded_coefficients;

/// The float32 pi constant the roll check takes its remainder by.
const ROLL_MODULUS: f64 = 3.141592502593994;
/// The caller reset 0x14007e270 stores integer 3; 0x14007e840 converts it to float for
/// the initial yaw. The outer retry uses float32 pi instead.
pub const INITIAL_YAW: f64 = 3.0;

/// The bounds check 0x1400dcc30: `(invalid, cleaned_shape, cleaned_animation)`.
///
/// Roll is checked modulo the observed float32 pi constant, without changing the stored
/// roll. A non-finite state is invalid. Non-finite decoded deformation coefficients are
/// cleared and mark the result invalid. This is not an input-sanitization boundary.
pub fn check_pose_bounds(
    rigid_state: &[f32; 6],
    shape_coefficients: &[f32],
    animation_coefficients: &[f32],
    rotation_limits: &[[f32; 2]; 3],
    translation_limits: &[[f32; 2]; 3],
) -> Result<(bool, Vec<f32>, Vec<f32>)> {
    let limits: Vec<[f32; 2]> = rotation_limits.iter().chain(translation_limits).copied().collect();
    if limits.iter().any(|[low, high]| !low.is_finite() || !high.is_finite() || low > high) {
        return invalid("Invalid pose or bound dimensions");
    }
    let mut invalid_pose = !rigid_state.iter().all(|v| v.is_finite());
    let mut checked = *rigid_state;
    let roll = f64::from(rigid_state[2]);
    checked[2] = if roll.is_finite() { (roll % ROLL_MODULUS) as f32 } else { f32::NAN };
    invalid_pose |= checked.iter().zip(&limits).any(|(&v, &[low, high])| v < low || v > high);
    let mut clean = |coefficients: &[f32]| -> Vec<f32> {
        let cleaned = coefficients.iter().map(|&c| if c.is_finite() { c } else { 0.0 }).collect();
        invalid_pose |= !coefficients.iter().all(|c| c.is_finite());
        cleaned
    };
    let (shape, animation) = (clean(shape_coefficients), clean(animation_coefficients));
    Ok((invalid_pose, shape, animation))
}

/// Result of [`fit_landmarks`]: the full latent state, the decoded coefficients, whether
/// the bounds held, and the attempts and updates used.
#[derive(Debug, Clone, PartialEq)]
pub struct LandmarkFit {
    pub state: Vec<f32>,
    pub shape_coefficients: Vec<f32>,
    pub animation_coefficients: Vec<f32>,
    pub valid: bool,
    pub attempts: u32,
    pub updates: u32,
}

fn rigid(state: &[f32]) -> [f32; 6] {
    std::array::from_fn(|i| state[i])
}

pub(crate) fn reset_indices(model: &PoseModel) -> Result<[usize; 4]> {
    let mut indices = [0; 4];
    for (slot, id) in indices.iter_mut().zip(RESET_POINTS) {
        let Some(index) = model.landmark_index(id) else {
            return invalid(format!("Reset landmark ({}, {}) is not in the model", id.0, id.1));
        };
        *slot = index;
    }
    Ok(indices)
}

/// The rigid reset 0x140052d70 from the four reset landmarks of `points` (in model order).
pub(crate) struct Reset {
    model_points: [[f32; 3]; 4],
    image_points: [[f32; 2]; 4],
    width: i64,
    height: i64,
    focal_factor: f64,
}

impl Reset {
    pub(crate) fn new(model: &PoseModel, points: &[[f32; 2]], width: i64, height: i64) -> Result<Self> {
        let indices = reset_indices(model)?;
        Ok(Self {
            model_points: indices.map(|i| model.vertices[i]),
            image_points: indices.map(|i| points[i]),
            width,
            height,
            focal_factor: f64::from(model.focal_factor),
        })
    }

    pub(crate) fn pose(&self, yaw: f64) -> Result<[f32; 6]> {
        initial_rigid_state(&self.model_points, &self.image_points, self.width, self.height, self.focal_factor, yaw)
    }
}

fn decode(model: &PoseModel, state: &mut [f32]) -> Result<(Vec<f32>, Vec<f32>)> {
    let ns = model.shape_count();
    let (shape, clamped) = bounded_coefficients(&state[6..6 + ns], &model.shape_ranges)?;
    state[6..6 + ns].copy_from_slice(&clamped);
    let (animation, clamped) = bounded_coefficients(&state[6 + ns..], &model.animation_ranges)?;
    state[6 + ns..].copy_from_slice(&clamped);
    Ok((shape, animation))
}

/// Fits complete D1 landmark records with the recovered bounded retry policy.
///
/// Supports the no-external-pose-hint branch. All landmarks must be present. Missing-record
/// retention, view-dependent groups, temporal selection and conversion to public head-pose
/// coordinates remain separate.
pub fn fit_landmarks(
    model: &PoseModel,
    normalized_points: &[[f32; 2]],
    width: i64,
    height: i64,
    state: Option<&[f32]>,
    iterations: u32,
) -> Result<LandmarkFit> {
    if iterations > 100 {
        return invalid("Expected a bounded nonnegative iteration count");
    }
    let (observations, sensitivity) = prepare_observations(normalized_points, &model.landmark_ids, width, height)?;
    let reset = Reset::new(model, normalized_points, width, height)?;
    let mut current = match state {
        None => model.initial_state(&reset.pose(NEUTRAL_YAW)?)?,
        Some(state) => state.to_vec(),
    };
    if current.len() != model.state_count() || !current.iter().all(|v| v.is_finite()) {
        return invalid("Expected a finite initialized fitting state");
    }
    let (mut shape, mut animation) = decode(model, &mut current)?;
    let (mut updates, mut attempts, mut invalid_pose) = (0, 0, true);
    for attempt in 0..2 {
        if attempt > 0 {
            current = reset.pose(NEUTRAL_YAW)?.to_vec();
            current.extend(coefficient_latents(&shape, &model.shape_ranges)?);
            current.extend(coefficient_latents(&animation, &model.animation_ranges)?);
        }
        for _ in 0..if attempt == 0 { iterations } else { 3 } {
            current = model.step(&observations, &current, Some(&sensitivity))?;
            (shape, animation) = decode(model, &mut current)?;
            updates += 1;
        }
        attempts = attempt + 1;
        (invalid_pose, shape, animation) =
            check_pose_bounds(&rigid(&current), &shape, &animation, &model.rotation_limits, &model.translation_limits)?;
        if !invalid_pose {
            break;
        }
    }
    Ok(LandmarkFit {
        state: current,
        shape_coefficients: shape,
        animation_coefficients: animation,
        valid: !invalid_pose,
        attempts,
        updates,
    })
}

/// One stage of [`fit_staged_landmarks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    pub mode: FitMode,
    pub updates: u32,
    pub attempts: u32,
    pub valid: bool,
}

/// Result of [`fit_staged_landmarks`].
#[derive(Debug, Clone, PartialEq)]
pub struct StagedFit {
    pub rigid_state: [f32; 6],
    pub shape_coefficients: Vec<f32>,
    pub animation_coefficients: Vec<f32>,
    pub valid: bool,
    pub stages: Vec<Stage>,
}

/// The no-hint, single-parent 3 -> 3 -> 5 caller branch on complete mapped landmarks.
///
/// Input points follow `model.landmark_ids`; the selection is reordered to the original
/// numeric FDP scan. Confidence gating must happen before the result is used for tracking.
/// Optional extra parent models and temporal caller state are absent.
pub fn fit_staged_landmarks(
    model: &PoseModel,
    normalized_points: &[[f32; 2]],
    width: i64,
    height: i64,
) -> Result<StagedFit> {
    if normalized_points.len() != model.landmark_ids.len() || !normalized_points.iter().flatten().all(|v| v.is_finite())
    {
        return invalid("Expected complete finite normalized landmarks");
    }
    let (selected, order) = select_pose_landmarks(model, &model.landmark_ids)?;
    let points: Vec<[f32; 2]> = order.iter().map(|&i| normalized_points[i]).collect();
    let (observations, weights) = prepare_observations(&points, &selected.landmark_ids, width, height)?;
    let reset = Reset::new(&selected, &points, width, height)?;
    let mut pose = reset.pose(INITIAL_YAW)?;
    let mut shape = vec![0.0; selected.shape_count()];
    let mut animation = vec![0.0; selected.animation_count()];
    let mut stages = vec![];
    let mut invalid_pose = false;
    for (mode, iterations) in [(FitMode::Shape, 2), (FitMode::Animation, 1)] {
        let (mut updates, mut attempts) = (0, 0);
        let mut active: Option<Vec<f32>> = None;
        for attempt in 0..2 {
            if attempt > 0 {
                pose = reset.pose(NEUTRAL_YAW)?;
                active = None;
            }
            for _ in 0..if attempt == 0 { iterations } else { 3 } {
                let ModeStep { rigid_state, shape_coefficients, animation_coefficients, active_state } = mode_step(
                    &selected,
                    &observations,
                    &pose,
                    &shape,
                    &animation,
                    mode,
                    Some(&weights),
                    active.as_deref(),
                    Accumulation::Double,
                )?;
                (pose, shape, animation, active) =
                    (rigid_state, shape_coefficients, animation_coefficients, Some(active_state));
                updates += 1;
            }
            attempts = attempt + 1;
            (invalid_pose, shape, animation) =
                check_pose_bounds(&pose, &shape, &animation, &selected.rotation_limits, &selected.translation_limits)?;
            if !invalid_pose {
                break;
            }
        }
        stages.push(Stage { mode, updates, attempts, valid: !invalid_pose });
        if invalid_pose {
            break;
        }
    }
    Ok(StagedFit {
        rigid_state: pose,
        shape_coefficients: shape,
        animation_coefficients: animation,
        valid: !invalid_pose,
        stages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_reject_nan_and_clear_invalid_coefficients() {
        let (invalid, shape, animation) = check_pose_bounds(
            &[0.0, f32::NAN, 0.0, 0.0, 0.0, 0.65],
            &[f32::NAN, 0.2],
            &[f32::INFINITY, -0.1],
            &[[-2.0, 2.0], [1.0, 5.0], [-3.2, 3.2]],
            &[[-4.0, 4.0], [-3.0, 3.0], [0.0, 11.0]],
        )
        .unwrap();
        assert!(invalid);
        assert_eq!(shape, [0.0, 0.2]);
        assert_eq!(animation, [0.0, -0.1]);
    }

    #[test]
    fn roll_is_checked_modulo_pi() {
        let limits = [[-1.0, 1.0], [-4.0, 4.0], [-0.5, 0.5]];
        let translation = [[-1.0, 1.0]; 3];
        let (invalid, _, _) =
            check_pose_bounds(&[0.0, 0.0, 3.3, 0.0, 0.0, 0.0], &[], &[], &limits, &translation).unwrap();
        assert!(!invalid);
        assert!(check_pose_bounds(&[0.0; 6], &[], &[], &[[1.0, 0.0]; 3], &translation).is_err());
    }
}
