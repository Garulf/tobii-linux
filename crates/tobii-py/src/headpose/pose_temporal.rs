//! tobii_linux.pose_temporal: the temporal measures and the stateful temporal fitter.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
use tobii_headpose::Vec3;
use tobii_headpose::fit::FitMode;
use tobii_headpose::temporal::{self, TemporalFit, TemporalPoseFitter};

use super::{HeadPoseResult, bytes, fixed, flat_bytes, floats, pose_model, rows};

#[pyfunction]
fn facing_degrees(rigid_state: &[u8]) -> PyResult<f32> {
    Ok(temporal::facing_degrees(&fixed(rigid_state)?))
}

#[pyfunction]
fn pose_output(rigid_state: &[u8]) -> PyResult<(Vec3, Vec3)> {
    Ok(temporal::pose_output(&fixed(rigid_state)?))
}

#[pyfunction]
fn is_near_frontal(output_rotation: [f32; 2]) -> bool {
    temporal::is_near_frontal(output_rotation)
}

#[pyfunction]
fn eye_face_rectangle(
    points: &[u8],
    landmark_ids: Vec<(u32, u32)>,
    width: i64,
    height: i64,
) -> PyResult<Option<[i64; 4]>> {
    temporal::eye_face_rectangle(&rows::<2>(points)?, &landmark_ids, width, height).py()
}

#[pyfunction]
fn finalized_projection<'py>(
    py: Python<'py>,
    model: &Bound<'py, PyAny>,
    rigid_state: &[u8],
    shape_coefficients: &[u8],
    animation_coefficients: &[u8],
) -> PyResult<Bound<'py, PyBytes>> {
    let projected = temporal::finalized_projection(
        &pose_model(model)?,
        &fixed(rigid_state)?,
        &floats(shape_coefficients)?,
        &floats(animation_coefficients)?,
    )
    .py()?;
    Ok(flat_bytes(py, &projected))
}

#[pyfunction]
fn reprojection_error(
    landmark_ids: Vec<(u32, u32)>,
    projected: &[u8],
    flipped_points: &[u8],
    width: i64,
    height: i64,
    rectangle_width: f64,
) -> PyResult<f32> {
    let (projected, flipped) = (rows::<2>(projected)?, rows::<2>(flipped_points)?);
    if projected.len() < landmark_ids.len() || flipped.len() < landmark_ids.len() {
        return Err(super::invalid("Expected a projected and a record point per landmark"));
    }
    Ok(temporal::reprojection_error(&landmark_ids, &projected, &flipped, width, height, rectangle_width))
}

/// A frame's fit with float32 arrays as bytes; trace entries are dicts.
pub fn fit_result<'py>(py: Python<'py>, fit: &TemporalFit) -> PyResult<Bound<'py, PyDict>> {
    let trace = PyList::empty(py);
    for entry in &fit.trace {
        let item = PyDict::new(py);
        item.set_item("mode", entry.mode.flag())?;
        item.set_item("valid", entry.valid)?;
        item.set_item("attempts", entry.attempts)?;
        if let Some(measure) = entry.measure {
            item.set_item("facing", f64::from(measure.facing))?;
            item.set_item("error", f64::from(measure.error))?;
            item.set_item("rectangle", PyTuple::new(py, measure.rectangle)?)?;
        }
        trace.append(item)?;
    }
    let result = PyDict::new(py);
    result.set_item("valid", fit.valid)?;
    result.set_item("rotation", bytes(py, &fit.rotation))?;
    result.set_item("translation", bytes(py, &fit.translation))?;
    result.set_item("rigid_state", bytes(py, &fit.rigid_state))?;
    result.set_item("shape_coefficients", bytes(py, &fit.shape_coefficients))?;
    result.set_item("animation_coefficients", bytes(py, &fit.animation_coefficients))?;
    result.set_item("initialised", fit.initialised)?;
    result.set_item("frontal", fit.frontal)?;
    result.set_item("trace", trace)?;
    Ok(result)
}

#[pyclass(name = "TemporalPoseFitter", module = "tobii_linux.pose_temporal")]
struct PyTemporalPoseFitter(TemporalPoseFitter);

fn mode_flag(mode: Option<FitMode>) -> Option<u8> {
    mode.map(FitMode::flag)
}

#[pymethods]
impl PyTemporalPoseFitter {
    #[new]
    #[pyo3(signature = (model, accumulation="float64"))]
    fn new(model: &Bound<'_, PyAny>, accumulation: &str) -> PyResult<Self> {
        let accumulation = super::pose_fit::accumulation(accumulation)?;
        Ok(Self(TemporalPoseFitter::with_accumulation(pose_model(model)?, accumulation).py()?))
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    /// The frame result with float32 arrays as bytes; trace entries are dicts.
    fn process<'py>(
        &mut self,
        py: Python<'py>,
        points: &[u8],
        width: i64,
        height: i64,
    ) -> PyResult<Bound<'py, PyDict>> {
        let points = rows::<2>(points)?;
        let fitter = &mut self.0;
        let fit = crate::exit::detached(py, || fitter.process(&points, width, height)).py()?;
        fit_result(py, &fit)
    }

    #[getter]
    fn rigid_state<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        bytes(py, &self.0.rigid_state)
    }

    #[setter]
    fn set_rigid_state(&mut self, state: &[u8]) -> PyResult<()> {
        self.0.rigid_state = fixed(state)?;
        Ok(())
    }

    #[getter]
    fn initialised(&self) -> bool {
        self.0.initialised
    }

    #[setter]
    fn set_initialised(&mut self, value: bool) {
        self.0.initialised = value;
    }

    #[getter]
    fn frontal(&self) -> bool {
        self.0.frontal
    }

    #[setter]
    fn set_frontal(&mut self, value: bool) {
        self.0.frontal = value;
    }

    #[getter]
    fn shape<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        bytes(py, &self.0.shape)
    }

    #[setter]
    fn set_shape(&mut self, values: &[u8]) -> PyResult<()> {
        self.0.shape = floats(values)?;
        Ok(())
    }

    #[getter]
    fn animation<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        bytes(py, &self.0.animation)
    }

    #[setter]
    fn set_animation(&mut self, values: &[u8]) -> PyResult<()> {
        self.0.animation = floats(values)?;
        Ok(())
    }

    #[getter]
    fn projected<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        flat_bytes(py, &self.0.projected)
    }

    #[getter]
    fn fit_mode(&self) -> Option<u8> {
        mode_flag(self.0.fit_mode)
    }

    #[getter]
    fn fit_state<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.0.fit_state.as_ref().map(|state| bytes(py, state))
    }

    #[getter]
    fn order(&self) -> Vec<usize> {
        self.0.order.clone()
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(facing_degrees, m)?,
        wrap_pyfunction!(pose_output, m)?,
        wrap_pyfunction!(is_near_frontal, m)?,
        wrap_pyfunction!(eye_face_rectangle, m)?,
        wrap_pyfunction!(finalized_projection, m)?,
        wrap_pyfunction!(reprojection_error, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add_class::<PyTemporalPoseFitter>()
}
