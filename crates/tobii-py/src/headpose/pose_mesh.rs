//! tobii_linux.pose_mesh: dense deformation bases, mesh normalization and rescaling.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::mesh;
use tobii_models::face::{Deformation, FaceMesh};

use super::{HeadPoseResult, basis_of, bytes, flat_bytes, rows};

type Unit = Vec<(usize, f64, f64, f64)>;
type Triple<'py> = (Bound<'py, PyBytes>, Bound<'py, PyBytes>, Bound<'py, PyBytes>);

/// `(animation, shape)` dense bases as bytes, from the vertex count and sparse units.
#[pyfunction]
fn dense_deformations<'py>(
    py: Python<'py>,
    points: usize,
    animation_units: Vec<Unit>,
    shape_units: Vec<Unit>,
) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyBytes>)> {
    let units = |units: Vec<Unit>| -> Vec<Deformation> {
        units
            .into_iter()
            .map(|entries| Deformation {
                name: String::new(),
                entries: entries.into_iter().map(|(i, x, y, z)| (i, [x, y, z])).collect(),
            })
            .collect()
    };
    let mesh = FaceMesh {
        vertices: vec![[0.0; 3]; points],
        texcoords: vec![],
        triangles: vec![],
        animation_units: units(animation_units),
        shape_units: units(shape_units),
    };
    let (animation, shape) = mesh::dense_deformations(&mesh).py()?;
    Ok((bytes(py, animation.values()), bytes(py, shape.values())))
}

#[pyfunction]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn normalize_mesh<'py>(
    py: Python<'py>,
    vertices: &[u8],
    first_basis: &[u8],
    first_count: usize,
    second_basis: &[u8],
    second_count: usize,
    landmark_vertices: [usize; 4],
    target_distance: f64,
) -> PyResult<Triple<'py>> {
    let vertices = rows::<3>(vertices)?;
    let first = basis_of(first_basis, vertices.len(), first_count)?;
    let second = basis_of(second_basis, vertices.len(), second_count)?;
    let (moved, first, second) =
        mesh::normalize_mesh(&vertices, &first, &second, landmark_vertices, target_distance).py()?;
    Ok((flat_bytes(py, &moved), bytes(py, first.values()), bytes(py, second.values())))
}

#[pyfunction]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn rescale_mesh<'py>(
    py: Python<'py>,
    vertices: &[u8],
    first_basis: &[u8],
    first_count: usize,
    second_basis: &[u8],
    second_count: usize,
    previous_distance: f64,
    target_distance: f64,
) -> PyResult<Triple<'py>> {
    let vertices = rows::<3>(vertices)?;
    let first = basis_of(first_basis, vertices.len(), first_count)?;
    let second = basis_of(second_basis, vertices.len(), second_count)?;
    let (moved, first, second) =
        mesh::rescale_mesh(&vertices, &first, &second, previous_distance, target_distance).py()?;
    Ok((flat_bytes(py, &moved), bytes(py, first.values()), bytes(py, second.values())))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(dense_deformations, m)?,
        wrap_pyfunction!(normalize_mesh, m)?,
        wrap_pyfunction!(rescale_mesh, m)?,
    ] {
        m.add_function(function)?;
    }
    Ok(())
}
