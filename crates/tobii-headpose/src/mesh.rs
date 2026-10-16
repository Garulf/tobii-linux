//! Recovered fitting mesh centering and scaling, without device-unit claims.

use tobii_models::FaceMesh;

use crate::array::{Basis, Vec3};
use crate::error::{Result, invalid};

/// The plaintext sparse animation and shape units as dense `N x 3 x K` bases, in file
/// order: `(animation, shape)`.
pub fn dense_deformations(mesh: &FaceMesh) -> Result<(Basis, Basis)> {
    let points = mesh.vertices.len();
    let dense = |units: &[tobii_models::face::Deformation]| -> Result<Basis> {
        let count = units.len();
        let mut values = vec![0.0f32; points * 3 * count];
        for (k, unit) in units.iter().enumerate() {
            for &(index, offset) in &unit.entries {
                if index >= points {
                    return invalid("Deformation vertex outside the mesh");
                }
                for (axis, &value) in offset.iter().enumerate() {
                    values[(index * 3 + axis) * count + k] = value as f32;
                }
            }
        }
        Ok(Basis::new(points, count, values).unwrap())
    };
    Ok((dense(&mesh.animation_units)?, dense(&mesh.shape_units)?))
}

/// The observed reference distance of the runtime constructors; units are not inferred.
pub const TARGET_DISTANCE: f64 = 0.065;

/// Centers four mapped landmarks and scales their XY midpoint separation.
///
/// Indices are ordered FDP (3,7), (3,11), (3,8), (3,12). Returns the vertices and both
/// bases scaled by the same factor.
pub fn normalize_mesh(
    vertices: &[Vec3],
    first: &Basis,
    second: &Basis,
    landmark_vertices: [usize; 4],
    target_distance: f64,
) -> Result<(Vec<Vec3>, Basis, Basis)> {
    if landmark_vertices.iter().any(|&i| i >= vertices.len())
        || !target_distance.is_finite()
        || target_distance <= 0.0
        || !vertices.iter().all(|v| v.iter().all(|c| c.is_finite()))
    {
        return invalid("Invalid model vertices, four indices or scale target");
    }
    for basis in [first, second] {
        if basis.points() != vertices.len() || !basis.is_finite() {
            return invalid("Invalid deformation basis");
        }
    }
    let [a, b, c, d] = landmark_vertices.map(|i| vertices[i]);
    let left = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let right = [(c[0] + d[0]) * 0.5, (c[1] + d[1]) * 0.5];
    let delta = [left[0] - right[0], left[1] - right[1]];
    let distance = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
    if distance == 0.0 {
        return invalid("Degenerate reference landmark distance");
    }
    let scale = target_distance as f32 / distance;
    let center = [(right[0] + left[0]) * 0.5, (right[1] + left[1]) * 0.5, (((c[2] + d[2]) + a[2]) + b[2]) * 0.25];
    let moved = vertices.iter().map(|v| [0, 1, 2].map(|i| (v[i] - center[i]) * scale)).collect();
    Ok((moved, first.map(|v| v * scale), second.map(|v| v * scale)))
}

/// The subsequent runtime distance change, 0x140052bc0.
pub fn rescale_mesh(
    vertices: &[Vec3],
    first: &Basis,
    second: &Basis,
    previous_distance: f64,
    target_distance: f64,
) -> Result<(Vec<Vec3>, Basis, Basis)> {
    if ![previous_distance, target_distance].iter().all(|x| x.is_finite() && *x > 0.0) {
        return invalid("Distances must be finite and positive");
    }
    let factor = target_distance as f32 / previous_distance as f32;
    if !vertices.iter().all(|v| v.iter().all(|c| c.is_finite())) || !first.is_finite() || !second.is_finite() {
        return invalid("Non-finite model data");
    }
    let moved = vertices.iter().map(|v| v.map(|c| c * factor)).collect();
    Ok((moved, first.map(|v| v * factor), second.map(|v| v * factor)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_centers_and_scales_reference_points() {
        let vertices = [[0.0, 0.0, 1.0], [0.0, 2.0, 3.0], [4.0, 0.0, 5.0], [4.0, 2.0, 7.0]];
        let basis = Basis::new(4, 1, vec![1.0; 12]).unwrap();
        let (moved, first, _) = normalize_mesh(&vertices, &basis, &basis, [0, 1, 2, 3], 0.065).unwrap();
        let scale = 0.065f32 / 4.0;
        assert_eq!(moved[0], [-2.0 * scale, -scale, -3.0 * scale]);
        assert_eq!(first.values()[0], scale);
        assert!(normalize_mesh(&vertices, &basis, &basis, [0, 0, 0, 0], 0.065).is_err());
        assert!(normalize_mesh(&vertices, &basis, &basis, [0, 1, 2, 9], 0.065).is_err());
    }

    #[test]
    fn rescale_rejects_nonpositive_distance() {
        let basis = Basis::zeros(1, 0);
        assert!(rescale_mesh(&[[1.0, 2.0, 3.0]], &basis, &basis, 0.0, 1.0).is_err());
        let (moved, _, _) = rescale_mesh(&[[1.0, 2.0, 3.0]], &basis, &basis, 0.5, 1.0).unwrap();
        assert_eq!(moved, [[2.0, 4.0, 6.0]]);
    }
}
