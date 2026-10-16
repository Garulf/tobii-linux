//! Recovered fitting updates: the rigid step, the full deformation step and the
//! mode-specific steps of the outer policy, plus their observations and reset pose.

use crate::array::{Basis, Matrix, Vec3};
use crate::error::{Result, invalid, unsupported};
use crate::linalg::inverse_f32;
use crate::model::{FdpId, PoseModel, coefficient_latents};
use crate::output::PI;
use crate::projection::{Deformation, bounded_coefficients, model_jacobian, project_points, rigid_jacobian};

/// How the update's matrix products accumulate.
///
/// `Double` is the runtime's arithmetic: float32 inputs, double-precision sums in index
/// order, one rounding to float32. `Single` sums in float32 instead; it only measures how
/// sensitive a fit is to arithmetic order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accumulation {
    #[default]
    Double,
    Single,
}

impl Accumulation {
    fn dot(self, terms: impl Iterator<Item = (f32, f32)>, offset: Option<f32>) -> f32 {
        match self {
            Self::Double => {
                let sum = terms.fold(0.0f64, |total, (a, b)| total + f64::from(a) * f64::from(b));
                offset.map_or(sum, |extra| sum + f64::from(extra)) as f32
            }
            Self::Single => {
                let sum = terms.fold(0.0f32, |total, (a, b)| total + a * b);
                offset.map_or(sum, |extra| sum + extra)
            }
        }
    }
}

/// A deformation block of the full state: basis, coefficient bounds and enabled units.
#[derive(Debug, Clone, Copy)]
pub struct Block<'a> {
    pub basis: &'a Basis,
    pub ranges: &'a [[f32; 2]],
    pub mask: Option<&'a [bool]>,
}

impl<'a> Block<'a> {
    fn deformation(self, latent: &'a [f32]) -> Deformation<'a> {
        Deformation { basis: self.basis, latent, ranges: self.ranges, mask: self.mask, fixed: None }
    }
}

/// Observation sensitivity at or above this excludes a point from the update.
pub const EXCLUDED_SENSITIVITY: f32 = 100_000.0;
/// Sensitivity of the observed D1 landmarks and of those the fit ignores.
pub const OBSERVED_SENSITIVITY: f32 = 0.002;
pub const IGNORED_SENSITIVITY: f32 = 100_001.0;
const IGNORED_POINTS: [FdpId; 3] = [(2, 1), (3, 5), (3, 6)];

fn sensitivity_supported(state: &[f32], observation: &[f32]) -> bool {
    state.iter().all(|&s| s.is_finite() && s > 1e-8) && observation_supported(observation)
}

fn observation_supported(observation: &[f32]) -> bool {
    observation.iter().all(|&s| s.is_finite() && s > 1e-8 && s < 1e8)
}

/// The regularized weighted update shared by every step.
///
/// Rows of points whose x observation sensitivity reaches [`EXCLUDED_SENSITIVITY`] are
/// zeroed (the original gates both rows on the x value). The normal matrix
/// `J^T W J + diag(1 / state_sensitivity)` is inverted with [`inverse_f32`].
fn update(
    jacobian: &Matrix,
    prediction: &[[f32; 2]],
    observed: &[[f32; 2]],
    state: &[f32],
    state_sensitivity: &[f32],
    observation_sensitivity: &[f32],
    accumulation: Accumulation,
) -> Result<Vec<f32>> {
    let (rows, cols) = (jacobian.rows, jacobian.cols);
    let mut transposed = Matrix::zeros(cols, rows);
    for r in 0..rows {
        if observation_sensitivity[r - r % 2] >= EXCLUDED_SENSITIVITY {
            continue;
        }
        for c in 0..cols {
            transposed.set(c, r, jacobian.get(r, c));
        }
    }
    let inverse_sensitivity: Vec<f32> = observation_sensitivity.iter().map(|&s| 1.0 / s).collect();
    let mut weighted = transposed.clone();
    for c in 0..cols {
        for (r, inverse) in inverse_sensitivity.iter().enumerate() {
            weighted.set(c, r, transposed.get(c, r) * inverse);
        }
    }
    let mut normal = Matrix::zeros(cols, cols);
    for (i, sensitivity) in state_sensitivity.iter().enumerate().take(cols) {
        for j in 0..cols {
            let diagonal = if i == j { 1.0 / sensitivity } else { 0.0 };
            let terms = weighted.row(i).iter().copied().zip(transposed.row(j).iter().copied());
            normal.set(i, j, accumulation.dot(terms, Some(diagonal)));
        }
    }
    let inverse = inverse_f32(&normal)?;
    let mut gain = Matrix::zeros(cols, rows);
    for c in 0..cols {
        for r in 0..rows {
            let terms = (0..cols).map(|k| (inverse.get(c, k), weighted.get(k, r)));
            gain.set(c, r, accumulation.dot(terms, None));
        }
    }
    let residual: Vec<f32> = observed.iter().zip(prediction).flat_map(|(o, p)| [o[0] - p[0], o[1] - p[1]]).collect();
    Ok((0..cols)
        .map(|c| accumulation.dot(gain.row(c).iter().copied().zip(residual.iter().copied()), Some(state[c])))
        .collect())
}

fn finite(values: &[f32]) -> bool {
    values.iter().all(|v| v.is_finite())
}

fn finite2(points: &[[f32; 2]]) -> bool {
    points.iter().all(|p| finite(p))
}

fn ones(count: usize) -> Vec<f32> {
    vec![1.0; count]
}

/// One regularized weighted update of the six rigid pose states.
///
/// The original update uses inverse sensitivity diagonals and an explicitly inverted
/// normal matrix. This finite positive-sensitivity implementation covers the observed
/// point exclusion threshold but not near-zero sensitivity fallbacks or deformation states.
pub fn rigid_step(
    vertices: &[Vec3],
    observations: &[[f32; 2]],
    state: &[f32; 6],
    focal_factor: f32,
    model_scale: f32,
    state_sensitivity: Option<&[f32]>,
    observation_sensitivity: Option<&[f32]>,
) -> Result<[f32; 6]> {
    if observations.len() != vertices.len() || !finite(state) || !finite2(observations) {
        return invalid("Expected six finite state values and matching Nx2 observations");
    }
    let state_sensitivity = state_sensitivity.map_or_else(|| ones(6), <[f32]>::to_vec);
    let observation_sensitivity = observation_sensitivity.map_or_else(|| ones(2 * observations.len()), <[f32]>::to_vec);
    if state_sensitivity.len() != 6
        || observation_sensitivity.len() != 2 * observations.len()
        || !sensitivity_supported(&state_sensitivity, &observation_sensitivity)
    {
        return invalid("Unsupported sensitivity extent/range");
    }
    let rotation = [state[0], state[1], state[2]];
    let translation = [state[3], state[4], state[5]];
    let (prediction, _) = project_points(vertices, rotation, translation, focal_factor, model_scale)?;
    let jacobian = rigid_jacobian(vertices, rotation, translation, focal_factor, model_scale)?;
    let updated = update(
        &jacobian,
        &prediction,
        observations,
        state,
        &state_sensitivity,
        &observation_sensitivity,
        Accumulation::Double,
    )?;
    Ok(std::array::from_fn(|i| updated[i]))
}

/// Valid FDP landmarks in the fitting plane with their observation sensitivities.
///
/// Supports D1 landmark groups. Group 10/13 view-dependent weighting and missing landmark
/// retention require additional tracker state and are not implemented.
pub fn prepare_observations(
    normalized_points: &[[f32; 2]],
    landmark_ids: &[FdpId],
    width: i64,
    height: i64,
) -> Result<(Vec<[f32; 2]>, Vec<f32>)> {
    if normalized_points.len() != landmark_ids.len()
        || !(1..=32768).contains(&width)
        || !(1..=32768).contains(&height)
        || !finite2(normalized_points)
    {
        return invalid("Expected finite normalized XY landmarks and image dimensions");
    }
    if landmark_ids.iter().any(|&(group, _)| group == 10 || group == 13) {
        return unsupported("View-dependent group 10/13 weighting requires tracker state");
    }
    let (w, h) = (width as f32, height as f32);
    let aspect = [if width > height { w / h } else { 1.0 }, if height > width { h / w } else { 1.0 }];
    let observed = normalized_points.iter().map(|p| [0, 1].map(|i| (1.0 - (p[i] + p[i])) * aspect[i])).collect();
    let sensitivity = landmark_ids
        .iter()
        .flat_map(|id| [if IGNORED_POINTS.contains(id) { IGNORED_SENSITIVITY } else { OBSERVED_SENSITIVITY }; 2])
        .collect();
    Ok((observed, sensitivity))
}

/// The default reset yaw: float32 pi, the model facing the camera.
pub const NEUTRAL_YAW: f64 = PI as f64;

/// The reset pose from four corresponding group-3 landmarks.
///
/// Both point sets are ordered (3,7), (3,11), (3,8), (3,12). Model points are in the
/// caller's already-prepared mesh space; image points are normalized XY.
pub fn initial_rigid_state(
    model_points: &[Vec3; 4],
    image_points: &[[f32; 2]; 4],
    width: i64,
    height: i64,
    focal_factor: f64,
    neutral_yaw: f64,
) -> Result<[f32; 6]> {
    if !model_points.iter().all(|p| finite(p))
        || !finite2(image_points)
        || !(1..=32768).contains(&width)
        || !(1..=32768).contains(&height)
        || !focal_factor.is_finite()
        || focal_factor <= 0.0
        || !neutral_yaw.is_finite()
    {
        return invalid("Expected four finite model/image correspondences and positive camera geometry");
    }
    let (model, image) = (model_points, image_points);
    let aspect = width as f32 / height as f32;
    let ax = if aspect > 1.0 { aspect } else { 1.0 };
    let ay = if aspect < 1.0 { 2.0 / aspect } else { 2.0 };
    let left_x = ax * (image[0][0] + image[1][0]) - ax;
    let right_x = ax * (image[2][0] + image[3][0]) - ax;
    let left_y = (0.5 - (image[0][1] + image[1][1]) * 0.5) * ay;
    let right_y = (0.5 - (image[2][1] + image[3][1]) * 0.5) * ay;
    let (dx, dy) = (left_x - right_x, left_y - right_y);
    let image_distance = (dx * dx + dy * dy).sqrt();
    let delta: [f32; 2] =
        std::array::from_fn(|i| (model[0][i] + model[1][i]) * 0.5 - (model[2][i] + model[3][i]) * 0.5);
    let model_distance = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
    if image_distance == 0.0 || model_distance == 0.0 {
        return invalid("Degenerate reset landmark geometry");
    }
    let scale = model_distance / image_distance;
    Ok([
        0.0,
        neutral_yaw as f32,
        0.0,
        (left_x + right_x) * scale * -0.5,
        (right_y + left_y) * scale * 0.5,
        scale * focal_factor as f32,
    ])
}

/// One original-style update including the bounded shape and animation states.
///
/// Latents are clamped before the model is built, as in the original update. The returned
/// step may exceed that bound until the caller's next decode. Outer retry and validation
/// policy is not supplied here.
#[allow(clippy::too_many_arguments, reason = "mirrors the recovered routine's inputs")]
pub fn deformation_step(
    vertices: &[Vec3],
    observations: &[[f32; 2]],
    state: &[f32],
    focal_factor: f32,
    model_scale: f32,
    blocks: [Block<'_>; 2],
    state_sensitivity: Option<&[f32]>,
    observation_sensitivity: Option<&[f32]>,
    accumulation: Accumulation,
) -> Result<Vec<f32>> {
    let [first, second] = blocks;
    let ns = first.ranges.len();
    let count = 6 + ns + second.ranges.len();
    if state.len() != count || observations.len() != vertices.len() || !finite(state) || !finite2(observations) {
        return invalid("Invalid full fitting state or observations");
    }
    let state_sensitivity = state_sensitivity.map_or_else(|| ones(count), <[f32]>::to_vec);
    let observation_sensitivity = observation_sensitivity.map_or_else(|| ones(2 * observations.len()), <[f32]>::to_vec);
    if state_sensitivity.len() != count
        || observation_sensitivity.len() != 2 * observations.len()
        || !sensitivity_supported(&state_sensitivity, &observation_sensitivity)
    {
        return invalid("Unsupported sensitivity extent/range");
    }
    let mut state = state.to_vec();
    for q in &mut state[6..] {
        *q = q.clamp(-3.0, 3.0);
    }
    let (jacobian, prediction) = model_jacobian(
        vertices,
        [state[0], state[1], state[2]],
        [state[3], state[4], state[5]],
        focal_factor,
        model_scale,
        first.deformation(&state[6..6 + ns]),
        second.deformation(&state[6 + ns..]),
    )?;
    update(&jacobian, &prediction, observations, &state, &state_sensitivity, &observation_sensitivity, accumulation)
}

/// The outer policy's fitting flags: 3 fits pose and shape, 5 pose and animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitMode {
    Shape = 3,
    Animation = 5,
}

impl FitMode {
    pub fn from_flag(flag: i64) -> Result<Self> {
        match flag {
            3 => Ok(Self::Shape),
            5 => Ok(Self::Animation),
            _ => invalid("Expected fitting flag 3 or 5"),
        }
    }

    pub fn flag(self) -> u8 {
        self as u8
    }
}

/// The result of [`mode_step`]: the rigid state, both physical coefficient blocks and the
/// active latent state (rigid plus the active block) to retain for the next update.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeStep {
    pub rigid_state: [f32; 6],
    pub shape_coefficients: Vec<f32>,
    pub animation_coefficients: Vec<f32>,
    pub active_state: Vec<f32>,
}

/// One flag-3 (pose and shape) or flag-5 (pose and animation) update.
///
/// Inactive deformation coefficients stay in physical coefficient space, exactly as stored
/// by the original model. The returned latents belong only to the active block and are
/// clipped by the subsequent decode, as in the outer loop. Pass `active_state` back for
/// consecutive updates in the same mode; re-encoding physical coefficients between those
/// updates can introduce additional rounding.
#[allow(clippy::too_many_arguments, reason = "mirrors the recovered routine's inputs")]
pub fn mode_step(
    model: &PoseModel,
    observations: &[[f32; 2]],
    rigid_state: &[f32; 6],
    shape_coefficients: &[f32],
    animation_coefficients: &[f32],
    mode: FitMode,
    observation_sensitivity: Option<&[f32]>,
    active_state: Option<&[f32]>,
    accumulation: Accumulation,
) -> Result<ModeStep> {
    let (ns, na) = (model.shape_count(), model.animation_count());
    if shape_coefficients.len() != ns
        || animation_coefficients.len() != na
        || observations.len() != model.vertices.len()
        || !finite(rigid_state)
        || !finite(shape_coefficients)
        || !finite(animation_coefficients)
        || !finite2(observations)
    {
        return invalid("Invalid mode-specific state or observations");
    }
    let mut q_shape = coefficient_latents(shape_coefficients, &model.shape_ranges)?;
    let mut q_animation = coefficient_latents(animation_coefficients, &model.animation_ranges)?;
    let (active_block, active): (_, Vec<usize>) = match mode {
        FitMode::Shape => (&mut q_shape, (0..6 + ns).collect()),
        FitMode::Animation => (&mut q_animation, (0..6).chain(6 + ns..6 + ns + na).collect()),
    };
    for q in active_block.iter_mut() {
        *q = q.clamp(-3.0, 3.0);
    }
    let mut pose = *rigid_state;
    if let Some(retained) = active_state {
        if retained.len() != active.len() || !finite(retained) {
            return invalid("Invalid retained active fitting state");
        }
        pose.copy_from_slice(&retained[..6]);
        *active_block = retained[6..].iter().map(|q| q.clamp(-3.0, 3.0)).collect();
    }
    let mut state = pose.to_vec();
    state.extend_from_slice(active_block);
    let (jacobian, prediction) = model_jacobian(
        &model.vertices,
        [pose[0], pose[1], pose[2]],
        [pose[3], pose[4], pose[5]],
        model.focal_factor,
        1.0,
        Deformation {
            basis: &model.shape_basis,
            latent: &q_shape,
            ranges: &model.shape_ranges,
            mask: Some(&model.shape_mask),
            fixed: (mode == FitMode::Animation).then_some(shape_coefficients),
        },
        Deformation {
            basis: &model.animation_basis,
            latent: &q_animation,
            ranges: &model.animation_ranges,
            mask: Some(&model.animation_mask),
            fixed: (mode == FitMode::Shape).then_some(animation_coefficients),
        },
    )?;
    let jacobian = jacobian.columns(&active);
    let state_sensitivity: Vec<f32> = active.iter().map(|&i| model.state_sensitivity[i]).collect();
    let observation_sensitivity = observation_sensitivity.map_or_else(|| ones(2 * observations.len()), <[f32]>::to_vec);
    if observation_sensitivity.len() != 2 * observations.len() || !observation_supported(&observation_sensitivity) {
        return invalid("Unsupported observation sensitivity");
    }
    let mut updated = update(
        &jacobian,
        &prediction,
        observations,
        &state,
        &state_sensitivity,
        &observation_sensitivity,
        accumulation,
    )?;
    let (mut shape, mut animation) = (shape_coefficients.to_vec(), animation_coefficients.to_vec());
    let (target, ranges) = match mode {
        FitMode::Shape => (&mut shape, &model.shape_ranges),
        FitMode::Animation => (&mut animation, &model.animation_ranges),
    };
    let (decoded, clamped) = bounded_coefficients(&updated[6..], ranges)?;
    *target = decoded;
    updated[6..].copy_from_slice(&clamped);
    Ok(ModeStep {
        rigid_state: std::array::from_fn(|i| updated[i]),
        shape_coefficients: shape,
        animation_coefficients: animation,
        active_state: updated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_flip_axes_and_preserve_aspect() {
        let (observed, weights) =
            prepare_observations(&[[0.25, 0.75], [0.5, 0.5]], &[(4, 1), (3, 5)], 640, 480).unwrap();
        assert!((observed[0][0] - 2.0 / 3.0).abs() < 1e-7 && (observed[0][1] + 0.5).abs() < 1e-7);
        assert_eq!(observed[1], [0.0, 0.0]);
        assert_eq!(weights, [0.002, 0.002, 100001.0, 100001.0]);
        assert!(matches!(
            prepare_observations(&[[0.5, 0.5]], &[(10, 1)], 640, 480),
            Err(crate::error::Error::Unsupported(_))
        ));
    }

    #[test]
    fn excluded_point_cannot_change_rigid_step() {
        let state = [0.0, 0.0, 0.0, 0.0, 0.0, 6.0];
        let result =
            rigid_step(&[[1.0, 2.0, 3.0]], &[[999.0, -999.0]], &state, 1.0, 1.0, None, Some(&[100001.0, 0.3])).unwrap();
        assert_eq!(result, state);
        assert!(
            rigid_step(
                &[[1.0, 2.0, 3.0]],
                &[[0.0, 0.0]],
                &[0.0, 0.0, 0.0, 0.0, 0.0, 4.0],
                1.0,
                1.0,
                Some(&[0.0; 6]),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn rigid_step_reduces_synthetic_reprojection_error() {
        let points: Vec<Vec3> = (0..12)
            .map(|i| {
                let f = i as f32;
                [(f * 0.37).sin(), (f * 0.71).cos(), (f * 1.3).sin() * 0.5]
            })
            .collect();
        let state = [0.0, 0.0, 0.0, 0.0, 0.0, 6.0];
        let (observed, _) = project_points(&points, [0.15, -0.2, 0.1], [0.1, -0.2, 5.5], 1.7, 0.8).unwrap();
        let error = |s: &[f32; 6]| -> f32 {
            let (p, _) = project_points(&points, [s[0], s[1], s[2]], [s[3], s[4], s[5]], 1.7, 0.8).unwrap();
            p.iter().zip(&observed).map(|(a, b)| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sum()
        };
        let updated = rigid_step(&points, &observed, &state, 1.7, 0.8, None, None).unwrap();
        assert!(error(&updated) < error(&state));
    }
}
