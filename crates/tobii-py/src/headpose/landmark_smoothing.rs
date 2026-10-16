//! tobii_linux.landmark_smoothing: the history filter and the record smoothing.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_headpose::smoothing::{self, LandmarkFilter, LandmarkRecord, Records, TableEntry};

use super::{HeadPoseResult, bytes, floats};

#[pyclass(name = "LandmarkFilter", module = "tobii_linux.landmark_smoothing")]
struct PyLandmarkFilter(LandmarkFilter);

#[pymethods]
impl PyLandmarkFilter {
    #[new]
    fn new(channels: usize, history: usize) -> PyResult<Self> {
        if history == 0 {
            return Err(super::invalid("The history must hold a sample"));
        }
        Ok(Self(LandmarkFilter::new(channels, history)))
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    fn begin_frame(&mut self) {
        self.0.begin_frame();
    }

    fn smooth_coordinates(&mut self, values: Vec<f32>, gains: Vec<f32>, time: i64) -> PyResult<Vec<f32>> {
        if gains.len() != values.len() {
            return Err(super::invalid("Expected one gain per value"));
        }
        self.0.smooth_coordinates(&values, &gains, time).py()
    }

    #[allow(clippy::too_many_arguments, reason = "the Python signature")]
    fn smooth_scalar(
        &mut self,
        value: f32,
        time: i64,
        gain: f32,
        low_floor: f32,
        high_cap: f32,
        initial_high: f32,
    ) -> PyResult<f32> {
        self.0.smooth_scalar(value, time, gain, low_floor, high_cap, initial_high).py()
    }

    /// `(coordinate updates [(id, x, y)], quality updates [(id, quality)])` for the records
    /// `[(id, x, y, present, quality)]`.
    #[allow(clippy::type_complexity, reason = "plain tuples for the Python wrapper")]
    fn smooth_records(
        &mut self,
        records: Vec<((u32, u32), f32, f32, bool, f32)>,
        time: i64,
        entries: Vec<TableEntry>,
    ) -> PyResult<(Vec<((u32, u32), f32, f32)>, Vec<((u32, u32), f32)>)> {
        let mut records: Records = records
            .into_iter()
            .map(|(id, x, y, present, quality)| (id, LandmarkRecord { x, y, present, quality }))
            .collect();
        let written = smoothing::smooth_records(&mut self.0, &mut records, time, &entries).py()?;
        let coordinates = written.coordinates.iter().map(|id| (*id, records[id].x, records[id].y)).collect();
        let qualities = written.qualities.iter().map(|id| (*id, records[id].quality)).collect();
        Ok((coordinates, qualities))
    }

    #[getter]
    fn channels(&self) -> usize {
        self.0.channels
    }

    #[getter]
    fn history(&self) -> usize {
        self.0.history
    }

    #[getter]
    fn enabled(&self) -> bool {
        self.0.enabled
    }

    #[setter]
    fn set_enabled(&mut self, enabled: bool) {
        self.0.enabled = enabled;
    }

    #[getter]
    fn cursor(&self) -> usize {
        self.0.cursor
    }

    #[setter]
    fn set_cursor(&mut self, cursor: usize) {
        self.0.cursor = cursor;
    }

    #[getter]
    fn frames(&self) -> i64 {
        self.0.frames
    }

    #[getter]
    fn window(&self) -> i64 {
        self.0.window()
    }

    #[getter]
    fn values<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        bytes(py, &self.0.values)
    }

    #[getter]
    fn times(&self) -> Vec<i64> {
        self.0.times.clone()
    }

    #[getter]
    fn position(&self) -> Vec<usize> {
        self.0.position.clone()
    }

    #[getter]
    fn initialised(&self) -> Vec<bool> {
        self.0.initialised.clone()
    }

    #[getter]
    fn high<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        bytes(py, &self.0.high)
    }

    #[getter]
    fn low<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        bytes(py, &self.0.low)
    }
}

#[pyfunction]
fn index_prediction_row<'py>(py: Python<'py>, count: usize, degree: usize) -> PyResult<Bound<'py, PyBytes>> {
    if count <= degree {
        return Err(super::invalid("Expected more samples than the polynomial degree"));
    }
    Ok(bytes(py, &smoothing::index_prediction_row(count, degree)))
}

/// Residuals of `count x channels` samples (bytes) over float32 sample times.
#[pyfunction]
fn time_residual<'py>(
    py: Python<'py>,
    samples: &[u8],
    channels: usize,
    times: Vec<f32>,
    degree: usize,
) -> PyResult<Bound<'py, PyBytes>> {
    let samples = floats(samples)?;
    if samples.len() != channels * times.len() || times.is_empty() {
        return Err(super::invalid("Samples must be count x channels"));
    }
    Ok(bytes(py, &smoothing::time_residual(&samples, channels, &times, degree)))
}

#[pyfunction]
fn polynomial_blend<'py>(
    py: Python<'py>,
    samples: &[u8],
    channels: usize,
    times: Vec<f32>,
    gains: Vec<f32>,
) -> PyResult<Bound<'py, PyBytes>> {
    let samples = floats(samples)?;
    if samples.len() != channels * times.len() || gains.len() != channels || times.len() <= 4 {
        return Err(super::invalid("Samples must be count x channels with one gain per channel"));
    }
    Ok(bytes(py, &smoothing::polynomial_blend(&samples, channels, &times, &gains)))
}

#[pyfunction]
fn reciprocal<'py>(py: Python<'py>, values: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    Ok(bytes(py, &floats(values)?.iter().map(|&v| smoothing::reciprocal(v)).collect::<Vec<_>>()))
}

/// `1 - y` for each present record's y.
#[pyfunction]
fn flip_y(ys: Vec<f32>) -> Vec<f32> {
    let mut records: Records = ys
        .iter()
        .enumerate()
        .map(|(i, &y)| ((0, i as u32), LandmarkRecord { y, present: true, ..LandmarkRecord::default() }))
        .collect();
    smoothing::flip_y(&mut records);
    records.values().map(|r| r.y).collect()
}

#[pyfunction]
fn table_entries(factors: Vec<f32>, table: Vec<(u32, u32, u32, usize)>) -> PyResult<Vec<TableEntry>> {
    smoothing::table_entries(&factors, &table).py()
}

#[pyfunction]
fn coordinate_gain(group: u32, quality: f32, factor: f32) -> f32 {
    smoothing::coordinate_gain(group, quality, factor)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(index_prediction_row, m)?,
        wrap_pyfunction!(time_residual, m)?,
        wrap_pyfunction!(polynomial_blend, m)?,
        wrap_pyfunction!(reciprocal, m)?,
        wrap_pyfunction!(flip_y, m)?,
        wrap_pyfunction!(table_entries, m)?,
        wrap_pyfunction!(coordinate_gain, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add_class::<PyLandmarkFilter>()?;
    m.add("TRACKER_TABLE", smoothing::TRACKER_TABLE.to_vec())?;
    m.add("EAR_TABLE", smoothing::EAR_TABLE.to_vec())?;
    m.add("GROUP_SIZES", smoothing::GROUP_SIZES.to_vec())?;
    m.add("QUALITY_SMOOTHING", smoothing::QUALITY_SMOOTHING.to_vec())?;
    Ok(())
}
