//! driver.HeadPoseProcessor: one image frame to one head-pose sample in the Rust core.

use std::path::PathBuf;

use pyo3::prelude::*;
use tobii_headpose::{DisplayArea, HeadPoseProcessor, Vec3};

use super::HeadPoseResult;
use crate::convert::bytes_like;

type Sample = (i64, bool, Vec3, Vec3);

#[pyclass(name = "HeadPoseProcessor", module = "tobii_linux.driver")]
struct PyHeadPoseProcessor(HeadPoseProcessor);

#[pymethods]
impl PyHeadPoseProcessor {
    #[new]
    fn new(
        py: Python<'_>,
        model_directory: PathBuf,
        top_left: Vec3,
        top_right: Vec3,
        bottom_left: Vec3,
    ) -> PyResult<Self> {
        let runtime = crate::inference::runtime(py)?;
        let area = DisplayArea { top_left, top_right, bottom_left };
        Ok(Self(crate::exit::detached(py, || HeadPoseProcessor::new(&runtime, &model_directory, area)).py()?))
    }

    fn start(&mut self, camera_translation: Vec3, camera_angles: Vec3) {
        self.0.start(camera_translation, camera_angles);
    }

    /// `(timestamp_us, valid, position_mm, rotation_rad)` of one frame.
    fn process(
        &mut self,
        py: Python<'_>,
        pixels: &Bound<'_, PyAny>,
        width: usize,
        height: usize,
        timestamp_us: i64,
    ) -> PyResult<Sample> {
        let pixels = bytes_like(pixels)?;
        let processor = &mut self.0;
        let sample = crate::exit::detached(py, || processor.process(&pixels, width, height, timestamp_us)).py()?;
        Ok((sample.timestamp_us, sample.valid, sample.position_mm, sample.rotation_rad))
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyHeadPoseProcessor>()
}
