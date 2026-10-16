//! tobii_linux.face_tracking: the working-view geometry and the native face tracker.
//!
//! Images cross as bytes with their `(rows, columns)` shape, points as float32 bytes, boxes
//! as `(x, y, width, height)` tuples and detections as `(x1, y1, x2, y2, ...)` tuples.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use tobii_headpose::tracking::{self, FaceTracker, TrackedFrame};
use tobii_headpose::{FdpId, temporal::Rectangle};
use tobii_inference::face_detect::Detection;

use super::{HeadPoseResult, flat_bytes, rows};
use crate::convert::bytes_like;

/// `(rows, columns)` of a 2D array shape; anything else is empty, which the core refuses.
fn image_shape(shape: &[usize]) -> (usize, usize) {
    match shape {
        &[rows, columns] => (rows, columns),
        _ => (0, 0),
    }
}

#[pyfunction]
fn expanded_region(face: Rectangle, frame_width: i64, frame_height: i64, expansion: f32) -> PyResult<Rectangle> {
    tracking::expanded_region(face, frame_width, frame_height, expansion).py()
}

type View<'py> = (Bound<'py, PyBytes>, (usize, usize), Rectangle, f32, Rectangle);

/// `(image bytes, (rows, columns), region, ratio, face)` of the working view.
#[pyfunction]
fn working_view<'py>(
    py: Python<'py>,
    pixels: &Bound<'py, PyAny>,
    shape: Vec<usize>,
    face: Rectangle,
    expansion: f32,
    target: i64,
) -> PyResult<View<'py>> {
    let pixels = bytes_like(pixels)?;
    let (rows, columns) = image_shape(&shape);
    let view =
        crate::exit::detached(py, || tracking::working_view(&pixels, columns, rows, face, expansion, target)).py()?;
    Ok((PyBytes::new(py, &view.image), (view.height, view.width), view.region, view.ratio, view.face))
}

#[pyfunction]
fn working_face_in_frame(face: Rectangle, region: Rectangle, ratio: f32) -> PyResult<Rectangle> {
    tracking::working_face_in_frame(face, region, ratio).py()
}

#[pyfunction]
fn to_frame_points<'py>(
    py: Python<'py>,
    points: &[u8],
    region: Rectangle,
    frame_width: i64,
    frame_height: i64,
) -> PyResult<Bound<'py, PyBytes>> {
    Ok(flat_bytes(py, &tracking::to_frame_points(&rows::<2>(points)?, region, frame_width, frame_height)))
}

#[pyfunction]
fn landmark_box(points: &[u8], frame_width: i64, frame_height: i64) -> PyResult<Rectangle> {
    tracking::landmark_box(&rows::<2>(points)?, frame_width, frame_height).py()
}

/// The roll from the two brow points then the two nostril points.
#[pyfunction]
fn roll_degrees(roll_points: [[f32; 2]; 4], width: i64, height: i64) -> f32 {
    tracking::roll_degrees(roll_points, width, height)
}

#[pyfunction]
fn overlap(corners: [f32; 4], face: Rectangle) -> f32 {
    tracking::overlap(corners, face)
}

/// The rectangle of the best `(x1, y1, x2, y2)` detection for `face`, or `None`.
#[pyfunction]
fn best_redetection(corners: Vec<[f32; 4]>, face: Rectangle) -> Option<Rectangle> {
    let found: Vec<Detection> = corners
        .into_iter()
        .map(|[x1, y1, x2, y2]| Detection { x1, y1, x2, y2, score: 0.0, angles: [0.0; 3] })
        .collect();
    tracking::best_redetection(&found, face)
}

type Points = Vec<(f64, f64)>;
type Frame<'py> = (
    u8,
    Option<Rectangle>,
    Option<(usize, usize)>,
    Option<Points>,
    Option<Bound<'py, PyBytes>>,
    f64,
    bool,
    Option<Bound<'py, PyDict>>,
    f64,
    Option<Bound<'py, PyBytes>>,
);

/// The `TrackedFrame` fields in order; the fit is the `TemporalPoseFitter.process` dict.
fn frame_tuple<'py>(py: Python<'py>, tracked: &TrackedFrame) -> PyResult<Frame<'py>> {
    let points = |points: &[[f32; 2]]| points.iter().map(|p| (f64::from(p[0]), f64::from(p[1]))).collect();
    Ok((
        tracked.state as u8,
        tracked.face,
        tracked.working_size.map(|[width, height]| (width, height)),
        tracked.working_points.as_deref().map(points),
        tracked.frame_points.as_deref().map(|p| flat_bytes(py, p)),
        f64::from(tracked.quality_score),
        tracked.accepted,
        tracked.fit.as_ref().map(|fit| super::pose_temporal::fit_result(py, fit)).transpose()?,
        f64::from(tracked.roll_degrees),
        tracked.fitted_points.as_deref().map(|p| flat_bytes(py, p)),
    ))
}

#[pyclass(name = "FaceTracker", module = "tobii_linux.face_tracking")]
struct PyFaceTracker(FaceTracker);

#[pymethods]
impl PyFaceTracker {
    #[new]
    #[pyo3(signature = (model_directory, region=None, smoothing_factors=None))]
    fn new(
        py: Python<'_>,
        model_directory: PathBuf,
        region: Option<[f64; 4]>,
        smoothing_factors: Option<Vec<f32>>,
    ) -> PyResult<Self> {
        let runtime = crate::inference::runtime(py)?;
        let tracker = crate::exit::detached(py, || {
            FaceTracker::new(&runtime, &model_directory, region, smoothing_factors.as_deref())
        })
        .py()?;
        Ok(Self(tracker))
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    fn process<'py>(
        &mut self,
        py: Python<'py>,
        pixels: &Bound<'py, PyAny>,
        shape: Vec<usize>,
        time_ms: i64,
    ) -> PyResult<Frame<'py>> {
        let pixels = bytes_like(pixels)?;
        let (rows, columns) = image_shape(&shape);
        let tracker = &mut self.0;
        let tracked = crate::exit::detached(py, || tracker.process(&pixels, columns, rows, time_ms)).py()?;
        frame_tuple(py, &tracked)
    }

    #[getter]
    fn state(&self) -> u8 {
        self.0.state as u8
    }

    #[getter]
    fn face(&self) -> Rectangle {
        self.0.face
    }

    #[getter]
    fn roll(&self) -> f32 {
        self.0.roll
    }

    #[getter]
    fn object_state(&self) -> u8 {
        self.0.object_state as u8
    }

    #[getter]
    fn smoothing_epoch(&self) -> i64 {
        self.0.smoothing_epoch
    }

    #[getter]
    fn lost_since(&self) -> i64 {
        self.0.lost_since
    }

    #[getter]
    fn recovery_timeout_ms(&self) -> i64 {
        self.0.recovery_timeout_ms
    }

    #[getter]
    fn smoothing_entries(&self) -> Vec<(u32, u32, u32, f32)> {
        self.0.smoothing_entries.clone()
    }

    #[getter]
    fn region(&self) -> Option<[f64; 4]> {
        self.0.region
    }

    #[setter]
    fn set_region(&mut self, region: Option<[f64; 4]>) {
        self.0.region = region;
    }

    #[getter]
    fn landmark_ids(&self) -> Vec<FdpId> {
        self.0.landmark_ids().to_vec()
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(expanded_region, m)?,
        wrap_pyfunction!(working_view, m)?,
        wrap_pyfunction!(working_face_in_frame, m)?,
        wrap_pyfunction!(to_frame_points, m)?,
        wrap_pyfunction!(landmark_box, m)?,
        wrap_pyfunction!(roll_degrees, m)?,
        wrap_pyfunction!(overlap, m)?,
        wrap_pyfunction!(best_redetection, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add_class::<PyFaceTracker>()
}
