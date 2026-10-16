//! tobii_linux.pose_output: Euler conversions, the adapter pose and the One Euro filters.

use pyo3::prelude::*;
use tobii_headpose::Vec3;
use tobii_headpose::output::{self, FilterConfig, HeadPoseFilter, Matrix3, OneEuroFilter, OneEuroVectorFilter};

fn matrix3(values: Vec<f32>) -> PyResult<Matrix3> {
    let rows = values.as_chunks::<3>().0.to_vec();
    rows.try_into().map_err(|_| super::invalid("Expected an order-3 matrix"))
}

#[pyfunction]
fn euler_matrix(angles: Vec3) -> Vec<f32> {
    output::euler_matrix(angles).as_flattened().to_vec()
}

#[pyfunction]
fn wrap_angle(angle: f32) -> f32 {
    output::wrap_angle(angle)
}

#[pyfunction]
fn matrix_euler(matrix: Vec<f32>) -> PyResult<Vec3> {
    Ok(output::matrix_euler(&matrix3(matrix)?))
}

/// `(matrix, position)` of the face adapter.
#[pyfunction]
fn adapter_head_pose(rotation: Vec3, translation: Vec3) -> (Vec<f32>, Vec3) {
    let (matrix, position) = output::adapter_head_pose(rotation, translation);
    (matrix.as_flattened().to_vec(), position)
}

#[pyclass(name = "OneEuroFilter", module = "tobii_linux.pose_output")]
struct PyOneEuroFilter(OneEuroFilter);

#[pymethods]
impl PyOneEuroFilter {
    #[new]
    fn new(config: FilterConfig) -> Self {
        Self(OneEuroFilter::new(config))
    }

    fn filter(&mut self, value: f32, timestamp_us: i64) -> f32 {
        self.0.filter(value, timestamp_us)
    }
}

#[pyclass(name = "OneEuroVectorFilter", module = "tobii_linux.pose_output")]
struct PyOneEuroVectorFilter(OneEuroVectorFilter);

#[pymethods]
impl PyOneEuroVectorFilter {
    #[new]
    fn new(config: FilterConfig) -> Self {
        Self(OneEuroVectorFilter::new(config))
    }

    fn filter(&mut self, value: Vec3, timestamp_us: i64) -> Vec3 {
        self.0.filter(value, timestamp_us)
    }
}

#[pyclass(name = "HeadPoseFilter", module = "tobii_linux.pose_output")]
struct PyHeadPoseFilter(HeadPoseFilter);

#[pymethods]
impl PyHeadPoseFilter {
    #[new]
    fn new(rotation_config: FilterConfig, position_config: FilterConfig) -> Self {
        Self(HeadPoseFilter::new(rotation_config, position_config))
    }

    fn filter(&mut self, rotation: Vec3, position: Vec3, timestamp_us: i64) -> (Vec3, Vec3) {
        self.0.filter(rotation, position, timestamp_us)
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(euler_matrix, m)?,
        wrap_pyfunction!(wrap_angle, m)?,
        wrap_pyfunction!(matrix_euler, m)?,
        wrap_pyfunction!(adapter_head_pose, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add_class::<PyOneEuroFilter>()?;
    m.add_class::<PyOneEuroVectorFilter>()?;
    m.add_class::<PyHeadPoseFilter>()?;
    m.add("ROTATION_FILTER", output::ROTATION_FILTER)?;
    m.add("POSITION_FILTER", output::POSITION_FILTER)?;
    Ok(())
}
