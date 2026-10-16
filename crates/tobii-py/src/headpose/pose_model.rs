//! tobii_linux.pose_model: configuration fields, latent encoding, the installed fitting
//! model and landmark selection.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::model::{self, FdpId};

use super::{HeadPoseResult, bytes, fixed, floats, model_tuple, pose_model, rows};

#[pyfunction]
fn numeric_config(path: PathBuf, key: &str, count: usize) -> PyResult<Vec<f64>> {
    model::numeric_config(&path, key, count).py()
}

#[pyfunction]
fn coefficient_latents<'py>(py: Python<'py>, coefficients: &[u8], ranges: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &model::coefficient_latents(&floats(coefficients)?, &rows::<2>(ranges)?).py()?))
}

/// PreparedPoseModel fields, as `pose_model._native_model` packs them.
#[pyfunction]
fn load_pose_model(py: Python<'_>, support_directory: PathBuf) -> PyResult<Bound<'_, PyAny>> {
    let model = crate::exit::detached(py, || model::load_pose_model(&support_directory)).py()?;
    model_tuple(py, &model)
}

#[pyfunction]
fn initial_state<'py>(py: Python<'py>, model: &Bound<'py, PyAny>, rigid_state: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &pose_model(model)?.initial_state(&fixed(rigid_state)?).py()?))
}

/// `(selected identifiers, indices into the model's points)`.
#[pyfunction]
fn select_pose_landmarks(model: &Bound<'_, PyAny>, present: Vec<(u32, u32)>) -> PyResult<(Vec<FdpId>, Vec<usize>)> {
    let (selected, indices) = model::select_pose_landmarks(&pose_model(model)?, &present).py()?;
    Ok((selected.landmark_ids, indices))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(numeric_config, m)?,
        wrap_pyfunction!(coefficient_latents, m)?,
        wrap_pyfunction!(load_pose_model, m)?,
        wrap_pyfunction!(initial_state, m)?,
        wrap_pyfunction!(select_pose_landmarks, m)?,
    ] {
        m.add_function(function)?;
    }
    Ok(())
}
