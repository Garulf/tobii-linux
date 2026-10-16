//! tobii_linux.d1: the D1 crop, input tensor, output decoding and landmark model.
//!
//! Arrays cross as float32 bytes with their shape; the wrappers build them with NumPy.

use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_inference::d1::{self, D1LandmarkModel, D1Landmarks, Point};
use tobii_inference::numeric::pack_f32;

use super::{InferenceResult, affine, affine_bytes, float_bytes, points};
use crate::convert::bytes_like;

type PointTuples = Vec<(f64, f64)>;

fn tuples(points: &[Point]) -> PointTuples {
    points.iter().map(|p| (f64::from(p[0]), f64::from(p[1]))).collect()
}

fn invalid<T>(message: &str) -> PyResult<T> {
    Err(PyValueError::new_err(message.to_owned()))
}

fn packed(values: &[f64]) -> PyResult<Vec<f32>> {
    values.iter().map(|&v| pack_f32(v)).collect::<Result<_, _>>().py()
}

/// `(crop_points, quality)` of the decoded network output.
#[pyfunction]
#[pyo3(signature = (values, normalized_initial_shape, crop_width=96.0))]
fn decode_output(
    values: Vec<f64>,
    normalized_initial_shape: Vec<Vec<f64>>,
    crop_width: f64,
) -> PyResult<(PointTuples, Vec<f64>)> {
    let values = packed(&values)?;
    let shape = normalized_initial_shape.iter().map(|p| packed(p)).collect::<PyResult<Vec<_>>>()?;
    if shape.len() != d1::LANDMARKS || shape.iter().any(|p| p.len() != 2) || values.len() != d1::OUTPUTS {
        return invalid(d1::OUTPUT_MESSAGE);
    }
    if crop_width != 96.0 {
        return invalid(d1::OUTPUT_DATA_MESSAGE);
    }
    let shape: Vec<Point> = shape.iter().map(|p| [p[0], p[1]]).collect();
    let output = d1::decode_output(&values, &shape).py()?;
    Ok((tuples(&output.crop_points), output.quality.into_iter().map(f64::from).collect()))
}

#[pyfunction]
fn normalize_image_points(points: Vec<[f64; 2]>, image_width: f64, image_height: f64) -> PyResult<PointTuples> {
    d1::normalize_image_points(&[], image_width, image_height).py()?;
    let points =
        points.iter().map(|p| Ok([pack_f32(p[0])?, pack_f32(p[1])?])).collect::<Result<Vec<Point>, _>>().py()?;
    Ok(tuples(&d1::normalize_image_points(&points, image_width, image_height).py()?))
}

/// The 9216 float32 network inputs of a 96×96 crop, as bytes.
#[pyfunction]
fn prepare_crop_tensor<'py>(py: Python<'py>, pixels: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(float_bytes(py, d1::prepare_crop_tensor(&bytes_like(pixels)?).py()?))
}

#[pyfunction]
fn normalize_reference_shape(
    points_data: &Bound<'_, PyAny>,
    shape: Vec<usize>,
    crop_width: f64,
) -> PyResult<PointTuples> {
    let Some(reference) = points(points_data, &shape)?.filter(|_| crop_width == 96.0) else {
        return invalid(d1::REFERENCE_MESSAGE);
    };
    Ok(tuples(&d1::normalize_reference_shape(&reference).py()?))
}

#[pyfunction]
fn fit_similarity<'py>(
    py: Python<'py>,
    source: &Bound<'py, PyAny>,
    source_shape: Vec<usize>,
    target: &Bound<'py, PyAny>,
    target_shape: Vec<usize>,
) -> PyResult<Bound<'py, PyBytes>> {
    let (Some(source), Some(target)) = (points(source, &source_shape)?, points(target, &target_shape)?) else {
        return invalid(d1::POINT_SETS_MESSAGE);
    };
    Ok(affine_bytes(py, &d1::fit_similarity(&source, &target).py()?))
}

#[pyfunction]
fn inverse_transform_points<'py>(
    py: Python<'py>,
    points_data: &Bound<'py, PyAny>,
    points_shape: Vec<usize>,
    transform: &Bound<'py, PyAny>,
    transform_shape: Vec<usize>,
) -> PyResult<Bound<'py, PyBytes>> {
    let (Some(points), Some(transform)) = (points(points_data, &points_shape)?, affine(transform, &transform_shape)?)
    else {
        return invalid(d1::INVERSE_MESSAGE);
    };
    let mapped = d1::inverse_transform_points(&points, &transform).py()?;
    Ok(float_bytes(py, mapped.into_iter().flatten()))
}

#[pyfunction]
fn compose_affine<'py>(
    py: Python<'py>,
    first: &Bound<'py, PyAny>,
    first_shape: Vec<usize>,
    second: &Bound<'py, PyAny>,
    second_shape: Vec<usize>,
) -> PyResult<Bound<'py, PyBytes>> {
    let (Some(first), Some(second)) = (affine(first, &first_shape)?, affine(second, &second_shape)?) else {
        return invalid(d1::COMPOSE_MESSAGE);
    };
    Ok(affine_bytes(py, &d1::compose_affine(&first, &second).py()?))
}

fn size(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

#[pyfunction]
#[allow(clippy::too_many_arguments, reason = "the Python signature plus the transform's shape")]
fn warp_grayscale<'py>(
    py: Python<'py>,
    pixels: &Bound<'py, PyAny>,
    image_width: i64,
    image_height: i64,
    transform: &Bound<'py, PyAny>,
    transform_shape: Vec<usize>,
    crop_width: i64,
    crop_height: i64,
) -> PyResult<Bound<'py, PyBytes>> {
    let pixels = bytes_like(pixels)?;
    let (width, height, crop_width, crop_height) =
        (size(image_width), size(image_height), size(crop_width), size(crop_height));
    let transform = affine(transform, &transform_shape)?.unwrap_or([[f32::NAN; 3]; 2]);
    let crop =
        crate::exit::detached(py, || d1::warp_grayscale(&pixels, width, height, &transform, crop_width, crop_height))
            .py()?;
    Ok(PyBytes::new(py, &crop))
}

/// Python's `isinstance(x, (int, np.integer))` for the four box values.
pub fn integer_box(bounding_box: &Bound<'_, PyAny>) -> Option<[i64; 4]> {
    let values: Vec<Bound<'_, PyAny>> = bounding_box.try_iter().ok()?.collect::<PyResult<_>>().ok()?;
    let values: Vec<i64> = values.iter().map(|v| v.extract::<i64>().ok()).collect::<Option<_>>()?;
    values.try_into().ok()
}

#[pyfunction]
fn crop_transform<'py>(
    py: Python<'py>,
    reference: &Bound<'py, PyAny>,
    reference_shape: Vec<usize>,
    bounding_box: &Bound<'py, PyAny>,
    roll_degrees: f64,
    compensate_roll: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyBytes>> {
    let (Some(reference), Some(bounding_box)) = (points(reference, &reference_shape)?, integer_box(bounding_box))
    else {
        return invalid(d1::CROP_MESSAGE);
    };
    let transform = d1::crop_transform(&reference, bounding_box, roll_degrees, compensate_roll.is_truthy()?).py()?;
    Ok(affine_bytes(py, &transform))
}

pub type Landmarks = (PointTuples, PointTuples, Vec<f64>, Vec<Vec<f64>>);

/// `(image_points, normalized_points, quality, image_to_crop)` of one frame.
pub fn landmarks_tuple(landmarks: &D1Landmarks) -> Landmarks {
    let transform = landmarks.image_to_crop.iter().map(|row| row.iter().map(|&v| f64::from(v)).collect()).collect();
    let quality = landmarks.quality.iter().map(|&q| f64::from(q)).collect();
    (tuples(&landmarks.image_points), tuples(&landmarks.normalized_points), quality, transform)
}

#[pyclass(frozen, name = "D1LandmarkModel", module = "tobii_linux._native.d1")]
pub struct PyD1LandmarkModel(D1LandmarkModel);

#[pymethods]
impl PyD1LandmarkModel {
    /// The network in `model_directory` and the raw init_shape.bin points as float32 bytes.
    #[new]
    fn new(
        py: Python<'_>,
        model_directory: PathBuf,
        raw_reference: &Bound<'_, PyAny>,
        shape: Vec<usize>,
    ) -> PyResult<Self> {
        let Some(reference) = points(raw_reference, &shape)? else { return invalid(d1::REFERENCE_MESSAGE) };
        let runtime = super::runtime(py)?;
        let model = crate::exit::detached(py, || D1LandmarkModel::new(&runtime, &model_directory, &reference)).py()?;
        Ok(Self(model))
    }

    #[getter]
    fn reference(&self) -> PointTuples {
        tuples(self.0.reference())
    }

    /// `(image_points, normalized_points, quality, image_to_crop)` of one frame.
    #[pyo3(signature = (pixels, width, height, bounding_box, roll_degrees=0.0, compensate_roll=None))]
    #[allow(clippy::too_many_arguments, reason = "the Python method's signature")]
    fn predict_frame(
        &self,
        py: Python<'_>,
        pixels: &Bound<'_, PyAny>,
        width: i64,
        height: i64,
        bounding_box: &Bound<'_, PyAny>,
        roll_degrees: f64,
        compensate_roll: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Landmarks> {
        let Some(bounding_box) = integer_box(bounding_box) else { return invalid(d1::CROP_MESSAGE) };
        let compensate = compensate_roll.map_or(Ok(true), |c| c.is_truthy())?;
        let pixels = bytes_like(pixels)?;
        let (width, height) = (size(width), size(height));
        let landmarks = crate::exit::detached(py, || {
            self.0.predict_frame(&pixels, width, height, bounding_box, roll_degrees, compensate)
        })
        .py()?;
        Ok(landmarks_tuple(&landmarks))
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(decode_output, m)?,
        wrap_pyfunction!(normalize_image_points, m)?,
        wrap_pyfunction!(prepare_crop_tensor, m)?,
        wrap_pyfunction!(normalize_reference_shape, m)?,
        wrap_pyfunction!(fit_similarity, m)?,
        wrap_pyfunction!(inverse_transform_points, m)?,
        wrap_pyfunction!(compose_affine, m)?,
        wrap_pyfunction!(warp_grayscale, m)?,
        wrap_pyfunction!(crop_transform, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add_class::<PyD1LandmarkModel>()
}
