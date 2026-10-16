//! tobii_linux.pose_geometry: display and camera matrices and the geometry stage.

use pyo3::prelude::*;
use tobii_headpose::Vec3;
use tobii_headpose::geometry::{self, Matrix4};
use tobii_headpose::output::Matrix3;

/// A flattened matrix in row order.
fn flat3(matrix: Matrix3) -> Vec<f32> {
    matrix.as_flattened().to_vec()
}

fn flat4(matrix: Matrix4) -> Vec<f32> {
    matrix.as_flattened().to_vec()
}

fn matrix4(values: Vec<f32>) -> PyResult<Matrix4> {
    let rows = values.as_chunks::<4>().0.to_vec();
    rows.try_into().map_err(|_| super::invalid("Expected a 4x4 matrix"))
}

fn matrix3(values: Vec<f32>) -> PyResult<Matrix3> {
    let rows = values.as_chunks::<3>().0.to_vec();
    rows.try_into().map_err(|_| super::invalid("Expected a 3x3 matrix"))
}

#[pyfunction]
fn x_rotation(angle: f32) -> Vec<f32> {
    flat3(geometry::x_rotation(angle))
}

#[pyfunction]
fn y_rotation(angle: f32) -> Vec<f32> {
    flat3(geometry::y_rotation(angle))
}

#[pyfunction]
fn z_rotation(angle: f32) -> Vec<f32> {
    flat3(geometry::z_rotation(angle))
}

#[pyfunction]
fn geometry_config() -> (Vec<f32>, Vec<f32>) {
    let (config3, config4) = geometry::geometry_config();
    (flat3(config3), flat4(config4))
}

#[pyfunction]
fn display_normal(top_left: Vec3, bottom_left: Vec3, top_right: Vec3) -> Vec3 {
    geometry::display_normal(top_left, bottom_left, top_right)
}

#[pyfunction]
fn display_center(top_left: Vec3, bottom_left: Vec3, top_right: Vec3) -> Vec3 {
    geometry::display_center(top_left, bottom_left, top_right)
}

#[pyfunction]
fn display_matrix(top_left: Vec3, bottom_left: Vec3, top_right: Vec3) -> Vec<f32> {
    flat4(geometry::display_matrix(top_left, bottom_left, top_right))
}

#[pyfunction]
fn camera_rotation(angles: Vec3) -> Vec<f32> {
    flat3(geometry::camera_rotation(angles))
}

#[pyfunction]
fn camera_extrinsic(translation: Vec3, angles: Vec3) -> Vec<f32> {
    flat4(geometry::camera_extrinsic(translation, angles))
}

#[pyfunction]
fn transform_position(position: Vec3, camera: Vec<f32>, display: Vec<f32>, config4: Vec<f32>) -> PyResult<Vec3> {
    Ok(geometry::transform_position(position, &matrix4(camera)?, &matrix4(display)?, &matrix4(config4)?))
}

#[pyfunction]
fn transform_rotation(angles: Vec3, camera: Vec<f32>, display: Vec<f32>, config3: Vec<f32>) -> PyResult<Vec3> {
    Ok(geometry::transform_rotation(angles, &matrix4(camera)?, &matrix4(display)?, &matrix3(config3)?))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(x_rotation, m)?,
        wrap_pyfunction!(y_rotation, m)?,
        wrap_pyfunction!(z_rotation, m)?,
        wrap_pyfunction!(geometry_config, m)?,
        wrap_pyfunction!(display_normal, m)?,
        wrap_pyfunction!(display_center, m)?,
        wrap_pyfunction!(display_matrix, m)?,
        wrap_pyfunction!(camera_rotation, m)?,
        wrap_pyfunction!(camera_extrinsic, m)?,
        wrap_pyfunction!(transform_position, m)?,
        wrap_pyfunction!(transform_rotation, m)?,
    ] {
        m.add_function(function)?;
    }
    Ok(())
}
