//! tobii_linux.qpc_clock: the service clock that follows Wine's QueryPerformanceCounter.
//!
//! `ServiceClock` reads the host clocks natively, or through a Python `read_us(name)`
//! callable (tests and synthetic hosts), which is called with the interpreter attached and
//! no native lock held. Verdicts and samples cross as tuples and dicts keyed by clock name;
//! the Python module wraps them in its dataclasses.

use std::sync::{Mutex, MutexGuard};

use pyo3::exceptions::{PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyTuple};
use tobii_service::qpc_clock::{
    self as qpc, ClassifierOptions, ClockReader, ClockState, ClockSwitch, ClockVerdict, CounterMapping, LinuxClock,
    LinuxClocks, QpcClockClassifier, QpcClockFollower, QpcSample, ServiceClock,
};

use crate::convert::bytes_like;
use crate::errors::IntoPyResult;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn clock_name(name: &str) -> PyResult<LinuxClock> {
    LinuxClock::from_name(name).ok_or_else(|| PyKeyError::new_err(name.to_string()))
}

fn optional_clock(name: Option<&str>) -> PyResult<Option<LinuxClock>> {
    name.map(clock_name).transpose()
}

/// The host clocks, or a Python `read_us(name)` callable.
enum Reader {
    Linux(LinuxClocks),
    Python(Py<PyAny>),
}

impl ClockReader for Reader {
    type Error = PyErr;

    fn read_us(&self, clock: LinuxClock) -> PyResult<i64> {
        match self {
            Reader::Linux(clocks) => {
                let Ok(now) = clocks.read_us(clock);
                Ok(now)
            }
            Reader::Python(read) => Python::attach(|py| read.call1(py, (clock.name(),))?.extract(py)),
        }
    }
}

type SwitchTuple = (&'static str, &'static str, i64);

fn switch_tuple(switch: &ClockSwitch) -> SwitchTuple {
    (switch.old.name(), switch.new.name(), switch.delta_us)
}

/// Microsecond service clock reading one Linux clock; the source can change at run time.
#[pyclass(frozen, subclass, name = "ServiceClock", module = "tobii_linux.qpc_clock")]
pub struct PyServiceClock(ServiceClock<Reader>);

impl PyServiceClock {
    fn state(&self, py: Python<'_>) -> ClockState {
        crate::exit::detached(py, || self.0.state())
    }
}

#[pymethods]
impl PyServiceClock {
    #[new]
    #[pyo3(signature = (source=None, automatic=false, read_us=None))]
    fn new(
        py: Python<'_>,
        source: Option<Bound<'_, PyAny>>,
        automatic: bool,
        read_us: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let source = source.unwrap_or_else(|| qpc::DEFAULT_SOURCE.name().into_pyobject(py).expect("str").into_any());
        let name: Option<String> = source.extract().ok();
        let Some(source) = name.as_deref().and_then(LinuxClock::from_name) else {
            let names: Vec<&str> = LinuxClock::PREFERENCE.iter().map(|clock| clock.name()).collect();
            let expected = PyTuple::new(source.py(), names)?.repr()?;
            return Err(PyValueError::new_err(format!(
                "Unknown Linux clock {}; expected one of {expected}",
                source.repr()?
            )));
        };
        let reader = read_us.map_or(Reader::Linux(LinuxClocks), Reader::Python);
        Ok(Self(ServiceClock::new(source, automatic, reader)))
    }

    fn __call__(&self, py: Python<'_>) -> PyResult<i64> {
        crate::exit::detached(py, || self.0.now_us())
    }

    /// One of the Linux clocks in microseconds, through this clock's reader.
    fn read_us(&self, py: Python<'_>, name: &str) -> PyResult<i64> {
        let clock = clock_name(name)?;
        crate::exit::detached(py, || self.0.reader().read_us(clock))
    }

    /// Every candidate clock, read back to back.
    fn read_all<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let values = crate::exit::detached(py, || self.0.read_all())?;
        let readings = PyDict::new(py);
        for (clock, value) in LinuxClock::PREFERENCE.iter().zip(values) {
            readings.set_item(clock.name(), value)?;
        }
        Ok(readings)
    }

    /// Read `source` from now on: its own value, or continuing from the current value.
    #[pyo3(signature = (source, keep_value=false))]
    fn switch(&self, py: Python<'_>, source: &str, keep_value: bool) -> PyResult<SwitchTuple> {
        let source = clock_name(source)?;
        let switch = crate::exit::detached(py, || self.0.switch(source, keep_value))?;
        Ok(switch_tuple(&switch))
    }

    #[getter]
    fn source(&self, py: Python<'_>) -> &'static str {
        self.state(py).source.name()
    }

    #[getter]
    fn generation(&self, py: Python<'_>) -> i64 {
        self.state(py).generation
    }

    #[getter]
    fn offset_us(&self, py: Python<'_>) -> i64 {
        self.state(py).offset_us
    }

    #[getter]
    fn automatic(&self) -> bool {
        self.0.automatic()
    }
}

/// A QpcSample dataclass: `qpc_us` and `host_us` by clock name; other names are ignored.
fn sample_arg(sample: &Bound<'_, PyAny>) -> PyResult<QpcSample> {
    let host = sample.getattr("host_us")?;
    let mut host_us = [None; 3];
    for (slot, clock) in host_us.iter_mut().zip(LinuxClock::PREFERENCE) {
        if host.contains(clock.name())? {
            *slot = Some(host.get_item(clock.name())?.extract()?);
        }
    }
    Ok(QpcSample { qpc_us: sample.getattr("qpc_us")?.extract()?, host_us })
}

fn readings<'py, V: IntoPyObject<'py> + Copy>(
    py: Python<'py>,
    values: &[(LinuxClock, V)],
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (clock, value) in values {
        dict.set_item(clock.name(), *value)?;
    }
    Ok(dict)
}

fn sample_tuple<'py>(py: Python<'py>, sample: &QpcSample) -> PyResult<(i64, Bound<'py, PyDict>)> {
    let host: Vec<(LinuxClock, i64)> = LinuxClock::PREFERENCE
        .iter()
        .zip(sample.host_us)
        .filter_map(|(clock, value)| value.map(|value| (*clock, value)))
        .collect();
    Ok((sample.qpc_us, readings(py, &host)?))
}

type VerdictTuple<'py> = (Option<&'static str>, &'static str, Bound<'py, PyDict>, Bound<'py, PyDict>);

fn verdict_tuple<'py>(py: Python<'py>, verdict: &ClockVerdict) -> PyResult<VerdictTuple<'py>> {
    Ok((
        verdict.clock.map(LinuxClock::name),
        verdict.basis.name(),
        readings(py, &verdict.delays_us)?,
        readings(py, &verdict.rates_ppm)?,
    ))
}

/// Decides which Linux clock one process's QueryPerformanceCounter follows.
#[pyclass(frozen, subclass, name = "QpcClockClassifier", module = "tobii_linux.qpc_clock")]
pub struct PyQpcClockClassifier(Mutex<QpcClockClassifier>);

impl PyQpcClockClassifier {
    fn with<T: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut QpcClockClassifier) -> T + Send) -> T {
        crate::exit::detached(py, || f(&mut lock(&self.0)))
    }
}

#[pymethods]
impl PyQpcClockClassifier {
    #[new]
    #[pyo3(signature = (
        tolerance_us=qpc::ABSOLUTE_TOLERANCE_US, recent=qpc::RECENT_REPORTS, window=qpc::RATE_WINDOW_REPORTS,
        min_rate_span_us=qpc::RATE_MIN_SPAN_US, rate_margin_ppm=qpc::RATE_MARGIN_PPM
    ))]
    fn new(tolerance_us: i64, recent: usize, window: usize, min_rate_span_us: i64, rate_margin_ppm: f64) -> Self {
        let options = ClassifierOptions { tolerance_us, recent, window, min_rate_span_us, rate_margin_ppm };
        Self(Mutex::new(QpcClockClassifier::new(options)))
    }

    fn add(&self, py: Python<'_>, sample: &Bound<'_, PyAny>) -> PyResult<()> {
        let sample = sample_arg(sample)?;
        self.with(py, |classifier| classifier.add(sample));
        Ok(())
    }

    fn clocks(&self, py: Python<'_>) -> Vec<&'static str> {
        self.with(py, |classifier| classifier.clocks()).into_iter().map(LinuxClock::name).collect()
    }

    fn recent_delays_us<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        readings(py, &self.with(py, |classifier| classifier.recent_delays_us()))
    }

    /// Median pairwise drift of each clock against the counter (Theil-Sen slope).
    fn drift_ppm<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        readings(py, &self.with(py, |classifier| classifier.drift_ppm()))
    }

    /// `(clock, basis, delays by clock, drift by clock)`.
    #[pyo3(signature = (current=None))]
    fn verdict<'py>(&self, py: Python<'py>, current: Option<&str>) -> PyResult<VerdictTuple<'py>> {
        let current = optional_clock(current)?;
        verdict_tuple(py, &self.with(py, |classifier| classifier.verdict(current)))
    }

    /// `(qpc_us, host_us)` of every retained sample, oldest first.
    fn sample_values<'py>(&self, py: Python<'py>) -> PyResult<Vec<(i64, Bound<'py, PyDict>)>> {
        let samples: Vec<QpcSample> = self.with(py, |classifier| classifier.samples().iter().copied().collect());
        samples.iter().map(|sample| sample_tuple(py, sample)).collect()
    }
}

/// Keeps a ServiceClock on the clock behind the bridges' counters; bridges are integer keys.
#[pyclass(frozen, name = "QpcClockFollower", module = "tobii_linux.qpc_clock")]
pub struct PyQpcClockFollower {
    clock: Py<PyServiceClock>,
    follower: Mutex<QpcClockFollower<u64>>,
}

#[pymethods]
impl PyQpcClockFollower {
    #[new]
    fn new(clock: Py<PyServiceClock>) -> Self {
        Self { clock, follower: Mutex::new(QpcClockFollower::default()) }
    }

    /// `(verdict tuple, switch tuple or None, key of the bridge holding the clock or None)`.
    fn observe<'py>(
        &self,
        py: Python<'py>,
        bridge: u64,
        sample: &Bound<'py, PyAny>,
    ) -> PyResult<(VerdictTuple<'py>, Option<SwitchTuple>, Option<u64>)> {
        let sample = sample_arg(sample)?;
        let clock = &self.clock.get().0;
        let decision = crate::exit::detached(py, || lock(&self.follower).observe(clock, bridge, sample))?;
        Ok((verdict_tuple(py, &decision.verdict)?, decision.switch.as_ref().map(switch_tuple), decision.held_by))
    }

    fn forget(&self, py: Python<'_>, bridge: u64) {
        crate::exit::detached(py, || lock(&self.follower).forget(&bridge));
    }

    #[getter]
    fn clock(&self, py: Python<'_>) -> Py<PyServiceClock> {
        self.clock.clone_ref(py)
    }
}

/// Service-clock microseconds as the game's QueryPerformanceCounter microseconds.
#[pyclass(frozen, name = "CounterMapping", module = "tobii_linux.qpc_clock")]
pub struct PyCounterMapping {
    clock: Py<PyServiceClock>,
    mapping: Mutex<CounterMapping>,
}

impl PyCounterMapping {
    fn offset(&self, py: Python<'_>) -> i64 {
        let state = self.clock.get().state(py);
        crate::exit::detached(py, || lock(&self.mapping).offset_us(&state))
    }
}

#[pymethods]
impl PyCounterMapping {
    #[new]
    #[pyo3(signature = (clock, recent=qpc::RECENT_REPORTS, tolerance_us=qpc::ABSOLUTE_TOLERANCE_US))]
    fn new(clock: Py<PyServiceClock>, recent: usize, tolerance_us: i64) -> Self {
        Self { clock, mapping: Mutex::new(CounterMapping::new(recent, tolerance_us)) }
    }

    fn observe(&self, py: Python<'_>, sample: &Bound<'_, PyAny>) -> PyResult<()> {
        let sample = sample_arg(sample)?;
        crate::exit::detached(py, || lock(&self.mapping).observe(sample));
        Ok(())
    }

    /// Service clock minus game counter.
    fn offset_us(&self, py: Python<'_>) -> i64 {
        self.offset(py)
    }

    fn __call__(&self, py: Python<'_>, service_us: i64) -> i64 {
        service_us - self.offset(py)
    }

    #[getter]
    fn clock(&self, py: Python<'_>) -> Py<PyServiceClock> {
        self.clock.clone_ref(py)
    }
}

#[pyfunction]
fn linux_clock_us(name: &str) -> PyResult<i64> {
    let Ok(now) = LinuxClocks.read_us(clock_name(name)?);
    Ok(now)
}

#[pyfunction]
fn micros(counter: u64, frequency: u64) -> PyResult<u128> {
    if frequency == 0 {
        return Err(PyValueError::new_err("QueryPerformanceCounter report without a frequency"));
    }
    Ok(qpc::micros(counter, frequency))
}

#[pyfunction]
#[pyo3(signature = (counter, frequency, details=None))]
fn encode_qpc_report<'py>(
    py: Python<'py>,
    counter: u64,
    frequency: u64,
    details: Option<&Bound<'py, PyDict>>,
) -> PyResult<Bound<'py, PyBytes>> {
    let mut lines = Vec::new();
    if let Some(details) = details {
        for (key, value) in details.iter() {
            lines.push((key.str()?.to_string(), value.str()?.to_string()));
        }
    }
    Ok(PyBytes::new(py, &qpc::encode_qpc_report(counter, frequency, &lines)))
}

/// `(counter, frequency, details)`.
#[pyfunction]
fn decode_qpc_report<'py>(py: Python<'py>, payload: &Bound<'py, PyAny>) -> PyResult<(u64, u64, Bound<'py, PyDict>)> {
    let report = qpc::decode_qpc_report(&bytes_like(payload)?).py()?;
    let details = PyDict::new(py);
    for (key, value) in &report.details {
        details.set_item(key, value)?;
    }
    Ok((report.counter, report.frequency, details))
}

#[pyfunction]
fn describe(basis: &str, delays_us: Vec<(String, i64)>, rates_ppm: Vec<(String, f64)>) -> String {
    qpc::describe(basis, &delays_us, &rates_ppm)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyServiceClock>()?;
    m.add_class::<PyQpcClockClassifier>()?;
    m.add_class::<PyQpcClockFollower>()?;
    m.add_class::<PyCounterMapping>()?;
    for function in [
        wrap_pyfunction!(linux_clock_us, m)?,
        wrap_pyfunction!(micros, m)?,
        wrap_pyfunction!(encode_qpc_report, m)?,
        wrap_pyfunction!(decode_qpc_report, m)?,
        wrap_pyfunction!(describe, m)?,
    ] {
        m.add_function(function)?;
    }
    let names: Vec<&str> = LinuxClock::PREFERENCE.iter().map(|clock| clock.name()).collect();
    m.add("CLOCK_PREFERENCE", PyTuple::new(m.py(), names)?)?;
    m.add("DEFAULT_SOURCE", qpc::DEFAULT_SOURCE.name())?;
    m.add("ABSOLUTE_TOLERANCE_US", qpc::ABSOLUTE_TOLERANCE_US)?;
    m.add("RECENT_REPORTS", qpc::RECENT_REPORTS)?;
    m.add("RATE_WINDOW_REPORTS", qpc::RATE_WINDOW_REPORTS)?;
    m.add("RATE_MIN_SPAN_US", qpc::RATE_MIN_SPAN_US)?;
    m.add("RATE_MIN_PAIR_GAP_US", qpc::RATE_MIN_PAIR_GAP_US)?;
    m.add("RATE_MARGIN_PPM", qpc::RATE_MARGIN_PPM)?;
    Ok(())
}
