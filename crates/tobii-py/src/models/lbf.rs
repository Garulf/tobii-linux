//! tobii_linux.lbf: the fa/fc LBF and LBF/lv model files.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyTuple};
use tobii_models::lbf::{Forest, LbfModel, LvModel, Matrix};

use super::ModelResult;
use crate::convert::bytes_like;

fn forest_tuple<'py>(py: Python<'py>, forest: &Forest) -> PyResult<Bound<'py, PyTuple>> {
    let trees = forest.trees.iter().map(|t| (t.marker, t.word, t.depth, PyBytes::new(py, &t.payload)));
    (forest.first, forest.scale_bits, PyList::new(py, trees)?).into_pyobject(py)
}

/// `(inputs, outputs, biases, weights, has_bias, codebook_max_abs)` with float32 bytes.
fn matrix_tuple<'py>(py: Python<'py>, matrix: &Matrix) -> PyResult<Bound<'py, PyTuple>> {
    let floats = |values: &[f32]| PyBytes::new(py, &values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    let (biases, weights) = (floats(&matrix.biases), floats(&matrix.weights));
    (matrix.inputs, matrix.outputs, biases, weights, matrix.has_bias, matrix.codebook_max_abs.clone()).into_pyobject(py)
}

/// `(version, points, tree_count, forest_end, stages, source_bytes)`; each point is
/// `(id, xy, forests)`.
#[pyfunction]
fn read_lbf(py: Python<'_>, path: PathBuf) -> PyResult<Bound<'_, PyTuple>> {
    let model = crate::exit::detached(py, || LbfModel::read(&path)).py()?;
    let points = model
        .points
        .iter()
        .map(|p| {
            let forests = p.submodels.iter().map(|f| forest_tuple(py, f)).collect::<PyResult<Vec<_>>>()?;
            (p.id, (p.xy[0], p.xy[1]), forests).into_pyobject(py)
        })
        .collect::<PyResult<Vec<_>>>()?;
    let stages = model
        .stages
        .iter()
        .map(|stage| stage.iter().map(|m| matrix_tuple(py, m)).collect::<PyResult<Vec<_>>>())
        .collect::<PyResult<Vec<_>>>()?;
    (model.version, points, model.tree_count, model.forest_end, stages, model.source_bytes).into_pyobject(py)
}

/// `(version, records, source_bytes)`; each record is `(id, forest, matrix_offset, matrix)`.
#[pyfunction]
fn read_lv(py: Python<'_>, path: PathBuf) -> PyResult<Bound<'_, PyTuple>> {
    let model = crate::exit::detached(py, || LvModel::read(&path)).py()?;
    let records = model
        .records
        .iter()
        .map(|r| {
            let forest = r.forest.as_ref().map(|f| forest_tuple(py, f)).transpose()?;
            (r.id, forest, r.matrix_offset, matrix_tuple(py, &r.matrix)?).into_pyobject(py)
        })
        .collect::<PyResult<Vec<_>>>()?;
    (model.version, records, model.source_bytes).into_pyobject(py)
}

/// `(int8 weights, float32 scales)`. The maxima must be float32 values, as the readers
/// produce them; other doubles are rounded to float32 first.
#[pyfunction]
fn quantize_matrix<'py>(
    py: Python<'py>,
    inputs: usize,
    outputs: usize,
    weights: &Bound<'py, PyAny>,
    codebook_max_abs: Option<Vec<f32>>,
) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyBytes>)> {
    let weights = bytes_like(weights)?;
    let weights = weights.as_chunks::<4>().0.iter().map(|w| f32::from_le_bytes(*w)).collect();
    let matrix = Matrix { inputs, outputs, biases: vec![], weights, has_bias: true, codebook_max_abs };
    let (quantized, scales) = matrix.quantize().py()?;
    let quantized: Vec<u8> = quantized.iter().map(|&q| q as u8).collect();
    let scales: Vec<u8> = scales.iter().flat_map(|s| s.to_le_bytes()).collect();
    Ok((PyBytes::new(py, &quantized), PyBytes::new(py, &scales)))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in
        [wrap_pyfunction!(read_lbf, m)?, wrap_pyfunction!(read_lv, m)?, wrap_pyfunction!(quantize_matrix, m)?]
    {
        m.add_function(function)?;
    }
    Ok(())
}
