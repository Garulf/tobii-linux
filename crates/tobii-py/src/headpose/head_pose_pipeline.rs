//! tobii_linux.head_pose_pipeline: the head-pose output stage and the sample encoding.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::{DisplayArea, HeadPoseOutput, HeadPoseSample, Vec3};

use super::fixed;

type Sample = (i64, bool, Vec3, Vec3);

#[pyclass(name = "HeadPoseOutput", module = "tobii_linux.head_pose_pipeline")]
struct PyHeadPoseOutput(HeadPoseOutput);

#[pymethods]
impl PyHeadPoseOutput {
    #[new]
    fn new(camera_translation: Vec3, camera_angles: Vec3, top_left: Vec3, top_right: Vec3, bottom_left: Vec3) -> Self {
        Self(HeadPoseOutput::new(camera_translation, camera_angles, &DisplayArea { top_left, top_right, bottom_left }))
    }

    /// `(timestamp_us, valid, position_mm, rotation_rad)` from fitted float32 state bytes.
    #[pyo3(signature = (fitted_rigid_state, timestamp_us))]
    fn process(&mut self, fitted_rigid_state: Option<&[u8]>, timestamp_us: i64) -> PyResult<Sample> {
        let state = fitted_rigid_state.map(fixed::<6>).transpose()?;
        let sample = self.0.process(state.as_ref(), timestamp_us);
        Ok((sample.timestamp_us, sample.valid, sample.position_mm, sample.rotation_rad))
    }

    #[getter]
    fn camera(&self) -> Vec<f32> {
        self.0.camera.as_flattened().to_vec()
    }

    #[getter]
    fn display(&self) -> Vec<f32> {
        self.0.display.as_flattened().to_vec()
    }
}

/// The 48-byte `tobii_head_pose_t`.
#[pyfunction]
fn encode_sample(
    py: Python<'_>,
    timestamp_us: i64,
    valid: bool,
    position_mm: Vec3,
    rotation_rad: Vec3,
) -> Bound<'_, PyBytes> {
    PyBytes::new(py, &HeadPoseSample { timestamp_us, valid, position_mm, rotation_rad }.encode())
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(encode_sample, m)?)?;
    m.add_class::<PyHeadPoseOutput>()
}
