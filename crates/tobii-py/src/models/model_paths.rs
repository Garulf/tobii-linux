//! tobii_linux.model_paths: where the models are looked for by default.

use std::path::PathBuf;

use pyo3::prelude::*;

#[pyfunction]
fn default_models_dir() -> PathBuf {
    tobii_models::paths::default_models_dir()
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(default_models_dir, m)?)
}
