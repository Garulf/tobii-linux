//! tobii_linux.pose_fit: the rigid, deformation and mode-specific fitting updates.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use tobii_headpose::fit::{self, Accumulation, Block, FitMode};

use super::{HeadPoseResult, basis_of, bytes, fixed, flat_bytes, floats, pose_model, rows};

pub fn accumulation(name: &str) -> PyResult<Accumulation> {
    match name {
        "float64" => Ok(Accumulation::Double),
        "float32" => Ok(Accumulation::Single),
        _ => Err(super::invalid("Accumulation must be float64 or float32")),
    }
}

#[pyfunction]
#[pyo3(signature = (vertices, observations, state, focal_factor, model_scale, state_sensitivity=None, observation_sensitivity=None))]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn rigid_step<'py>(
    py: Python<'py>,
    vertices: &[u8],
    observations: &[u8],
    state: &[u8],
    focal_factor: f64,
    model_scale: f64,
    state_sensitivity: Option<&[u8]>,
    observation_sensitivity: Option<&[u8]>,
) -> PyResult<Bound<'py, PyBytes>> {
    let state_sensitivity = state_sensitivity.map(floats).transpose()?;
    let observation_sensitivity = observation_sensitivity.map(floats).transpose()?;
    let updated = fit::rigid_step(
        &rows::<3>(vertices)?,
        &rows::<2>(observations)?,
        &fixed(state)?,
        focal_factor as f32,
        model_scale as f32,
        state_sensitivity.as_deref(),
        observation_sensitivity.as_deref(),
    )
    .py()?;
    Ok(bytes(py, &updated))
}

/// `(observations, sensitivity)` as bytes.
#[pyfunction]
fn prepare_observations<'py>(
    py: Python<'py>,
    points: &[u8],
    landmark_ids: Vec<(u32, u32)>,
    width: i64,
    height: i64,
) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyBytes>)> {
    let (observed, sensitivity) = fit::prepare_observations(&rows::<2>(points)?, &landmark_ids, width, height).py()?;
    Ok((flat_bytes(py, &observed), bytes(py, &sensitivity)))
}

#[pyfunction]
fn initial_rigid_state<'py>(
    py: Python<'py>,
    model_points: &[u8],
    image_points: &[u8],
    width: i64,
    height: i64,
    focal_factor: f64,
    neutral_yaw: f64,
) -> PyResult<Bound<'py, PyBytes>> {
    let model: [[f32; 3]; 4] =
        rows::<3>(model_points)?.try_into().map_err(|_| super::invalid("Expected four model points"))?;
    let image: [[f32; 2]; 4] =
        rows::<2>(image_points)?.try_into().map_err(|_| super::invalid("Expected four image points"))?;
    let state = fit::initial_rigid_state(&model, &image, width, height, focal_factor, neutral_yaw).py()?;
    Ok(bytes(py, &state))
}

/// A deformation block: `(basis bytes, count, ranges bytes, mask)`.
type BlockArgs<'a> = (&'a [u8], usize, &'a [u8], Option<Vec<bool>>);

#[pyfunction]
#[pyo3(signature = (vertices, observations, state, focal_factor, model_scale, shape, animation, state_sensitivity=None, observation_sensitivity=None, accumulation="float64"))]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn deformation_step<'py>(
    py: Python<'py>,
    vertices: &[u8],
    observations: &[u8],
    state: &[u8],
    focal_factor: f64,
    model_scale: f64,
    shape: BlockArgs<'_>,
    animation: BlockArgs<'_>,
    state_sensitivity: Option<&[u8]>,
    observation_sensitivity: Option<&[u8]>,
    accumulation: &str,
) -> PyResult<Bound<'py, PyBytes>> {
    let vertices = rows::<3>(vertices)?;
    let unpack = |(data, count, ranges, mask): BlockArgs<'_>| -> PyResult<_> {
        Ok((basis_of(data, vertices.len(), count)?, rows::<2>(ranges)?, mask))
    };
    let (b1, r1, m1) = unpack(shape)?;
    let (b2, r2, m2) = unpack(animation)?;
    let state_sensitivity = state_sensitivity.map(floats).transpose()?;
    let observation_sensitivity = observation_sensitivity.map(floats).transpose()?;
    let updated = fit::deformation_step(
        &vertices,
        &rows::<2>(observations)?,
        &floats(state)?,
        focal_factor as f32,
        model_scale as f32,
        [
            Block { basis: &b1, ranges: &r1, mask: m1.as_deref() },
            Block { basis: &b2, ranges: &r2, mask: m2.as_deref() },
        ],
        state_sensitivity.as_deref(),
        observation_sensitivity.as_deref(),
        self::accumulation(accumulation)?,
    )
    .py()?;
    Ok(bytes(py, &updated))
}

/// The mode step's dict of float32 bytes: rigid_state, shape_coefficients,
/// animation_coefficients and active_state.
#[pyfunction]
#[pyo3(signature = (model, observations, rigid_state, shape_coefficients, animation_coefficients, fitting_mode, observation_sensitivity=None, active_state=None, accumulation="float64"))]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn mode_step<'py>(
    py: Python<'py>,
    model: &Bound<'py, PyAny>,
    observations: &[u8],
    rigid_state: &[u8],
    shape_coefficients: &[u8],
    animation_coefficients: &[u8],
    fitting_mode: i64,
    observation_sensitivity: Option<&[u8]>,
    active_state: Option<&[u8]>,
    accumulation: &str,
) -> PyResult<Bound<'py, PyDict>> {
    let mode = FitMode::from_flag(fitting_mode).py()?;
    let model = pose_model(model)?;
    let observation_sensitivity = observation_sensitivity.map(floats).transpose()?;
    let active_state = active_state.map(floats).transpose()?;
    let step = fit::mode_step(
        &model,
        &rows::<2>(observations)?,
        &fixed(rigid_state)?,
        &floats(shape_coefficients)?,
        &floats(animation_coefficients)?,
        mode,
        observation_sensitivity.as_deref(),
        active_state.as_deref(),
        self::accumulation(accumulation)?,
    )
    .py()?;
    let result = PyDict::new(py);
    result.set_item("rigid_state", bytes(py, &step.rigid_state))?;
    result.set_item("shape_coefficients", bytes(py, &step.shape_coefficients))?;
    result.set_item("animation_coefficients", bytes(py, &step.animation_coefficients))?;
    result.set_item("active_state", bytes(py, &step.active_state))?;
    Ok(result)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(rigid_step, m)?,
        wrap_pyfunction!(prepare_observations, m)?,
        wrap_pyfunction!(initial_rigid_state, m)?,
        wrap_pyfunction!(deformation_step, m)?,
        wrap_pyfunction!(mode_step, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("NEUTRAL_YAW", fit::NEUTRAL_YAW)?;
    Ok(())
}
