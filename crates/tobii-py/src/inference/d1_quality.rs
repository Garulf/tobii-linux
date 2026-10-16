//! tobii_linux.d1_quality: D1 confidence aggregation and the caller gate.

use pyo3::prelude::*;
use pyo3::types::PyInt;
use tobii_inference::d1_quality::{self, D1_IDS, QUALITY_MESSAGE};

use super::{InferenceResult, floats};

/// The aggregate score of float32 `quality` bytes of the given array shape.
#[pyfunction]
fn d1_quality_score(quality: &Bound<'_, PyAny>, shape: Vec<usize>, landmark_ids: &Bound<'_, PyAny>) -> PyResult<f64> {
    let ids: Option<Vec<(i64, i64)>> = landmark_ids.extract().ok();
    let (Some(ids), [_]) = (ids, shape.as_slice()) else {
        return Err(pyo3::exceptions::PyValueError::new_err(QUALITY_MESSAGE));
    };
    d1_quality::d1_quality_score(&floats(quality)?, &ids).map(f64::from).py()
}

/// The gate for `caller_mode` compared with Python equality, so 1.0 is mode 1.
#[pyfunction]
fn passes_d1_quality(py: Python<'_>, score: f64, caller_mode: &Bound<'_, PyAny>) -> PyResult<bool> {
    for mode in 0..3 {
        if caller_mode.eq(PyInt::new(py, mode))? {
            return Ok(d1_quality::passes_d1_quality(score as f32, mode));
        }
    }
    Ok(false)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("D1_IDS", D1_IDS.to_vec())?;
    m.add_function(wrap_pyfunction!(d1_quality_score, m)?)?;
    m.add_function(wrap_pyfunction!(passes_d1_quality, m)?)
}
