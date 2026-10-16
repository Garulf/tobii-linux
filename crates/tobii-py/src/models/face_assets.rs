//! tobii_linux.face_assets: mesh, FDP mapping, landmark identifiers and shape files.

use std::path::PathBuf;

use pyo3::prelude::*;
use tobii_models::face::{self, Deformation, FaceMesh, Fdp, Shapes};

use super::ModelResult;

/// `(dimensions, values)` of a float32 shape file; the values are float32, widened exactly.
#[pyfunction]
#[pyo3(signature = (path, dimensions=2))]
fn read_shapes(path: PathBuf, dimensions: usize) -> PyResult<(Vec<u32>, Vec<f32>)> {
    let shapes = Shapes::read(&path, dimensions).py()?;
    Ok((shapes.dimensions, shapes.values))
}

#[pyfunction]
fn read_landmark_ids(path: PathBuf) -> PyResult<Vec<(u32, u32)>> {
    face::read_landmark_ids(&path).py()
}

type FdpRow = ((i64, i64), (f64, f64, f64), i64, i64);

/// `(identifier, position, mesh, vertex)` of every FDP row in file order.
#[pyfunction]
fn read_fdp(path: PathBuf) -> PyResult<Vec<FdpRow>> {
    let fdp = Fdp::read(&path).py()?;
    Ok(fdp.points.iter().map(|(id, p)| (*id, p.position.into(), p.mesh, p.vertex)).collect())
}

type Unit = (String, Vec<(usize, f64, f64, f64)>);
type MeshFields = (Vec<(f64, f64, f64)>, Vec<(f64, f64)>, Vec<(usize, usize, usize)>, Vec<Unit>, Vec<Unit>);

fn units(units: &[Deformation]) -> Vec<Unit> {
    units.iter().map(|u| (u.name.clone(), u.entries.iter().map(|&(i, [x, y, z])| (i, x, y, z)).collect())).collect()
}

/// The fields of FaceMesh in order: vertices, texcoords, triangles, animation and shape units.
#[pyfunction]
fn read_mesh(path: PathBuf) -> PyResult<MeshFields> {
    let mesh = FaceMesh::read(&path).py()?;
    Ok((
        mesh.vertices.iter().map(|&v| v.into()).collect(),
        mesh.texcoords.iter().map(|&t| t.into()).collect(),
        mesh.triangles.iter().map(|&t| t.into()).collect(),
        units(&mesh.animation_units),
        units(&mesh.shape_units),
    ))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(read_shapes, m)?,
        wrap_pyfunction!(read_landmark_ids, m)?,
        wrap_pyfunction!(read_fdp, m)?,
        wrap_pyfunction!(read_mesh, m)?,
    ] {
        m.add_function(function)?;
    }
    Ok(())
}
