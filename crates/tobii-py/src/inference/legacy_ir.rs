//! tobii_linux.legacy_ir: the IR4/IR6 graphs translated into OpenVINO IR v11.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_inference::legacy_ir;

use super::InferenceResult;

/// `(name, xml, weights)` of the translated model directory.
#[pyfunction]
fn translate(py: Python<'_>, directory: PathBuf) -> PyResult<(Option<String>, String, Bound<'_, PyBytes>)> {
    let model = crate::exit::detached(py, || legacy_ir::load(&directory)).py()?;
    Ok((model.name, model.xml, PyBytes::new(py, &model.weights)))
}

/// `(library path, OpenVINO version)` of the runtime the native networks use.
#[pyfunction]
fn runtime(py: Python<'_>) -> PyResult<(PathBuf, String)> {
    let runtime = super::runtime(py)?;
    Ok((runtime.library().to_owned(), runtime.version().py()?))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(translate, m)?)?;
    m.add_function(wrap_pyfunction!(runtime, m)?)
}
