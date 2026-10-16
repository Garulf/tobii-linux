//! The installed fitting model, prepared as the runtime loader does; no camera capture or
//! outer tracker policy.

use std::path::Path;

use tobii_models::face::{self, FaceMesh, Fdp};

use crate::array::{Basis, Vec3};
use crate::error::{Result, invalid};
use crate::fit::{Accumulation, Block, deformation_step};
use crate::mesh::{TARGET_DISTANCE, dense_deformations, normalize_mesh};

/// An FDP landmark identifier `(group, index)`.
pub type FdpId = (u32, u32);

/// The four group-3 eye-corner landmarks that set the mesh scale and the reset pose.
pub const RESET_POINTS: [FdpId; 4] = [(3, 7), (3, 11), (3, 8), (3, 12)];

fn python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn line_boundary(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// One numeric configuration field, including multiline continuation rows.
///
/// After the key, rows are read while every field parses as a float; `#` starts a comment.
pub fn parse_numeric_config(text: &str, key: &str, count: usize) -> Result<Vec<f64>> {
    let mut values: Option<Vec<f64>> = None;
    for raw in text.split(line_boundary) {
        let mut fields: Vec<&str> =
            raw.split('#').next().unwrap_or("").split(python_space).filter(|f| !f.is_empty()).collect();
        if fields.is_empty() {
            continue;
        }
        if fields[0] == key {
            if values.is_some() {
                return invalid(format!("Duplicate configuration field {key}"));
            }
            values = Some(vec![]);
            fields.remove(0);
        } else if values.is_none() {
            continue;
        }
        let row: Option<Vec<f64>> = fields.iter().map(|f| f.parse().ok()).collect();
        match (row, &mut values) {
            (Some(row), Some(values)) => values.extend(row),
            (None, Some(_)) => break,
            _ => {}
        }
    }
    match values {
        Some(values) if values.len() == count && values.iter().all(|v| v.is_finite()) => Ok(values),
        _ => invalid(format!("Expected {count} finite values for {key}")),
    }
}

/// [`parse_numeric_config`] of a configuration file such as `Head Tracker.cfg`.
pub fn numeric_config(path: &Path, key: &str, count: usize) -> Result<Vec<f64>> {
    parse_numeric_config(&std::fs::read_to_string(path)?, key, count)
}

/// The original float32 log-ratio encoder, restricted to interior coefficients.
pub fn coefficient_latents(coefficients: &[f32], ranges: &[[f32; 2]]) -> Result<Vec<f32>> {
    if ranges.len() != coefficients.len()
        || !coefficients.iter().all(|c| c.is_finite())
        || !ranges.iter().all(|r| r[0].is_finite() && r[1].is_finite())
        || ranges.iter().any(|r| r[0] >= r[1])
    {
        return invalid("Invalid coefficient bounds");
    }
    let units: Vec<f32> = coefficients
        .iter()
        .zip(ranges)
        .map(|(&c, &[low, high])| (c - (high + low) * 0.5) / ((high - low) * 0.5))
        .collect();
    if units.iter().any(|u| u.abs() >= 1.0) {
        return invalid("Coefficient must be strictly inside its bounds");
    }
    Ok(units.iter().map(|&u| (f64::from((u + 1.0) / (1.0 - u)).ln() as f32) * 0.5).collect())
}

/// The prepared fitting model: mapped landmark vertices, masked deformation bases,
/// coefficient bounds, sensitivities and pose limits.
///
/// The state vector is `[rx, ry, rz, tx, ty, tz, shape latents..., animation latents...]`.
#[derive(Debug, Clone, PartialEq)]
pub struct PoseModel {
    pub landmark_ids: Vec<FdpId>,
    pub vertices: Vec<Vec3>,
    pub shape_basis: Basis,
    pub animation_basis: Basis,
    pub shape_ranges: Vec<[f32; 2]>,
    pub animation_ranges: Vec<[f32; 2]>,
    pub shape_mask: Vec<bool>,
    pub animation_mask: Vec<bool>,
    pub state_sensitivity: Vec<f32>,
    pub rotation_limits: [[f32; 2]; 3],
    pub translation_limits: [[f32; 2]; 3],
    pub focal_factor: f32,
}

impl PoseModel {
    pub fn shape_count(&self) -> usize {
        self.shape_ranges.len()
    }

    pub fn animation_count(&self) -> usize {
        self.animation_ranges.len()
    }

    pub fn state_count(&self) -> usize {
        6 + self.shape_count() + self.animation_count()
    }

    /// Position of a landmark in `landmark_ids`.
    pub fn landmark_index(&self, id: FdpId) -> Option<usize> {
        self.landmark_ids.iter().position(|&known| known == id)
    }

    /// The rigid state followed by the latents of zero coefficients.
    pub fn initial_state(&self, rigid_state: &[f32; 6]) -> Result<Vec<f32>> {
        if !rigid_state.iter().all(|v| v.is_finite()) {
            return invalid("Expected six finite rigid state values");
        }
        let mut state = rigid_state.to_vec();
        state.extend(coefficient_latents(&vec![0.0; self.shape_count()], &self.shape_ranges)?);
        state.extend(coefficient_latents(&vec![0.0; self.animation_count()], &self.animation_ranges)?);
        Ok(state)
    }

    /// One full-state update, [`deformation_step`] with this model.
    pub fn step(
        &self,
        observations: &[[f32; 2]],
        state: &[f32],
        observation_sensitivity: Option<&[f32]>,
    ) -> Result<Vec<f32>> {
        self.step_with(observations, state, observation_sensitivity, Accumulation::Double)
    }

    pub fn step_with(
        &self,
        observations: &[[f32; 2]],
        state: &[f32],
        observation_sensitivity: Option<&[f32]>,
        accumulation: Accumulation,
    ) -> Result<Vec<f32>> {
        deformation_step(
            &self.vertices,
            observations,
            state,
            self.focal_factor,
            1.0,
            [
                Block { basis: &self.shape_basis, ranges: &self.shape_ranges, mask: Some(&self.shape_mask) },
                Block {
                    basis: &self.animation_basis,
                    ranges: &self.animation_ranges,
                    mask: Some(&self.animation_mask),
                },
            ],
            Some(&self.state_sensitivity),
            observation_sensitivity,
            accumulation,
        )
    }

    /// The model reduced to the given landmark positions, in that order.
    pub fn select(&self, indices: &[usize]) -> Self {
        Self {
            landmark_ids: indices.iter().map(|&i| self.landmark_ids[i]).collect(),
            vertices: indices.iter().map(|&i| self.vertices[i]).collect(),
            shape_basis: self.shape_basis.select(indices),
            animation_basis: self.animation_basis.select(indices),
            ..self.clone()
        }
    }
}

fn mesh_index(mesh: &FaceMesh, mapping: &Fdp, id: FdpId) -> Result<usize> {
    let Some(record) = mapping.get((i64::from(id.0), i64::from(id.1))) else {
        return invalid(format!("Landmark ({}, {}) has no FDP mapping", id.0, id.1));
    };
    match usize::try_from(record.vertex) {
        Ok(vertex) if record.mesh == 0 && vertex < mesh.vertices.len() => Ok(vertex),
        _ => invalid("Unsupported mesh reference"),
    }
}

fn mask(values: Vec<f32>) -> Result<Vec<bool>> {
    if values.iter().any(|&v| v != 0.0 && v != 1.0) {
        return invalid("Invalid deformation enable mask");
    }
    Ok(values.iter().map(|&v| v != 0.0).collect())
}

fn pairs(values: &[f32]) -> Vec<[f32; 2]> {
    values.as_chunks::<2>().0.to_vec()
}

fn limits(values: &[f32]) -> [[f32; 2]; 3] {
    [[values[0], values[1]], [values[2], values[3]], [values[4], values[5]]]
}

/// Loads the observed jk_300/D1 configuration and selects its 57 mapped vertices.
///
/// Keeps all 40 shape and 23 animation states, zeroing disabled basis columns as observed
/// in the runtime loader. This does not select a live tracker mode.
pub fn load_pose_model(support: &Path) -> Result<PoseModel> {
    let config = std::fs::read_to_string(support.join("Head Tracker.cfg"))?;
    let mesh = FaceMesh::read(&support.join("jk_300.wfm"))?;
    let mapping = Fdp::read(&support.join("jk_300_rigid.fdp"))?;
    let ids = face::read_landmark_ids(&support.join("bdtsdata/NN/vnn_nir/landmarks.bin"))?;
    let (ns, na) = (mesh.shape_units.len(), mesh.animation_units.len());
    let field = |key: &str, count: usize| -> Result<Vec<f32>> {
        Ok(parse_numeric_config(&config, key, count)?.iter().map(|&v| v as f32).collect())
    };
    let shape_mask = mask(field("pose_fitting_su_use", ns)?)?;
    let animation_mask = mask(field("pose_fitting_au_use", na)?)?;
    let (animation, shape) = dense_deformations(&mesh)?;
    let mut reference = [0; 4];
    for (slot, id) in reference.iter_mut().zip(RESET_POINTS) {
        *slot = mesh_index(&mesh, &mapping, id)?;
    }
    let vertices: Vec<Vec3> = mesh.vertices.iter().map(|v| v.map(|c| c as f32)).collect();
    let (vertices, shape, animation) = normalize_mesh(&vertices, &shape, &animation, reference, TARGET_DISTANCE)?;
    let (shape, animation) = (shape.masked(&shape_mask), animation.masked(&animation_mask));
    let selected = ids.iter().map(|&id| mesh_index(&mesh, &mapping, id)).collect::<Result<Vec<_>>>()?;
    let animation_ranges = pairs(&field("action_unit_limit", 2 * na)?);
    let mut sensitivity = field("pose_fitting_pose_sensitivity", 6)?;
    sensitivity.extend(field("pose_fitting_su_sensitivity", ns)?);
    sensitivity.extend(field("pose_fitting_au_sensitivity", na)?);
    if sensitivity.iter().any(|&s| s <= 1e-8) || animation_ranges.iter().any(|r| r[0] >= r[1]) {
        return invalid("Unsupported sensitivity or coefficient range");
    }
    Ok(PoseModel {
        landmark_ids: ids,
        vertices: selected.iter().map(|&i| vertices[i]).collect(),
        shape_basis: shape.select(&selected),
        animation_basis: animation.select(&selected),
        shape_ranges: vec![[-1.0, 1.0]; ns],
        animation_ranges,
        shape_mask,
        animation_mask,
        state_sensitivity: sensitivity,
        rotation_limits: limits(&field("rotation_limit", 6)?),
        translation_limits: limits(&field("translation_limit", 6)?),
        focal_factor: field("camera_focus", 1)?[0],
    })
}

/// Present mapped landmarks in the original numeric FDP scan order.
///
/// Returns the reduced and reordered model and the indices into the input model's points.
/// Identifiers absent from the model are ignored. Presence is supplied by the caller; this
/// does not guess validity from a neural quality score.
pub fn select_pose_landmarks(model: &PoseModel, present: &[FdpId]) -> Result<(PoseModel, Vec<usize>)> {
    let mut selected: Vec<FdpId> = present.iter().copied().filter(|id| model.landmark_ids.contains(id)).collect();
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty() {
        return invalid("No mapped present landmarks");
    }
    let indices: Vec<usize> = selected.iter().map(|&id| model.landmark_index(id).unwrap()).collect();
    Ok((model.select(&indices), indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_config_reads_continuation_rows() {
        let text = "a 1\nsmoothing_factors\n 1.1 0.9 # comment\n 2 3\nnext 4\n";
        assert_eq!(parse_numeric_config(text, "smoothing_factors", 4).unwrap(), [1.1, 0.9, 2.0, 3.0]);
        assert!(parse_numeric_config(text, "smoothing_factors", 3).is_err());
        assert!(parse_numeric_config("k 1\nk 2\n", "k", 2).is_err());
        assert!(parse_numeric_config("k nan\n", "k", 1).is_err());
    }

    #[test]
    fn asymmetric_bounds_encode_physical_zero() {
        let bounds = [[-0.2, 0.7], [-2.0, 1.0]];
        let latent = coefficient_latents(&[0.0, 0.0], &bounds).unwrap();
        assert!(latent.iter().all(|&q| q != 0.0));
        let (decoded, _) = crate::projection::bounded_coefficients(&latent, &bounds).unwrap();
        assert!(decoded.iter().all(|d| d.abs() < 1e-7));
        assert!(coefficient_latents(&[1.0], &[[-1.0, 1.0]]).is_err());
    }
}
