//! Recovered face-model projection primitive, its Jacobians and the bounded coefficients.
//!
//! Coordinates remain in the fitting model's local convention. Camera calibration,
//! parameter optimization and public head-pose coordinate conversion are separate.

use crate::array::{Basis, Matrix, Vec3};
use crate::error::{Result, invalid};

/// float32 sines and cosines of the three angles, each from the libm double result.
#[derive(Debug, Clone, Copy)]
struct Trig {
    sx: f32,
    sy: f32,
    sz: f32,
    cx: f32,
    cy: f32,
    cz: f32,
}

impl Trig {
    fn new(angles: Vec3) -> Self {
        let sin = |a: f32| f64::from(a).sin() as f32;
        let cos = |a: f32| f64::from(a).cos() as f32;
        let [x, y, z] = angles;
        Self { sx: sin(x), sy: sin(y), sz: sin(z), cx: cos(x), cy: cos(y), cz: cos(z) }
    }

    /// `Ry(ry) @ Rx(rx) @ Rz(rz)` with the original's float32 products.
    fn matrix(&self) -> [[f32; 3]; 3] {
        let Self { sx, sy, sz, cx, cy, cz } = *self;
        let (sy_sx, cy_sx) = (sy * sx, cy * sx);
        [
            [sy_sx * sz + cz * cy, sy_sx * cz - cy * sz, cx * sy],
            [cx * sz, cz * cx, -sx],
            [cy_sx * sz - cz * sy, cy_sx * cz + sz * sy, cy * cx],
        ]
    }
}

fn finite(values: &[f32]) -> bool {
    values.iter().all(|v| v.is_finite())
}

fn finite3(points: &[Vec3]) -> bool {
    points.iter().all(|p| finite(p))
}

/// Projects finite model points with `Ry(ry) @ Rx(rx) @ Rz(rz)`; angles are radians.
///
/// Rotation and translation apply before the perspective division; no principal-point
/// offset is added in this observed primitive. Returns `(projected, transformed)`. Inputs
/// with zero depth are rejected.
pub fn project_points(
    vertices: &[Vec3],
    rotation: Vec3,
    translation: Vec3,
    focal_factor: f32,
    model_scale: f32,
) -> Result<(Vec<[f32; 2]>, Vec<Vec3>)> {
    if !finite3(vertices)
        || !finite(&rotation)
        || !finite(&translation)
        || !focal_factor.is_finite()
        || !model_scale.is_finite()
    {
        return invalid("Expected finite Nx3 vertices and three rotation/translation values");
    }
    let Trig { sx, sy, sz, cx, cy, cz } = Trig::new(rotation);
    let (sy_sx, cy_sx) = (sy * sx, cy * sx);
    let (x_y, x_x, x_z) = (sy_sx * cz - cy * sz, sy_sx * sz + cz * cy, cx * sy);
    let (y_y, y_x) = (cz * cx, cx * sz);
    let (z_y, z_x) = (cy_sx * cz + sz * sy, cy_sx * sz - cz * sy);
    let transformed: Vec<Vec3> = vertices
        .iter()
        .map(|&[x, y, z]| {
            let px = (y * x_y + x * x_x + z * x_z) * model_scale + translation[0];
            let py = ((y * y_y + x * y_x) - z * sx) * model_scale + translation[1];
            let pz = (y * z_y + x * z_x + z * cy * cx) * model_scale + translation[2];
            [px, py, pz]
        })
        .collect();
    if transformed.iter().any(|p| p[2] == 0.0) {
        return invalid("Zero projection depth");
    }
    let projected: Vec<[f32; 2]> = transformed
        .iter()
        .map(|&[px, py, pz]| {
            let factor = (1.0 / pz) * focal_factor;
            [factor * px, factor * py]
        })
        .collect();
    if !projected.iter().all(|p| finite(p)) || !finite3(&transformed) {
        return invalid("Non-finite projection result");
    }
    Ok((projected, transformed))
}

/// Decodes fitting latents with `tanh(clamp(q, -3, 3)) * half_range + midpoint`.
///
/// Returns the decoded coefficients and the clamped latents. This does not assign a
/// particular deformation block to animation or shape configuration.
pub fn bounded_coefficients(latent: &[f32], ranges: &[[f32; 2]]) -> Result<(Vec<f32>, Vec<f32>)> {
    if ranges.len() != latent.len()
        || !finite(latent)
        || !ranges.iter().all(|r| finite(r))
        || ranges.iter().any(|r| r[1] <= r[0])
    {
        return invalid("Expected finite latent vector and increasing bounds");
    }
    let clamped: Vec<f32> = latent.iter().map(|&q| q.clamp(-3.0, 3.0)).collect();
    let values = clamped
        .iter()
        .zip(ranges)
        .map(|(&q, &[low, high])| {
            let half = (high - low) * 0.5;
            let middle = (high + low) * 0.5;
            (f64::from(q).tanh() as f32) * half + middle
        })
        .collect();
    Ok((values, clamped))
}

/// Adds two deformation bases in the observed first-then-second order.
///
/// Each step sums `basis @ coefficients + vertices` in double precision and rounds the
/// result to float32, as the runtime's matrix product does.
pub fn deform_vertices(
    vertices: &[Vec3],
    first: &Basis,
    first_coefficients: &[f32],
    second: &Basis,
    second_coefficients: &[f32],
) -> Result<Vec<Vec3>> {
    if !finite3(vertices) {
        return invalid("Expected finite Nx3 vertices");
    }
    let mut result = vertices.to_vec();
    for (basis, coefficients) in [(first, first_coefficients), (second, second_coefficients)] {
        if basis.points() != vertices.len()
            || basis.count() != coefficients.len()
            || !basis.is_finite()
            || !finite(coefficients)
        {
            return invalid("Inconsistent/non-finite deformation basis");
        }
        for (point, value) in result.iter_mut().enumerate() {
            for (axis, coordinate) in value.iter_mut().enumerate() {
                let sum = dot64(basis.components(point, axis), coefficients);
                *coordinate = (sum + f64::from(*coordinate)) as f32;
            }
        }
    }
    Ok(result)
}

/// Sequential double-precision dot product of float32 values; every product is exact.
pub(crate) fn dot64(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).fold(0.0, |total, (&x, &y)| total + f64::from(x) * f64::from(y))
}

/// Analytic `2N x 6` projection Jacobian for the six enabled rigid pose states.
///
/// Rows alternate x and y per point. The scalar float32 derivative order of the original
/// is kept, since reassociation is visible after inverting the configured 69-state normal
/// matrix. Deformation-state columns and observation gating are not included.
pub fn rigid_jacobian(
    vertices: &[Vec3],
    rotation: Vec3,
    translation: Vec3,
    focal_factor: f32,
    model_scale: f32,
) -> Result<Matrix> {
    let (_, camera) = project_points(vertices, rotation, translation, focal_factor, model_scale)?;
    let Trig { sx, sy, sz, cx, cy, cz } = Trig::new(rotation);
    let (sy_sx, cy_sx) = (sy * sx, cy * sx);
    let r00 = sy_sx * sz + cz * cy;
    let r01 = sy_sx * cz - cy * sz;
    let r02 = cx * sy;
    let r20 = cy_sx * sz - cz * sy;
    let r21 = cy_sx * cz + sz * sy;
    let r22 = cy * cx;
    let mut result = Matrix::zeros(2 * vertices.len(), 6);
    for (n, (&[x, y, z], &[px, py, pz])) in vertices.iter().zip(&camera).enumerate() {
        let inverse_depth = 1.0 / pz;
        let translation_factor = inverse_depth * focal_factor;
        let depth_factor = inverse_depth * translation_factor;
        let factor = depth_factor * model_scale;
        let dz_rx = (y * (r22 * cz) + x * (r22 * sz)) - z * cy_sx;
        let dz_ry = (x * (-r00) - y * r01) - z * r02;
        let dz_rz = x * r21 - y * r20;
        let row_x = [
            (((y * (r02 * cz) + x * (r02 * sz)) - z * sy_sx) * pz - dz_rx * px) * factor,
            ((y * r21 + x * r20 + z * r22) * pz - dz_ry * px) * factor,
            ((y * (-r00) + x * r01) * pz - dz_rz * px) * factor,
            translation_factor,
            0.0,
            -(depth_factor * px),
        ];
        let row_y = [
            (-((x * (sz * sx) + y * (cz * sx) + z * cx) * pz) - dz_rx * py) * factor,
            -(dz_ry * py * factor),
            ((x * (cz * cx) - y * (cx * sz)) * pz - dz_rz * py) * factor,
            0.0,
            translation_factor,
            -(depth_factor * py),
        ];
        result.values[12 * n..12 * n + 6].copy_from_slice(&row_x);
        result.values[12 * n + 6..12 * n + 12].copy_from_slice(&row_y);
    }
    Ok(result)
}

/// One deformation block of the full model: its basis, latent state and bounds, the
/// enabled components (all when `None`) and, optionally, physical coefficients that
/// replace the decoded latents.
#[derive(Debug, Clone, Copy)]
pub struct Deformation<'a> {
    pub basis: &'a Basis,
    pub latent: &'a [f32],
    pub ranges: &'a [[f32; 2]],
    pub mask: Option<&'a [bool]>,
    pub fixed: Option<&'a [f32]>,
}

/// The original full model's update matrix and prediction: `(2N x (6 + K1 + K2), N x 2)`.
///
/// The installed implementation transforms each basis component with
/// `(1 - tanh(b)^2) * tanh(b) * half_range`. This is deliberately preserved; it is not
/// replaced with the usual derivative with respect to the current latent state.
pub fn model_jacobian(
    vertices: &[Vec3],
    rotation: Vec3,
    translation: Vec3,
    focal_factor: f32,
    model_scale: f32,
    first: Deformation<'_>,
    second: Deformation<'_>,
) -> Result<(Matrix, Vec<[f32; 2]>)> {
    let decode = |block: &Deformation<'_>| -> Result<Vec<f32>> {
        let (decoded, _) = bounded_coefficients(block.latent, block.ranges)?;
        Ok(block.fixed.map_or(decoded, <[f32]>::to_vec))
    };
    let (c1, c2) = (decode(&first)?, decode(&second)?);
    let deformed = deform_vertices(vertices, first.basis, &c1, second.basis, &c2)?;
    let (prediction, camera) = project_points(&deformed, rotation, translation, focal_factor, model_scale)?;
    let rigid = rigid_jacobian(&deformed, rotation, translation, focal_factor, model_scale)?;
    let r = Trig::new(rotation).matrix();
    let rows = 2 * vertices.len();
    let mut blocks = vec![rigid];
    for block in [first, second] {
        let basis = block.basis;
        let count = basis.count();
        let enabled = match block.mask {
            Some(mask) if mask.len() != count => return invalid("Deformation mask extent mismatch"),
            Some(mask) => mask.to_vec(),
            None => vec![true; count],
        };
        if block.ranges.len() != count {
            return invalid("Deformation range extent mismatch");
        }
        let half: Vec<f32> = block.ranges.iter().map(|&[low, high]| (high - low) * 0.5).collect();
        let derivative = |point: usize, axis: usize, k: usize| -> f32 {
            if !enabled[k] {
                return 0.0;
            }
            let t = basis.tanh()[(point * 3 + axis) * count + k];
            ((1.0 - t * t) * t) * half[k]
        };
        let mut jacobian = Matrix::zeros(rows, count);
        for (n, &[px, py, pz]) in camera.iter().enumerate() {
            let inverse_depth = 1.0 / pz;
            let factor = (inverse_depth * (inverse_depth * focal_factor)) * model_scale;
            for k in 0..count {
                let (x, y, z) = (derivative(n, 0, k), derivative(n, 1, k), derivative(n, 2, k));
                let dz = y * r[2][1] + x * r[2][0] + z * r[2][2];
                let dx = y * r[0][1] + x * r[0][0] + z * r[0][2];
                let dy = y * r[1][1] + x * r[1][0] + z * r[1][2];
                jacobian.set(2 * n, k, (dx * pz - dz * px) * factor);
                jacobian.set(2 * n + 1, k, (dy * pz - dz * py) * factor);
            }
        }
        blocks.push(jacobian);
    }
    let cols = blocks.iter().map(|b| b.cols).sum();
    let values = (0..rows).flat_map(|r| blocks.iter().flat_map(move |b| b.row(r).to_vec())).collect();
    Ok((Matrix { rows, cols, values }, prediction))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_perspective_and_scale() {
        let (projected, camera) = project_points(&[[1.0, 2.0, 3.0]], [0.0; 3], [0.0, 0.0, 4.0], 2.0, 2.0).unwrap();
        assert_eq!(camera, [[2.0, 4.0, 10.0]]);
        assert_eq!(projected, [[0.4, 0.8]]);
    }

    #[test]
    fn rotation_order_on_basis_vector() {
        let quarter = std::f32::consts::FRAC_PI_2;
        let (_, camera) = project_points(&[[1.0, 0.0, 0.0]], [quarter; 3], [0.0, 0.0, 4.0], 1.0, 1.0).unwrap();
        for (got, want) in camera[0].iter().zip([1.0, 0.0, 4.0]) {
            assert!((got - want).abs() < 1e-6);
        }
    }

    #[test]
    fn zero_depth_rejected() {
        assert!(project_points(&[[1.0, 2.0, 0.0]], [0.0; 3], [0.0; 3], 1.0, 1.0).is_err());
    }

    #[test]
    fn clamp_preserves_bounded_coefficients() {
        let (coefficients, latent) = bounded_coefficients(&[-10.0, 0.0, 10.0], &[[-2.0, 4.0]; 3]).unwrap();
        assert_eq!(latent, [-3.0, 0.0, 3.0]);
        assert_eq!(coefficients[1], 1.0);
        assert!(coefficients.iter().all(|&c| c > -2.0 && c < 4.0));
        assert!(bounded_coefficients(&[0.0], &[[1.0, 1.0]]).is_err());
    }

    #[test]
    fn deformation_adds_bases_in_double_precision() {
        let first = Basis::new(1, 2, vec![1.0, 2.0, 0.0, 0.0, 0.5, 0.25]).unwrap();
        let second = Basis::new(1, 1, vec![0.0, 1.0, 0.0]).unwrap();
        let deformed = deform_vertices(&[[1.0, 1.0, 1.0]], &first, &[2.0, 3.0], &second, &[4.0]).unwrap();
        assert_eq!(deformed, [[9.0, 5.0, 2.75]]);
        assert!(deform_vertices(&[[1.0, 1.0, 1.0]], &first, &[2.0], &second, &[4.0]).is_err());
    }
}
