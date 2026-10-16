//! The fitted jk_300 face mesh as a wireframe: in the camera image and in display coordinates.
//!
//! The mesh, its normalisation and its deformation bases are prepared exactly as
//! `load_pose_model` prepares them, but for all 357 vertices instead of the 57 landmark
//! vertices the fit uses. A fit result (rigid state plus shape and animation coefficients)
//! deforms and projects it the way the fitter does (`deform_vertices` and `project_points`
//! at model scale 1). The display placement multiplies float32 matrices in plain row order;
//! the Python module used NumPy's matrix products, so millimetre positions can differ in
//! the last float32 bits, below the browser's 0.1 mm resolution.

use std::collections::HashSet;
use std::path::Path;

use tobii_headpose::array::{Basis, Vec3};
use tobii_headpose::geometry::{Matrix4, geometry_config, y_rotation};
use tobii_headpose::mesh::{TARGET_DISTANCE, dense_deformations, normalize_mesh};
use tobii_headpose::output::{MILLIMETRES_PER_METRE, Matrix3, PI, euler_matrix};
use tobii_headpose::pipeline::{DisplayArea, HeadPoseOutput, HeadPoseSample};
use tobii_headpose::projection::{deform_vertices, project_points};
use tobii_headpose::{PoseModel, Result, TemporalFit};
use tobii_models::{FaceMesh, Fdp};

pub const NORMALIZATION_POINTS: [(i64, i64); 4] = [(3, 7), (3, 11), (3, 8), (3, 12)];

/// Each undirected triangle edge once, in first-seen order.
pub fn triangle_edges(triangles: &[[usize; 3]]) -> Vec<(usize, usize)> {
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for &[a, b, c] in triangles {
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let key = (p.min(q), p.max(q));
            if seen.insert(key) {
                edges.push(key);
            }
        }
    }
    edges
}

/// Normalised mesh, deformation bases and edges; `landmark_indices` are the vertices of the
/// fitted landmarks.
#[derive(Debug, Clone)]
pub struct FaceWireframe {
    pub vertices: Vec<Vec3>,
    pub shape_basis: Basis,
    pub animation_basis: Basis,
    pub edges: Vec<(usize, usize)>,
    pub focal_factor: f32,
    pub landmark_indices: Vec<usize>,
}

fn vertex(mapping: &Fdp, id: (i64, i64)) -> Result<usize> {
    let record = mapping
        .get(id)
        .ok_or_else(|| tobii_headpose::Error::Value(format!("Landmark ({}, {}) has no FDP mapping", id.0, id.1)))?;
    usize::try_from(record.vertex).map_err(|_| tobii_headpose::Error::Value("Unsupported mesh reference".into()))
}

fn product3(left: &Matrix3, right: &Matrix3) -> Matrix3 {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| left[i][0] * right[0][j] + left[i][1] * right[1][j] + left[i][2] * right[2][j])
    })
}

fn product4(left: &Matrix4, right: &Matrix4) -> Matrix4 {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            left[i][0] * right[0][j] + left[i][1] * right[1][j] + left[i][2] * right[2][j] + left[i][3] * right[3][j]
        })
    })
}

fn transpose(matrix: &Matrix3) -> Matrix3 {
    std::array::from_fn(|i| std::array::from_fn(|j| matrix[j][i]))
}

/// Row-vector matrix whose `matrix_euler` gives `angles` before the geometry stage's sign flips.
fn display_rotation(angles: Vec3) -> Matrix3 {
    let [a, b, c] = angles;
    euler_matrix([-a, b, -c])
}

impl FaceWireframe {
    /// The mesh of `support`; masks and focal factor come from its loaded pose model.
    pub fn load(support: &Path, model: &PoseModel) -> Result<Self> {
        let mesh = FaceMesh::read(&support.join("jk_300.wfm"))?;
        let mapping = Fdp::read(&support.join("jk_300_rigid.fdp"))?;
        let (animation, shape) = dense_deformations(&mesh)?;
        let reference = [
            vertex(&mapping, NORMALIZATION_POINTS[0])?,
            vertex(&mapping, NORMALIZATION_POINTS[1])?,
            vertex(&mapping, NORMALIZATION_POINTS[2])?,
            vertex(&mapping, NORMALIZATION_POINTS[3])?,
        ];
        let vertices: Vec<Vec3> = mesh.vertices.iter().map(|v| v.map(|c| c as f32)).collect();
        let (vertices, shape, animation) = normalize_mesh(&vertices, &shape, &animation, reference, TARGET_DISTANCE)?;
        let landmark_indices = model
            .landmark_ids
            .iter()
            .map(|&(group, index)| vertex(&mapping, (i64::from(group), i64::from(index))))
            .collect::<Result<_>>()?;
        Ok(Self {
            vertices,
            shape_basis: shape.masked(&model.shape_mask),
            animation_basis: animation.masked(&model.animation_mask),
            edges: triangle_edges(&mesh.triangles),
            focal_factor: model.focal_factor,
            landmark_indices,
        })
    }

    pub fn deformed(&self, fit: &TemporalFit) -> Result<Vec<Vec3>> {
        deform_vertices(
            &self.vertices,
            &self.shape_basis,
            &fit.shape_coefficients,
            &self.animation_basis,
            &fit.animation_coefficients,
        )
    }

    /// (fitting-plane points, camera-space points in metres) of the deformed mesh at the fitted pose.
    pub fn camera_points(&self, fit: &TemporalFit) -> Result<(Vec<[f32; 2]>, Vec<Vec3>)> {
        let rigid = fit.rigid_state;
        project_points(
            &self.deformed(fit)?,
            [rigid[0], rigid[1], rigid[2]],
            [rigid[3], rigid[4], rigid[5]],
            self.focal_factor,
            1.0,
        )
    }

    /// Pixel coordinates, Y down, inverting the fitter's observed = (1 - 2 p) * aspect.
    pub fn image_points(&self, fit: &TemporalFit, width: usize, height: usize) -> Result<Vec<[f32; 2]>> {
        let (plane, _) = self.camera_points(fit)?;
        let (w, h) = (width as f64, height as f64);
        let aspect = [if w > h { (w / h) as f32 } else { 1.0 }, if h > w { (h / w) as f32 } else { 1.0 }];
        let size = [width as f32, height as f32];
        Ok(plane.iter().map(|p| std::array::from_fn(|i| (1.0 - p[i] / aspect[i]) * 0.5 * size[i])).collect())
    }

    /// Mesh in display millimetres (display-centred: right, up, normal) at `sample`'s filtered pose.
    ///
    /// `output` is the HeadPoseOutput that produced `sample` from this fit. The mesh is
    /// placed with the geometry stage's position transform, then turned and moved from the
    /// unfiltered pose to the filtered one.
    pub fn display_points(
        &self,
        fit: &TemporalFit,
        output: &HeadPoseOutput,
        sample: &HeadPoseSample,
    ) -> Result<Vec<Vec3>> {
        let (_, camera) = self.camera_points(fit)?;
        let combined = product4(&product4(&geometry_config().1, &output.camera), &output.display);
        let (rotation, position) = output.unfiltered(&fit.rigid_state);
        let turn = product3(&transpose(&display_rotation(rotation)), &display_rotation(sample.rotation_rad));
        let flip = y_rotation(PI);
        let turned = product3(&product3(&flip, &turn), &flip);
        Ok(camera
            .iter()
            .map(|point| {
                let row = [
                    point[0] * MILLIMETRES_PER_METRE,
                    point[1] * MILLIMETRES_PER_METRE,
                    point[2] * MILLIMETRES_PER_METRE,
                    1.0,
                ];
                let placed: [f32; 4] = std::array::from_fn(|j| (0..4).map(|k| row[k] * combined[k][j]).sum());
                let moved: Vec3 = std::array::from_fn(|i| placed[i] / placed[3] - position[i]);
                std::array::from_fn(|j| (0..3).map(|k| moved[k] * turned[k][j]).sum::<f32>() + sample.position_mm[j])
            })
            .collect())
    }
}

/// Display corners and the tracker origin in the display-centred axes head pose is reported in.
pub fn display_scene(area: &DisplayArea, display: &Matrix4) -> ([[f32; 3]; 4], [f32; 3]) {
    let (tl, tr, bl) = (area.top_left, area.top_right, area.bottom_left);
    let fourth: Vec3 = std::array::from_fn(|i| tr[i] + bl[i] - tl[i]);
    let place = |point: Vec3| -> [f32; 3] {
        let row = [point[0], point[1], point[2], 1.0];
        std::array::from_fn(|j| (0..4).map(|k| row[k] * display[k][j]).sum())
    };
    ([place(tl), place(tr), place(fourth), place(bl)], place([0.0; 3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_unique_and_ordered() {
        assert_eq!(triangle_edges(&[[0, 1, 2], [2, 1, 3]]), [(0, 1), (1, 2), (0, 2), (1, 3), (2, 3)]);
    }
}
