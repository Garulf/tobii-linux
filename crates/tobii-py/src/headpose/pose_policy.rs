//! tobii_linux.pose_policy: the bounds check and the bounded retry fitting policies.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::policy;

use super::{HeadPoseResult, bytes, fixed, floats, pose_model, rows};

fn limits(data: &[u8]) -> PyResult<[[f32; 2]; 3]> {
    rows::<2>(data)?.try_into().map_err(|_| super::invalid("Invalid pose or bound dimensions"))
}

/// `(invalid, shape, animation)`.
#[pyfunction]
fn check_pose_bounds<'py>(
    py: Python<'py>,
    rigid_state: &[u8],
    shape_coefficients: &[u8],
    animation_coefficients: &[u8],
    rotation_limits: &[u8],
    translation_limits: &[u8],
) -> PyResult<(bool, Bound<'py, PyBytes>, Bound<'py, PyBytes>)> {
    let (invalid, shape, animation) = policy::check_pose_bounds(
        &fixed(rigid_state)?,
        &floats(shape_coefficients)?,
        &floats(animation_coefficients)?,
        &limits(rotation_limits)?,
        &limits(translation_limits)?,
    )
    .py()?;
    Ok((invalid, bytes(py, &shape), bytes(py, &animation)))
}

type Fit<'py> = (Bound<'py, PyBytes>, Bound<'py, PyBytes>, Bound<'py, PyBytes>, bool, u32, u32);

/// `(state, shape, animation, valid, attempts, updates)`.
#[pyfunction]
#[pyo3(signature = (model, points, width, height, state=None, iterations=3))]
fn fit_landmarks<'py>(
    py: Python<'py>,
    model: &Bound<'py, PyAny>,
    points: &[u8],
    width: i64,
    height: i64,
    state: Option<&[u8]>,
    iterations: i64,
) -> PyResult<Fit<'py>> {
    let iterations =
        u32::try_from(iterations).map_err(|_| super::invalid("Expected a bounded nonnegative iteration count"))?;
    let model = pose_model(model)?;
    let points = rows::<2>(points)?;
    let state = state.map(floats).transpose()?;
    let fit = crate::exit::detached(py, || {
        policy::fit_landmarks(&model, &points, width, height, state.as_deref(), iterations)
    })
    .py()?;
    Ok((
        bytes(py, &fit.state),
        bytes(py, &fit.shape_coefficients),
        bytes(py, &fit.animation_coefficients),
        fit.valid,
        fit.attempts,
        fit.updates,
    ))
}

type Staged<'py> = (Bound<'py, PyBytes>, Bound<'py, PyBytes>, Bound<'py, PyBytes>, bool, Vec<(u8, u32, u32, bool)>);

/// `(rigid_state, shape, animation, valid, [(mode, updates, attempts, valid)])`.
#[pyfunction]
fn fit_staged_landmarks<'py>(
    py: Python<'py>,
    model: &Bound<'py, PyAny>,
    points: &[u8],
    width: i64,
    height: i64,
) -> PyResult<Staged<'py>> {
    let model = pose_model(model)?;
    let points = rows::<2>(points)?;
    let fit = crate::exit::detached(py, || policy::fit_staged_landmarks(&model, &points, width, height)).py()?;
    let stages = fit.stages.iter().map(|s| (s.mode.flag(), s.updates, s.attempts, s.valid)).collect();
    Ok((
        bytes(py, &fit.rigid_state),
        bytes(py, &fit.shape_coefficients),
        bytes(py, &fit.animation_coefficients),
        fit.valid,
        stages,
    ))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(check_pose_bounds, m)?,
        wrap_pyfunction!(fit_landmarks, m)?,
        wrap_pyfunction!(fit_staged_landmarks, m)?,
    ] {
        m.add_function(function)?;
    }
    Ok(())
}
