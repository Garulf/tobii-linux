//! tobii_linux.pose_projection: projection, bounded coefficients, deformation and Jacobians.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::projection::{self, Deformation};

use super::{HeadPoseResult, basis_of, bytes, fixed, flat_bytes, floats, matrix_bytes, rows};

type Pair<'py> = (Bound<'py, PyBytes>, Bound<'py, PyBytes>);

/// `(projected, transformed)` as bytes.
#[pyfunction]
fn project_points<'py>(
    py: Python<'py>,
    vertices: &[u8],
    rotation: &[u8],
    translation: &[u8],
    focal_factor: f64,
    model_scale: f64,
) -> PyResult<Pair<'py>> {
    let (projected, transformed) = projection::project_points(
        &rows::<3>(vertices)?,
        fixed(rotation)?,
        fixed(translation)?,
        focal_factor as f32,
        model_scale as f32,
    )
    .py()?;
    Ok((flat_bytes(py, &projected), flat_bytes(py, &transformed)))
}

#[pyfunction]
fn bounded_coefficients<'py>(py: Python<'py>, latent: &[u8], ranges: &[u8]) -> PyResult<Pair<'py>> {
    let (values, clamped) = projection::bounded_coefficients(&floats(latent)?, &rows::<2>(ranges)?).py()?;
    Ok((bytes(py, &values), bytes(py, &clamped)))
}

#[pyfunction]
fn deform_vertices<'py>(
    py: Python<'py>,
    vertices: &[u8],
    first_basis: &[u8],
    first_coefficients: &[u8],
    second_basis: &[u8],
    second_coefficients: &[u8],
) -> PyResult<Bound<'py, PyBytes>> {
    let (c1, c2) = (floats(first_coefficients)?, floats(second_coefficients)?);
    let vertices = rows::<3>(vertices)?;
    let deformed = projection::deform_vertices(
        &vertices,
        &basis_of(first_basis, vertices.len(), c1.len())?,
        &c1,
        &basis_of(second_basis, vertices.len(), c2.len())?,
        &c2,
    )
    .py()?;
    Ok(flat_bytes(py, &deformed))
}

#[pyfunction]
fn rigid_jacobian<'py>(
    py: Python<'py>,
    vertices: &[u8],
    rotation: &[u8],
    translation: &[u8],
    focal_factor: f64,
    model_scale: f64,
) -> PyResult<Bound<'py, PyBytes>> {
    let jacobian = projection::rigid_jacobian(
        &rows::<3>(vertices)?,
        fixed(rotation)?,
        fixed(translation)?,
        focal_factor as f32,
        model_scale as f32,
    )
    .py()?;
    Ok(matrix_bytes(py, &jacobian))
}

/// One deformation block: `(basis bytes, count, latent bytes, ranges bytes, mask, fixed bytes)`.
type Block<'a> = (&'a [u8], usize, &'a [u8], &'a [u8], Option<Vec<bool>>, Option<&'a [u8]>);

/// `(jacobian bytes, columns, prediction bytes)`.
#[pyfunction]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn model_jacobian<'py>(
    py: Python<'py>,
    vertices: &[u8],
    rotation: &[u8],
    translation: &[u8],
    focal_factor: f64,
    model_scale: f64,
    first: Block<'_>,
    second: Block<'_>,
) -> PyResult<(Bound<'py, PyBytes>, usize, Bound<'py, PyBytes>)> {
    let vertices = rows::<3>(vertices)?;
    let unpack = |block: Block<'_>| -> PyResult<_> {
        let (data, count, latent, ranges, mask, fixed) = block;
        let basis = basis_of(data, vertices.len(), count)?;
        Ok((basis, floats(latent)?, rows::<2>(ranges)?, mask, fixed.map(floats).transpose()?))
    };
    let (b1, l1, r1, m1, f1) = unpack(first)?;
    let (b2, l2, r2, m2, f2) = unpack(second)?;
    let (jacobian, prediction) = projection::model_jacobian(
        &vertices,
        fixed(rotation)?,
        fixed(translation)?,
        focal_factor as f32,
        model_scale as f32,
        Deformation { basis: &b1, latent: &l1, ranges: &r1, mask: m1.as_deref(), fixed: f1.as_deref() },
        Deformation { basis: &b2, latent: &l2, ranges: &r2, mask: m2.as_deref(), fixed: f2.as_deref() },
    )
    .py()?;
    Ok((matrix_bytes(py, &jacobian), jacobian.cols, flat_bytes(py, &prediction)))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(project_points, m)?,
        wrap_pyfunction!(bounded_coefficients, m)?,
        wrap_pyfunction!(deform_vertices, m)?,
        wrap_pyfunction!(rigid_jacobian, m)?,
        wrap_pyfunction!(model_jacobian, m)?,
    ] {
        m.add_function(function)?;
    }
    Ok(())
}
