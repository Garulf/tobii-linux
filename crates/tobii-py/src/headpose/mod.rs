//! tobii_linux head-pose modules: one submodule per Python module, over tobii-headpose.
//!
//! float32 arrays cross the boundary as little-endian bytes in numpy's C order; the Python
//! wrappers rebuild the arrays and keep the dataclasses.

use pyo3::exceptions::{PyNotImplementedError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::{Basis, Error, Matrix, PoseModel};

mod eye_refine;
mod head_pose_pipeline;
mod landmark_smoothing;
mod lbf_features;
mod lbf_predictor;
mod pose_fit;
mod pose_geometry;
mod pose_linalg;
mod pose_mesh;
mod pose_model;
mod pose_output;
mod pose_policy;
mod pose_projection;
mod pose_temporal;

pub fn error(error: Error) -> PyErr {
    match error {
        Error::Value(message) => PyValueError::new_err(message),
        Error::Unsupported(message) => PyNotImplementedError::new_err(message),
        Error::Models(error) => crate::models::error(error),
        Error::Io(error) => error.into(),
    }
}

pub trait HeadPoseResult<T> {
    fn py(self) -> PyResult<T>;
}

impl<T> HeadPoseResult<T> for Result<T, Error> {
    fn py(self) -> PyResult<T> {
        self.map_err(error)
    }
}

pub fn invalid(message: &str) -> PyErr {
    PyValueError::new_err(message.to_string())
}

/// float32 values from little-endian bytes.
pub fn floats(data: &[u8]) -> PyResult<Vec<f32>> {
    let (chunks, rest) = data.as_chunks::<4>();
    if !rest.is_empty() {
        return Err(invalid("float32 data must be a multiple of four bytes"));
    }
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

/// `N`-wide float32 rows from little-endian bytes.
pub fn rows<const N: usize>(data: &[u8]) -> PyResult<Vec<[f32; N]>> {
    let values = floats(data)?;
    if values.len() % N != 0 {
        return Err(invalid("float32 data does not fill whole rows"));
    }
    Ok(values.as_chunks::<N>().0.to_vec())
}

pub fn fixed<const N: usize>(data: &[u8]) -> PyResult<[f32; N]> {
    floats(data)?.try_into().map_err(|_| invalid(&format!("Expected {N} float32 values")))
}

pub fn bytes<'py>(py: Python<'py>, values: &[f32]) -> Bound<'py, PyBytes> {
    PyBytes::new(py, &values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>())
}

pub fn flat_bytes<'py, const N: usize>(py: Python<'py>, rows: &[[f32; N]]) -> Bound<'py, PyBytes> {
    bytes(py, rows.as_flattened())
}

pub fn matrix_bytes<'py>(py: Python<'py>, matrix: &Matrix) -> Bound<'py, PyBytes> {
    bytes(py, &matrix.values)
}

/// A basis whose point count is known, so an empty `K` keeps its points.
pub fn basis_of(data: &[u8], points: usize, count: usize) -> PyResult<Basis> {
    Basis::new(points, count, floats(data)?).ok_or_else(|| invalid("Inconsistent/non-finite deformation basis"))
}

/// PreparedPoseModel fields as `pose_model._native_model` packs them:
/// `(ids, (vertices, shape basis, K1, animation basis, K2), (shape ranges, animation ranges,
/// shape mask, animation mask), (sensitivity, rotation limits, translation limits, focal))`.
type ModelFields<'a> = (
    Vec<(u32, u32)>,
    (&'a [u8], &'a [u8], usize, &'a [u8], usize),
    (&'a [u8], &'a [u8], Vec<bool>, Vec<bool>),
    (&'a [u8], &'a [u8], &'a [u8], f64),
);

pub fn pose_model(fields: &Bound<'_, PyAny>) -> PyResult<PoseModel> {
    let (ids, (vertices, shape, ns, animation, na), (shape_ranges, animation_ranges, shape_mask, animation_mask), tail): ModelFields<'_> =
        fields.extract()?;
    let (sensitivity, rotation, translation, focal) = tail;
    let vertices = rows::<3>(vertices)?;
    let points = vertices.len();
    let limits = |data: &[u8]| -> PyResult<[[f32; 2]; 3]> {
        let pairs = rows::<2>(data)?;
        pairs.try_into().map_err(|_| invalid("Limits must be 3x2"))
    };
    Ok(PoseModel {
        landmark_ids: ids,
        vertices,
        shape_basis: basis_of(shape, points, ns)?,
        animation_basis: basis_of(animation, points, na)?,
        shape_ranges: rows::<2>(shape_ranges)?,
        animation_ranges: rows::<2>(animation_ranges)?,
        shape_mask,
        animation_mask,
        state_sensitivity: floats(sensitivity)?,
        rotation_limits: limits(rotation)?,
        translation_limits: limits(translation)?,
        focal_factor: focal as f32,
    })
}

pub fn model_tuple<'py>(py: Python<'py>, model: &PoseModel) -> PyResult<Bound<'py, PyAny>> {
    (
        model.landmark_ids.clone(),
        (
            flat_bytes(py, &model.vertices),
            bytes(py, model.shape_basis.values()),
            model.shape_basis.count(),
            bytes(py, model.animation_basis.values()),
            model.animation_basis.count(),
        ),
        (
            flat_bytes(py, &model.shape_ranges),
            flat_bytes(py, &model.animation_ranges),
            model.shape_mask.clone(),
            model.animation_mask.clone(),
        ),
        (
            bytes(py, &model.state_sensitivity),
            flat_bytes(py, &model.rotation_limits),
            flat_bytes(py, &model.translation_limits),
            f64::from(model.focal_factor),
        ),
    )
        .into_pyobject(py)
        .map(|t| t.into_any())
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    crate::submodule(m, "pose_linalg", pose_linalg::register)?;
    crate::submodule(m, "pose_projection", pose_projection::register)?;
    crate::submodule(m, "pose_mesh", pose_mesh::register)?;
    crate::submodule(m, "pose_geometry", pose_geometry::register)?;
    crate::submodule(m, "pose_output", pose_output::register)?;
    crate::submodule(m, "pose_fit", pose_fit::register)?;
    crate::submodule(m, "pose_model", pose_model::register)?;
    crate::submodule(m, "pose_policy", pose_policy::register)?;
    crate::submodule(m, "pose_temporal", pose_temporal::register)?;
    crate::submodule(m, "lbf_features", lbf_features::register)?;
    crate::submodule(m, "lbf_predictor", lbf_predictor::register)?;
    crate::submodule(m, "eye_refine", eye_refine::register)?;
    crate::submodule(m, "landmark_smoothing", landmark_smoothing::register)?;
    crate::submodule(m, "head_pose_pipeline", head_pose_pipeline::register)
}
