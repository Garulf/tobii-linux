//! tobii_linux.face_detect: the face finder's letterbox, priors, decoding and network.
//!
//! Images cross as bytes with their array shape `(rows, columns)`; layouts as `(offset_x, offset_y, width,
//! height, scale)`; candidates and detections as `(x1, y1, x2, y2, score, angles)`.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_inference::face_detect::{self, Candidate, DetectOptions, Detection, FaceFinder, Letterbox};

use super::{InferenceResult, float_bytes, floats};
use crate::convert::bytes_like;

type LayoutTuple = (i64, i64, i64, i64, f64);
type BoxTuple = (f64, f64, f64, f64, f64, (f64, f64, f64));

fn layout_tuple(layout: &Letterbox) -> LayoutTuple {
    (layout.offset_x, layout.offset_y, layout.width, layout.height, f64::from(layout.scale))
}

fn layout_arg((offset_x, offset_y, width, height, scale): LayoutTuple) -> Letterbox {
    Letterbox { offset_x, offset_y, width, height, scale: scale as f32 }
}

fn box_tuple(b: &Candidate) -> BoxTuple {
    let [a0, a1, a2] = b.angles.map(f64::from);
    (f64::from(b.x1), f64::from(b.y1), f64::from(b.x2), f64::from(b.y2), f64::from(b.score), (a0, a1, a2))
}

fn detection_tuple(d: &Detection) -> BoxTuple {
    box_tuple(&Candidate { x1: d.x1, y1: d.y1, x2: d.x2, y2: d.y2, score: d.score, angles: d.angles })
}

fn candidate_arg((x1, y1, x2, y2, score, (a0, a1, a2)): BoxTuple) -> Candidate {
    let f = |v: f64| v as f32;
    Candidate { x1: f(x1), y1: f(y1), x2: f(x2), y2: f(y2), score: f(score), angles: [f(a0), f(a1), f(a2)] }
}

fn options(sensitivity: f64, min_face: f64, max_face: f64, region: Option<[f64; 4]>) -> DetectOptions {
    DetectOptions { sensitivity, min_face, max_face, region }
}

fn size(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

/// `(rows, columns)` of a 2D array shape; anything else is empty, which the core refuses.
fn image_shape(shape: &[usize]) -> (usize, usize) {
    match shape {
        &[rows, columns] => (rows, columns),
        _ => (0, 0),
    }
}

#[pyfunction]
fn resize_bilinear<'py>(
    py: Python<'py>,
    pixels: &Bound<'py, PyAny>,
    shape: Vec<usize>,
    width: i64,
    height: i64,
) -> PyResult<Bound<'py, PyBytes>> {
    let pixels = bytes_like(pixels)?;
    let (rows, columns) = image_shape(&shape);
    let resized =
        crate::exit::detached(py, || face_detect::resize_bilinear(&pixels, columns, rows, size(width), size(height)))
            .py()?;
    Ok(PyBytes::new(py, &resized))
}

#[pyfunction]
fn letterbox_layout(frame_width: usize, frame_height: usize) -> PyResult<LayoutTuple> {
    Ok(layout_tuple(&face_detect::letterbox_layout(frame_width, frame_height).py()?))
}

/// `(image bytes, layout)` of a frame.
#[pyfunction]
fn letterbox<'py>(
    py: Python<'py>,
    pixels: &Bound<'py, PyAny>,
    shape: Vec<usize>,
) -> PyResult<(Bound<'py, PyBytes>, LayoutTuple)> {
    let pixels = bytes_like(pixels)?;
    let (rows, columns) = image_shape(&shape);
    let (image, layout) = crate::exit::detached(py, || face_detect::letterbox(&pixels, columns, rows)).py()?;
    Ok((PyBytes::new(py, &image), layout_tuple(&layout)))
}

/// The priors as float32 bytes, four values each.
#[pyfunction]
fn anchors(py: Python<'_>, width: usize, height: usize) -> Bound<'_, PyBytes> {
    float_bytes(py, face_detect::anchors(width, height).into_iter().flatten())
}

#[pyfunction]
fn decode(
    output: &Bound<'_, PyAny>,
    priors: &Bound<'_, PyAny>,
    score_threshold: f64,
    width: usize,
    height: usize,
) -> PyResult<Vec<BoxTuple>> {
    let priors = floats(priors)?;
    let priors = priors.as_chunks::<4>().0;
    let boxes = face_detect::decode(&floats(output)?, priors, score_threshold as f32, width, height).py()?;
    Ok(boxes.iter().map(box_tuple).collect())
}

/// Indices of the boxes kept, from their `[x1, y1, x2, y2]` corners.
#[pyfunction]
fn suppress(boxes: Vec<[f64; 4]>) -> Vec<usize> {
    face_detect::suppress(&boxes.iter().map(|b| b.map(|v| v as f32)).collect::<Vec<_>>())
}

#[pyfunction]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn to_frame(
    boxes: Vec<BoxTuple>,
    layout: LayoutTuple,
    frame_width: usize,
    frame_height: usize,
    min_face: f64,
    max_face: f64,
    region: Option<[f64; 4]>,
) -> PyResult<Vec<BoxTuple>> {
    let boxes: Vec<Candidate> = boxes.into_iter().map(candidate_arg).collect();
    let options = options(0.0, min_face, max_face, region);
    let found = face_detect::to_frame(&boxes, &layout_arg(layout), frame_width, frame_height, &options).py()?;
    Ok(found.iter().map(detection_tuple).collect())
}

#[pyfunction]
fn score_threshold(sensitivity: f64) -> f64 {
    f64::from(face_detect::score_threshold(sensitivity))
}

/// The detector after the network, for raw outputs of a letterboxed frame.
#[pyfunction]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn detections(
    py: Python<'_>,
    output: &Bound<'_, PyAny>,
    layout: LayoutTuple,
    frame_width: usize,
    frame_height: usize,
    sensitivity: f64,
    min_face: f64,
    max_face: f64,
    region: Option<[f64; 4]>,
) -> PyResult<Vec<BoxTuple>> {
    let output = floats(output)?;
    let options = options(sensitivity, min_face, max_face, region);
    let found = crate::exit::detached(py, || {
        face_detect::detections(&output, &layout_arg(layout), frame_width, frame_height, &options)
    })
    .py()?;
    Ok(found.iter().map(detection_tuple).collect())
}

#[pyclass(frozen, name = "FaceFinder", module = "tobii_linux._native.face_detect")]
pub struct PyFaceFinder(FaceFinder);

#[pymethods]
impl PyFaceFinder {
    #[new]
    fn new(py: Python<'_>, model_directory: PathBuf) -> PyResult<Self> {
        let runtime = super::runtime(py)?;
        Ok(Self(crate::exit::detached(py, || FaceFinder::new(&runtime, &model_directory)).py()?))
    }

    /// Raw float32 outputs, as bytes, for a letterboxed 256×192 image.
    fn run<'py>(&self, py: Python<'py>, image: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
        let image = bytes_like(image)?;
        Ok(float_bytes(py, crate::exit::detached(py, || self.0.run(&image)).py()?))
    }

    #[allow(clippy::too_many_arguments, reason = "the Python signature")]
    fn detect(
        &self,
        py: Python<'_>,
        pixels: &Bound<'_, PyAny>,
        shape: Vec<usize>,
        sensitivity: f64,
        min_face: f64,
        max_face: f64,
        region: Option<[f64; 4]>,
    ) -> PyResult<Vec<BoxTuple>> {
        let pixels = bytes_like(pixels)?;
        let (rows, columns) = image_shape(&shape);
        let options = options(sensitivity, min_face, max_face, region);
        let found = crate::exit::detached(py, || self.0.detect(&pixels, columns, rows, &options)).py()?;
        Ok(found.iter().map(detection_tuple).collect())
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(resize_bilinear, m)?,
        wrap_pyfunction!(letterbox_layout, m)?,
        wrap_pyfunction!(letterbox, m)?,
        wrap_pyfunction!(anchors, m)?,
        wrap_pyfunction!(decode, m)?,
        wrap_pyfunction!(suppress, m)?,
        wrap_pyfunction!(to_frame, m)?,
        wrap_pyfunction!(score_threshold, m)?,
        wrap_pyfunction!(detections, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("INPUT_WIDTH", face_detect::INPUT_WIDTH)?;
    m.add("INPUT_HEIGHT", face_detect::INPUT_HEIGHT)?;
    m.add_class::<PyFaceFinder>()?;
    Ok(())
}
