//! tobii_linux.pose_linalg: the float32 inverse.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::Matrix;
use tobii_headpose::linalg::inverse_f32;

use super::{HeadPoseResult, floats, invalid, matrix_bytes};

/// The inverse of an `order x order` float32 matrix given as bytes, as bytes.
#[pyfunction]
fn inverse_float32<'py>(py: Python<'py>, matrix: &[u8], order: usize) -> PyResult<Bound<'py, PyBytes>> {
    let matrix = Matrix::from_values(order, order, floats(matrix)?)
        .ok_or_else(|| invalid("Expected a finite square matrix of order at least four"))?;
    Ok(matrix_bytes(py, &inverse_f32(&matrix).py()?))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(inverse_float32, m)?)
}
