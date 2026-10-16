//! tobii_linux.eye_positioning: the eye positioning calculator and its hint and advance rules.
//!
//! Statuses cross the boundary as their string values; the Python module keeps the Status
//! enum and the GuideSample dataclass.

use std::sync::{Mutex, MutexGuard, PoisonError};

use pyo3::prelude::*;
use pyo3::types::PyDict;
use tobii_gaze::positioning::{self, EyeView, EyesPositioning, EyesView, GuideSample, PositioningGuide, Status};

use crate::gaze::device_arg;

type Triple = (f64, f64, f64);

fn sample_arg(sample: &Bound<'_, PyAny>) -> PyResult<GuideSample> {
    Ok(GuideSample {
        timestamp_us: sample.getattr("timestamp_us")?.extract()?,
        left: sample.getattr("left")?.extract()?,
        right: sample.getattr("right")?.extract()?,
    })
}

fn pair(point: Option<[f64; 2]>) -> Option<(f64, f64)> {
    point.map(|[x, y]| (x, y))
}

fn eye_dict<'py>(py: Python<'py>, eye: Option<EyeView>) -> PyResult<Option<Bound<'py, PyDict>>> {
    let Some(eye) = eye else {
        return Ok(None);
    };
    let view = PyDict::new(py);
    view.set_item("x", eye.x)?;
    view.set_item("y", eye.y)?;
    view.set_item("grey", eye.grey)?;
    view.set_item("opacity", eye.opacity)?;
    Ok(Some(view))
}

fn eyes_dict<'py>(py: Python<'py>, eyes: EyesView) -> PyResult<Bound<'py, PyDict>> {
    let view = PyDict::new(py);
    view.set_item("left", eye_dict(py, eyes.left)?)?;
    view.set_item("right", eye_dict(py, eyes.right)?)?;
    view.set_item("size", eyes.size)?;
    Ok(view)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[pyclass(frozen, name = "EyesPositioning", module = "tobii_linux._native.eye_positioning")]
pub struct PyEyesPositioning(Mutex<EyesPositioning>);

impl PyEyesPositioning {
    fn with<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut EyesPositioning) -> R + Send) -> R {
        crate::exit::detached(py, || f(&mut lock(&self.0)))
    }
}

#[pymethods]
impl PyEyesPositioning {
    #[new]
    fn new() -> Self {
        Self(Mutex::new(EyesPositioning::new()))
    }

    fn update(&self, py: Python<'_>, sample: &Bound<'_, PyAny>) -> PyResult<&'static str> {
        let sample = sample_arg(sample)?;
        Ok(self.with(py, |c| c.update(&sample)).value())
    }

    fn eyes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let eyes = self.with(py, |c| c.eyes());
        eyes_dict(py, eyes)
    }

    #[getter]
    fn left_history(&self, py: Python<'_>) -> Vec<Option<(f64, f64)>> {
        self.with(py, |c| c.left_history.iter().map(|p| pair(*p)).collect())
    }

    #[getter]
    fn right_history(&self, py: Python<'_>) -> Vec<Option<(f64, f64)>> {
        self.with(py, |c| c.right_history.iter().map(|p| pair(*p)).collect())
    }

    #[getter]
    fn distances(&self, py: Python<'_>) -> Vec<f64> {
        self.with(py, |c| c.distances.iter().copied().collect())
    }

    #[getter]
    fn left(&self, py: Python<'_>) -> Option<(f64, f64)> {
        self.with(py, |c| pair(c.left))
    }

    #[getter]
    fn right(&self, py: Python<'_>) -> Option<(f64, f64)> {
        self.with(py, |c| pair(c.right))
    }

    #[getter]
    fn size(&self, py: Python<'_>) -> i64 {
        self.with(py, |c| c.size)
    }

    #[getter]
    fn status(&self, py: Python<'_>) -> &'static str {
        self.with(py, |c| c.status.value())
    }
}

#[pyclass(frozen, name = "PositioningGuide", module = "tobii_linux._native.eye_positioning")]
pub struct PyPositioningGuide(Mutex<PositioningGuide>);

impl PyPositioningGuide {
    fn with<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut PositioningGuide) -> R + Send) -> R {
        crate::exit::detached(py, || f(&mut lock(&self.0)))
    }
}

#[pymethods]
impl PyPositioningGuide {
    #[new]
    fn new() -> Self {
        Self(Mutex::new(PositioningGuide::new()))
    }

    fn update(&self, py: Python<'_>, sample: &Bound<'_, PyAny>) -> PyResult<&'static str> {
        let sample = sample_arg(sample)?;
        Ok(self.with(py, |g| g.update(&sample)))
    }

    /// A copy of the calculator's current state.
    #[getter]
    fn calculator(&self, py: Python<'_>) -> PyEyesPositioning {
        PyEyesPositioning(Mutex::new(self.with(py, |g| g.calculator.clone())))
    }

    fn eyes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let eyes = self.with(py, |g| g.calculator.eyes());
        eyes_dict(py, eyes)
    }

    #[getter]
    fn hint(&self, py: Python<'_>) -> &'static str {
        self.with(py, |g| g.hint)
    }

    #[getter]
    fn good(&self, py: Python<'_>) -> bool {
        self.with(py, |g| g.good())
    }

    #[getter]
    fn previous(&self, py: Python<'_>) -> &'static str {
        self.with(py, |g| g.previous.value())
    }

    #[getter]
    fn pending(&self, py: Python<'_>) -> u32 {
        self.with(py, |g| g.pending)
    }

    #[getter]
    fn lost_since_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |g| g.lost_since_us)
    }

    #[getter]
    fn start_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |g| g.start_us)
    }

    #[getter]
    fn present_since_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |g| g.present_since_us)
    }

    #[getter]
    fn absent_since_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |g| g.absent_since_us)
    }

    fn presence_ms(&self, py: Python<'_>, now: i64) -> f64 {
        self.with(py, |g| g.presence_ms(now))
    }

    fn ready_to_advance(&self, py: Python<'_>, now: i64) -> bool {
        self.with(py, |g| g.ready_to_advance(now))
    }
}

#[pyfunction]
fn guide_sample(g: &Bound<'_, PyAny>) -> PyResult<(i64, Option<Triple>, Option<Triple>)> {
    let sample = positioning::guide_sample(&device_arg(g)?);
    let triple = |eye: Option<[f64; 3]>| eye.map(|[x, y, z]| (x, y, z));
    Ok((sample.timestamp_us, triple(sample.left), triple(sample.right)))
}

#[pyfunction]
fn mirrored(position: Vec<f64>) -> PyResult<(f64, f64)> {
    let (&x, &y) = position
        .first()
        .zip(position.get(1))
        .ok_or_else(|| pyo3::exceptions::PyIndexError::new_err("tuple index out of range"))?;
    let [x, y] = positioning::mirrored([x, y, 0.0]);
    Ok((x, y))
}

#[pyfunction]
fn eye_size(distance: f64) -> i64 {
    positioning::eye_size(distance)
}

#[pyfunction]
fn eye_grey(size: i64) -> i64 {
    positioning::eye_grey(size)
}

#[pyfunction]
fn eye_opacity(position: Option<[f64; 2]>) -> f64 {
    positioning::eye_opacity(position)
}

#[pyfunction]
fn extrapolated(history: Vec<Option<[f64; 2]>>) -> Option<(f64, f64)> {
    pair(positioning::extrapolated(&history))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("STATUSES", Status::ALL.map(|status| (status.value(), status.hint())).to_vec())?;
    m.add_class::<PyEyesPositioning>()?;
    m.add_class::<PyPositioningGuide>()?;
    m.add_function(wrap_pyfunction!(guide_sample, m)?)?;
    m.add_function(wrap_pyfunction!(mirrored, m)?)?;
    m.add_function(wrap_pyfunction!(eye_size, m)?)?;
    m.add_function(wrap_pyfunction!(eye_grey, m)?)?;
    m.add_function(wrap_pyfunction!(eye_opacity, m)?)?;
    m.add_function(wrap_pyfunction!(extrapolated, m)?)?;
    Ok(())
}
