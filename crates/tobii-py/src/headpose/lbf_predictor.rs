//! tobii_linux.lbf_predictor: feature gathering, the stage predictor and refinement over
//! the model dicts of `lbf.read_lbf`.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::lbf;
use tobii_models::lbf::{LbfModel, LbfPoint, Matrix};

use super::lbf_features::{forest, image};
use super::{HeadPoseResult, bytes, floats};
use crate::convert::bytes_like;

fn item<'py>(object: &Bound<'py, PyAny>, key: &str) -> PyResult<Bound<'py, PyAny>> {
    object.get_item(key)
}

/// A matrix from its dict: shape, float32 bias and weight bytes, has_bias, codebook maxima.
pub fn matrix(object: &Bound<'_, PyAny>) -> PyResult<Matrix> {
    let (inputs, outputs): (usize, usize) = item(object, "shape")?.extract()?;
    let biases = floats(&bytes_like(&item(object, "biases")?)?)?;
    let weights = floats(&bytes_like(&item(object, "weights")?)?)?;
    if weights.len() != inputs * outputs || biases.len() != outputs {
        return Err(super::invalid("Matrix data does not match its shape"));
    }
    Ok(Matrix {
        inputs,
        outputs,
        biases,
        weights,
        has_bias: item(object, "has_bias")?.extract()?,
        codebook_max_abs: item(object, "codebook_max_abs")?.extract()?,
    })
}

pub fn matrices(object: &Bound<'_, PyAny>) -> PyResult<Vec<Matrix>> {
    object.try_iter()?.map(|m| matrix(&m?)).collect()
}

/// The points and stages of a `read_lbf` dict; other fields are not used.
pub fn model(object: &Bound<'_, PyAny>) -> PyResult<LbfModel> {
    let mut points = vec![];
    for point in item(object, "points")?.try_iter()? {
        let point = point?;
        let submodels = item(&point, "submodels")?.try_iter()?.map(|f| forest(&f?)).collect::<PyResult<Vec<_>>>()?;
        let (x, y): (f32, f32) = item(&point, "xy")?.extract()?;
        points.push(LbfPoint { id: item(&point, "id")?.extract()?, xy: [x, y], submodels });
    }
    let stages = item(object, "stages")?.try_iter()?.map(|s| matrices(&s?)).collect::<PyResult<Vec<_>>>()?;
    Ok(LbfModel { version: 4, points, tree_count: 0, forest_end: 0, stages, source_bytes: 0 })
}

fn centers(centers: Vec<(f64, f64)>) -> Vec<[f64; 2]> {
    centers.into_iter().map(|(a, b)| [a, b]).collect()
}

/// `(features, stage_scale)`.
#[pyfunction]
#[pyo3(signature = (model, stage, pixels, rows, columns, stride, centers, scale, mirrored=false))]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn gather_features(
    model: &Bound<'_, PyAny>,
    stage: i64,
    pixels: &Bound<'_, PyAny>,
    rows: i64,
    columns: i64,
    stride: i64,
    centers: Vec<(f64, f64)>,
    scale: f64,
    mirrored: bool,
) -> PyResult<(Vec<usize>, f32)> {
    let model = self::model(model)?;
    let stage = usize::try_from(stage).map_err(|_| super::invalid("Landmark count or stage mismatch"))?;
    let pixels = bytes_like(pixels)?;
    lbf::gather_features(
        &model,
        stage,
        image(&pixels, rows, columns, stride),
        &self::centers(centers),
        scale as f32,
        mirrored,
    )
    .py()
}

#[pyfunction]
fn predict_stage<'py>(
    py: Python<'py>,
    matrices: &Bound<'py, PyAny>,
    features: Vec<i64>,
) -> PyResult<Bound<'py, PyBytes>> {
    let matrices = self::matrices(matrices)?;
    let features = features
        .into_iter()
        .map(usize::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| super::invalid("Feature index outside predictor"))?;
    Ok(bytes(py, &lbf::predict_stage(&matrices, &features).py()?))
}

#[pyfunction]
#[pyo3(signature = (model, pixels, rows, columns, stride, centers, scale, mirrored=false))]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn refine_landmarks(
    py: Python<'_>,
    model: &Bound<'_, PyAny>,
    pixels: &Bound<'_, PyAny>,
    rows: i64,
    columns: i64,
    stride: i64,
    centers: Vec<(f64, f64)>,
    scale: f64,
    mirrored: bool,
) -> PyResult<Vec<(f32, f32)>> {
    let model = self::model(model)?;
    let pixels = bytes_like(pixels)?;
    let centers = self::centers(centers);
    let refined = crate::exit::detached(py, || {
        lbf::refine_landmarks(&model, image(&pixels, rows, columns, stride), &centers, scale as f32, mirrored)
    })
    .py()?;
    Ok(refined.iter().map(|p| (p[0], p[1])).collect())
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(gather_features, m)?,
        wrap_pyfunction!(predict_stage, m)?,
        wrap_pyfunction!(refine_landmarks, m)?,
    ] {
        m.add_function(function)?;
    }
    Ok(())
}
