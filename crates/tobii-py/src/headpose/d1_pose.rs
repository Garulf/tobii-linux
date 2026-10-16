//! tobii_linux.headpose: D1 landmarks of a supplied face box and their staged fit.

use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use tobii_headpose::d1_pose::D1Pose;

use super::HeadPoseResult;
use super::pose_policy::{Staged, staged};
use crate::convert::bytes_like;
use crate::inference::d1::{Landmarks, integer_box, landmarks_tuple};

#[pyclass(frozen, name = "NativeD1Pose", module = "tobii_linux.headpose")]
struct PyD1Pose(D1Pose);

fn size(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

#[pymethods]
impl PyD1Pose {
    #[new]
    fn new(py: Python<'_>, model_directory: PathBuf) -> PyResult<Self> {
        let runtime = crate::inference::runtime(py)?;
        Ok(Self(crate::exit::detached(py, || D1Pose::new(&runtime, &model_directory)).py()?))
    }

    /// `(landmarks, quality_score, quality_accepted, staged fit or None)`.
    #[allow(clippy::too_many_arguments, reason = "the Python method's signature")]
    fn predict_frame<'py>(
        &self,
        py: Python<'py>,
        pixels: &Bound<'py, PyAny>,
        width: i64,
        height: i64,
        face_box: &Bound<'py, PyAny>,
        caller_mode: i64,
        roll_degrees: f64,
    ) -> PyResult<(Landmarks, f64, bool, Option<Staged<'py>>)> {
        let Some(face_box) = integer_box(face_box) else {
            return Err(PyValueError::new_err(tobii_inference::d1::CROP_MESSAGE));
        };
        let pixels = bytes_like(pixels)?;
        let (width, height) = (size(width), size(height));
        let pose = &self.0;
        let result = crate::exit::detached(py, || {
            pose.predict_frame(&pixels, width, height, face_box, caller_mode, roll_degrees)
        })
        .py()?;
        let fit = result.fit.as_ref().map(|fit| staged(py, fit));
        Ok((landmarks_tuple(&result.landmarks), f64::from(result.quality_score), result.quality_accepted, fit))
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyD1Pose>()
}
