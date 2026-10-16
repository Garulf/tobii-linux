//! tobii_linux.calibration: calibration targets, gaze focus and attention rules, backups and
//! the calibration sequence as a resumable operation.
//!
//! Samples cross the boundary as (timestamp_us, point) tuples and calibrations as
//! (calibration_id, blob); the Python module keeps its dataclasses and drives operations on
//! its session and UI.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pyo3::IntoPyObjectExt;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};
use tobii_gaze::Operation;
use tobii_gaze::calibration::{
    self, AttentionWatch, BackupCheck, CalibrationOutcome, Calibrator, FocusCheck, GazeFocus, GazeSample, Progress,
    UiEvent,
};
use tobii_gaze::device::LocalCalibration;
use tobii_protocol::gaze::gaze_message;

use crate::errors::IntoPyResult;
use crate::gaze_device::{local_arg, local_tuple, point_tuple};
use crate::gaze_operation::{CalibrationError, IntoGazeResult, PyOperation, StimulusTuple, stimulus_tuple, values_arg};
use crate::ttp::message_arg;

type Point = (f64, f64);
type SampleTuple = (i64, Option<Point>);

fn pair([x, y]: [f64; 2]) -> Point {
    (x, y)
}

fn sample_tuple(sample: GazeSample) -> SampleTuple {
    (sample.timestamp_us, sample.point.map(pair))
}

fn sample_arg(sample: &Bound<'_, PyAny>) -> PyResult<GazeSample> {
    Ok(GazeSample {
        timestamp_us: sample.getattr("timestamp_us")?.extract()?,
        point: sample.getattr("point")?.extract()?,
    })
}

fn samples_arg(samples: &Bound<'_, PyAny>) -> PyResult<Vec<GazeSample>> {
    samples.try_iter()?.map(|s| sample_arg(&s?)).collect()
}

pub fn ui_arguments<'py>(py: Python<'py>, event: UiEvent) -> PyResult<Bound<'py, PyTuple>> {
    match event {
        UiEvent::ShowPoint { index, x, y } => (index, x, y).into_pyobject(py),
        UiEvent::PointDone { index, accepted } => (index, accepted).into_pyobject(py),
        UiEvent::Focus { index, focused } => (index, focused).into_pyobject(py),
        UiEvent::ShowTargets { stage, targets } => (stage, targets).into_pyobject(py),
        UiEvent::StageDone { stage } | UiEvent::Computing { stage } => (stage,).into_pyobject(py),
        UiEvent::Restarting { attempt } | UiEvent::CalibrationStarted { attempt } => (attempt,).into_pyobject(py),
    }
}

#[pyfunction]
fn calibration_area(display_mm: [f64; 2]) -> (f64, f64, f64, f64) {
    let [x, y, w, h] = calibration::calibration_area(display_mm);
    (x, y, w, h)
}

#[pyfunction]
fn calibration_targets(display_mm: Option<[f64; 2]>, points: Vec<[f64; 2]>) -> Vec<Point> {
    calibration::calibration_targets(display_mm, &points).into_iter().map(pair).collect()
}

#[pyfunction]
fn display_size_mm(values: &Bound<'_, PyAny>) -> PyResult<Point> {
    Ok(pair(calibration::display_size_mm(&values_arg(values)?).gaze()?))
}

#[pyfunction]
fn save_backup(path: PathBuf, calibration: &Bound<'_, PyAny>, overwrite: bool) -> PyResult<PathBuf> {
    calibration::save_backup(&path, &local_arg(calibration)?, overwrite).gaze()
}

#[pyfunction]
fn load_backup(py: Python<'_>, path: PathBuf) -> PyResult<(u32, Bound<'_, PyBytes>)> {
    Ok(local_tuple(py, &calibration::load_backup(&path).gaze()?))
}

#[pyfunction]
fn gaze_sample(message: &Bound<'_, PyAny>) -> PyResult<Option<SampleTuple>> {
    let gaze = gaze_message(&message_arg(message)?).py()?;
    Ok(gaze.map(|g| sample_tuple(calibration::gaze_sample(&g))))
}

#[pyfunction]
fn binocular_sample(message: &Bound<'_, PyAny>) -> PyResult<Option<SampleTuple>> {
    let gaze = gaze_message(&message_arg(message)?).py()?;
    Ok(gaze.map(|g| sample_tuple(calibration::binocular_sample(&g))))
}

#[pyfunction]
fn fixation(samples: &Bound<'_, PyAny>, window_ms: f64, spread: f64, min_samples: usize) -> PyResult<bool> {
    Ok(calibration::fixation(&samples_arg(samples)?, window_ms, spread, min_samples))
}

#[pyfunction]
fn zone_radius(screen_px: [f64; 2], targets: Vec<[f64; 2]>) -> PyResult<f64> {
    FocusCheck { screen_px }
        .zone_radius(&targets)
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("min() arg is an empty sequence"))
}

/// The index of the closest target within `radius`, or -1.
#[pyfunction]
fn closest(screen_px: [f64; 2], point: [f64; 2], targets: Vec<[f64; 2]>, radius: f64) -> i64 {
    FocusCheck { screen_px }.closest(point, &targets, radius).map_or(-1, |index| index as i64)
}

#[pyfunction]
fn focus_changed(
    screen_px: [f64; 2],
    samples: &Bound<'_, PyAny>,
    targets: Vec<[f64; 2]>,
    focused: usize,
) -> PyResult<bool> {
    Ok(FocusCheck { screen_px }.focus_changed(&samples_arg(samples)?, &targets, focused))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[pyclass(frozen, name = "GazeFocus", module = "tobii_linux._native.calibration")]
pub struct PyGazeFocus(Mutex<GazeFocus>);

impl PyGazeFocus {
    fn with<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut GazeFocus) -> R + Send) -> R {
        crate::exit::detached(py, || f(&mut lock(&self.0)))
    }
}

#[pymethods]
impl PyGazeFocus {
    #[new]
    fn new(screen_px: [f64; 2], stage_targets: Vec<[f64; 2]>) -> Self {
        Self(Mutex::new(GazeFocus::new(FocusCheck { screen_px }, &stage_targets)))
    }

    /// `candidates`: (index, (x, y)) pairs still to collect, in order.
    fn observe(
        &self,
        py: Python<'_>,
        sample: &Bound<'_, PyAny>,
        candidates: Vec<(usize, [f64; 2])>,
    ) -> PyResult<Option<usize>> {
        let sample = sample_arg(sample)?;
        Ok(self.with(py, |focus| focus.observe(&sample, &candidates)))
    }

    #[getter]
    fn radius(&self, py: Python<'_>) -> Option<f64> {
        self.with(py, |focus| focus.radius)
    }

    #[getter]
    fn focused(&self, py: Python<'_>) -> Option<usize> {
        self.with(py, |focus| focus.focused)
    }

    #[setter]
    fn set_focused(&self, py: Python<'_>, focused: Option<usize>) {
        self.with(py, |focus| focus.focused = focused);
    }

    #[getter]
    fn last_focus_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |focus| focus.last_focus_us)
    }

    #[getter]
    fn last_valid_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |focus| focus.last_valid_us)
    }
}

#[pyclass(frozen, name = "AttentionWatch", module = "tobii_linux._native.calibration")]
pub struct PyAttentionWatch(Mutex<AttentionWatch>);

impl PyAttentionWatch {
    fn with<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut AttentionWatch) -> R + Send) -> R {
        crate::exit::detached(py, || f(&mut lock(&self.0)))
    }
}

#[pymethods]
impl PyAttentionWatch {
    #[new]
    #[pyo3(signature = (stall_ms=calibration::ATTENTION_STALL_MS, no_pop_ms=calibration::ATTENTION_NO_POP_MS))]
    fn new(stall_ms: f64, no_pop_ms: f64) -> Self {
        Self(Mutex::new(AttentionWatch::new(stall_ms, no_pop_ms)))
    }

    fn observe(&self, py: Python<'_>, sample: &Bound<'_, PyAny>, looking_at_waiting_target: bool) -> PyResult<()> {
        let sample = sample_arg(sample)?;
        self.with(py, |watch| watch.observe(&sample, looking_at_waiting_target));
        Ok(())
    }

    fn popped(&self, py: Python<'_>) {
        self.with(py, AttentionWatch::popped);
    }

    #[getter]
    fn stalled(&self, py: Python<'_>) -> bool {
        self.with(py, |watch| watch.stalled())
    }

    #[getter]
    fn last_sample_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |watch| watch.last_sample_us)
    }

    #[getter]
    fn last_attention_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |watch| watch.last_attention_us)
    }

    #[getter]
    fn last_pop_us(&self, py: Python<'_>) -> Option<i64> {
        self.with(py, |watch| watch.last_pop_us)
    }
}

fn calibration_object(py: Python<'_>, calibration: LocalCalibration) -> PyResult<Py<PyAny>> {
    local_tuple(py, &calibration).into_py_any(py)
}

fn check_tuple(check: &BackupCheck) -> (u32, u32, String, String) {
    (check.file_id, check.device_id, check.file_blob_sha256.clone(), check.device_blob_sha256.clone())
}

#[pyfunction]
fn read_display_size() -> PyOperation {
    let operation = Operation::new(|io| async move { calibration::read_display_size(&io).await });
    PyOperation::new(operation, |py, size| pair(size).into_py_any(py))
}

#[pyfunction]
fn read_device_calibration_id() -> PyOperation {
    let operation = Operation::new(|io| async move { calibration::read_device_calibration_id(&io).await });
    PyOperation::new(operation, |py, id| id.into_py_any(py))
}

#[pyfunction]
fn retrieve_blob() -> PyOperation {
    let operation = Operation::new(|io| async move { calibration::retrieve_blob(&io).await });
    PyOperation::new(operation, |py, blob| PyBytes::new(py, &blob).into_py_any(py))
}

#[pyfunction]
fn read_stimulus_points() -> PyOperation {
    let operation = Operation::new(|io| async move { calibration::read_stimulus_points(&io).await });
    PyOperation::new(operation, |py, points| points.iter().map(stimulus_tuple).collect::<Vec<_>>().into_py_any(py))
}

#[pyfunction]
fn backup_calibration() -> PyOperation {
    let operation = Operation::new(|io| async move { calibration::backup_calibration(&io).await });
    PyOperation::new(operation, calibration_object)
}

#[pyfunction]
fn verify_backup(calibration: &Bound<'_, PyAny>) -> PyResult<PyOperation> {
    let calibration = local_arg(calibration)?;
    let operation = Operation::new(move |io| async move { calibration::verify_backup(&io, &calibration).await });
    Ok(PyOperation::new(operation, |py, check| check_tuple(&check).into_py_any(py)))
}

#[pyfunction]
fn restore_calibration(calibration: &Bound<'_, PyAny>) -> PyResult<PyOperation> {
    let calibration = local_arg(calibration)?;
    let operation = Operation::new(move |io| async move { calibration::restore_calibration(&io, &calibration).await });
    Ok(PyOperation::new(operation, |py, id| id.into_py_any(py)))
}

/// What a running calibration has done so far.
#[pyclass(frozen, name = "CalibrationProgress", module = "tobii_linux._native.calibration")]
pub struct PyCalibrationProgress(Arc<Mutex<Progress>>);

impl PyCalibrationProgress {
    fn snapshot(&self, py: Python<'_>) -> Progress {
        crate::exit::detached(py, || lock(&self.0).clone())
    }
}

#[pymethods]
impl PyCalibrationProgress {
    #[getter]
    fn step(&self, py: Python<'_>) -> String {
        self.snapshot(py).step
    }

    #[getter]
    fn discarded(&self, py: Python<'_>) -> Vec<(usize, usize)> {
        self.snapshot(py).discarded
    }

    #[getter]
    fn compute_failures(&self, py: Python<'_>) -> Vec<(usize, usize, Vec<StimulusTuple>)> {
        let failures = self.snapshot(py).compute_failures;
        failures.iter().map(|f| (f.stage, f.attempt, f.stimulus_points.iter().map(stimulus_tuple).collect())).collect()
    }

    #[getter]
    fn stalled_runs(&self, py: Python<'_>) -> usize {
        self.snapshot(py).stalled_runs
    }
}

fn outcome_object(py: Python<'_>, outcome: CalibrationOutcome) -> PyResult<Py<PyAny>> {
    let calibration = local_tuple(py, &outcome.calibration);
    let stimulus: Vec<StimulusTuple> = outcome.stimulus_points.iter().map(stimulus_tuple).collect();
    let points: Vec<_> = outcome.points.iter().map(point_tuple).collect();
    (calibration, stimulus, points, outcome.discarded).into_py_any(py)
}

/// The calibration sequence and its progress; see `tobii_gaze::calibration::Calibrator`.
#[pyfunction]
#[pyo3(signature = (targets, stages, previous, screen_px, max_attempts, subscribe_gaze, restarts, wait_for_fixation,
                    choose_by_gaze, max_stalled_runs, attention_stall_ms, attention_no_pop_ms))]
#[allow(clippy::too_many_arguments, reason = "mirrors the Calibrator constructor's keyword arguments")]
fn calibrator(
    targets: Vec<[f64; 2]>,
    stages: Vec<Vec<usize>>,
    previous: Option<&Bound<'_, PyAny>>,
    screen_px: Option<[f64; 2]>,
    max_attempts: usize,
    subscribe_gaze: bool,
    restarts: usize,
    wait_for_fixation: bool,
    choose_by_gaze: bool,
    max_stalled_runs: usize,
    attention_stall_ms: f64,
    attention_no_pop_ms: f64,
) -> PyResult<(PyOperation, PyCalibrationProgress)> {
    let config = Calibrator {
        targets,
        stages,
        previous: previous.map(local_arg).transpose()?,
        focus: screen_px.map(|screen_px| FocusCheck { screen_px }),
        max_attempts,
        subscribe_gaze,
        restarts,
        wait_for_fixation,
        choose_by_gaze,
        max_stalled_runs,
        attention_stall_ms,
        attention_no_pop_ms,
    };
    let (operation, progress) = config.operation();
    Ok((PyOperation::new(operation, outcome_object), PyCalibrationProgress(progress)))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("CalibrationError", py.get_type::<CalibrationError>())?;
    m.add("INTERACTIVE_POINTS", calibration::INTERACTIVE_POINTS.map(pair).to_vec())?;
    m.add("STAGES", calibration::STAGES.map(<[usize]>::to_vec).to_vec())?;
    m.add("READ_COMMANDS", calibration::READ_COMMANDS.to_vec())?;
    m.add_class::<PyGazeFocus>()?;
    m.add_class::<PyAttentionWatch>()?;
    m.add_class::<PyCalibrationProgress>()?;
    m.add_function(wrap_pyfunction!(calibration_area, m)?)?;
    m.add_function(wrap_pyfunction!(calibration_targets, m)?)?;
    m.add_function(wrap_pyfunction!(display_size_mm, m)?)?;
    m.add_function(wrap_pyfunction!(save_backup, m)?)?;
    m.add_function(wrap_pyfunction!(load_backup, m)?)?;
    m.add_function(wrap_pyfunction!(gaze_sample, m)?)?;
    m.add_function(wrap_pyfunction!(binocular_sample, m)?)?;
    m.add_function(wrap_pyfunction!(fixation, m)?)?;
    m.add_function(wrap_pyfunction!(zone_radius, m)?)?;
    m.add_function(wrap_pyfunction!(closest, m)?)?;
    m.add_function(wrap_pyfunction!(focus_changed, m)?)?;
    m.add_function(wrap_pyfunction!(read_display_size, m)?)?;
    m.add_function(wrap_pyfunction!(read_device_calibration_id, m)?)?;
    m.add_function(wrap_pyfunction!(retrieve_blob, m)?)?;
    m.add_function(wrap_pyfunction!(read_stimulus_points, m)?)?;
    m.add_function(wrap_pyfunction!(backup_calibration, m)?)?;
    m.add_function(wrap_pyfunction!(verify_backup, m)?)?;
    m.add_function(wrap_pyfunction!(restore_calibration, m)?)?;
    m.add_function(wrap_pyfunction!(calibrator, m)?)?;
    Ok(())
}
