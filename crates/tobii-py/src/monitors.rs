//! tobii_linux.monitors: desktop monitors, Tobii's monitor choice rules and the xrandr, DRM
//! and Windows query parsers.
//!
//! Monitors cross the boundary as tuples in dataclass field order; selections come back as
//! indexes into the list the caller passed, so the Python module returns its own objects.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_gaze::monitors::{self, Monitor};

use crate::convert::bytes_like;
use crate::gaze_operation::IntoGazeResult;

type MonitorTuple<'py> =
    (String, String, Option<(i64, i64, i64, i64)>, bool, u32, Option<Bound<'py, PyBytes>>, Option<String>);

fn monitor_tuple<'py>(py: Python<'py>, m: Monitor) -> MonitorTuple<'py> {
    let bounds = m.bounds.map(|[x, y, w, h]| (x, y, w, h));
    let edid = m.edid.map(|edid| PyBytes::new(py, &edid));
    (m.key, m.name, bounds, m.primary, m.rotation, edid, m.instance_id)
}

/// A Monitor from any object with its attributes (the Python dataclass).
fn monitor_arg(monitor: &Bound<'_, PyAny>) -> PyResult<Monitor> {
    let edid = monitor.getattr("edid")?;
    Ok(Monitor {
        key: monitor.getattr("key")?.extract()?,
        name: monitor.getattr("name")?.extract()?,
        bounds: monitor.getattr("bounds")?.extract()?,
        primary: monitor.getattr("primary")?.is_truthy()?,
        rotation: monitor.getattr("rotation")?.extract()?,
        edid: if edid.is_none() { None } else { Some(bytes_like(&edid)?) },
        instance_id: monitor.getattr("instance_id")?.extract()?,
        connector: None,
    })
}

fn monitors_arg(monitors: &Bound<'_, PyAny>) -> PyResult<Vec<Monitor>> {
    monitors.try_iter()?.map(|m| monitor_arg(&m?)).collect()
}

fn tuples<'py>(py: Python<'py>, monitors: Vec<Monitor>) -> Vec<MonitorTuple<'py>> {
    monitors.into_iter().map(|m| monitor_tuple(py, m)).collect()
}

#[pyfunction]
fn size_mm(monitor: &Bound<'_, PyAny>) -> PyResult<Option<(f64, f64)>> {
    Ok(monitor_arg(monitor)?.size_mm().map(|[w, h]| (w, h)))
}

#[pyfunction]
fn native_resolution(monitor: &Bound<'_, PyAny>) -> PyResult<Option<(u32, u32)>> {
    Ok(monitor_arg(monitor)?.native_resolution().map(|[w, h]| (w, h)))
}

#[pyfunction]
fn edid_id(monitor: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    Ok(monitor_arg(monitor)?.edid_id())
}

#[pyfunction]
fn display_id(monitor: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    Ok(monitor_arg(monitor)?.display_id())
}

#[pyfunction]
#[pyo3(signature = (a, b, tolerance=0.001))]
fn same_aspect_ignoring_orientation(a: Option<Vec<f64>>, b: Option<Vec<f64>>, tolerance: f64) -> bool {
    let size = |values: Option<Vec<f64>>| values.and_then(|v| <[f64; 2]>::try_from(v).ok());
    monitors::same_aspect_ignoring_orientation(size(a), size(b), tolerance)
}

#[pyfunction]
#[pyo3(signature = (monitor, resolution_px=None))]
fn has_recommended_aspect(monitor: &Bound<'_, PyAny>, resolution_px: Option<[f64; 2]>) -> PyResult<bool> {
    Ok(monitors::has_recommended_aspect(&monitor_arg(monitor)?, resolution_px))
}

#[pyfunction]
fn primary(monitors: &Bound<'_, PyAny>) -> PyResult<Option<usize>> {
    Ok(monitors::primary(&monitors_arg(monitors)?))
}

#[pyfunction]
#[pyo3(signature = (monitors, persisted_key=None))]
fn initial_monitor(monitors: &Bound<'_, PyAny>, persisted_key: Option<String>) -> PyResult<Option<usize>> {
    Ok(monitors::initial_monitor(&monitors_arg(monitors)?, persisted_key.as_deref()))
}

#[pyfunction]
fn previous_monitor(monitors: &Bound<'_, PyAny>, current: Option<&Bound<'_, PyAny>>) -> PyResult<Option<usize>> {
    let current = current.map(monitor_arg).transpose()?;
    Ok(monitors::previous_monitor(&monitors_arg(monitors)?, current.as_ref()))
}

#[pyfunction]
fn next_monitor(monitors: &Bound<'_, PyAny>, current: Option<&Bound<'_, PyAny>>) -> PyResult<Option<usize>> {
    let current = current.map(monitor_arg).transpose()?;
    Ok(monitors::next_monitor(&monitors_arg(monitors)?, current.as_ref()))
}

#[pyfunction]
fn parse_xrandr<'py>(py: Python<'py>, text: &str) -> PyResult<Vec<MonitorTuple<'py>>> {
    Ok(tuples(py, monitors::parse_xrandr(text).gaze()?))
}

#[pyfunction]
fn drm_fallback(py: Python<'_>, root: PathBuf) -> PyResult<Vec<MonitorTuple<'_>>> {
    let found = crate::exit::detached(py, || monitors::drm_fallback(&root)).gaze()?;
    Ok(tuples(py, found))
}

#[pyfunction]
fn instance_id_from_interface(interface: Option<&str>) -> Option<String> {
    monitors::instance_id_from_interface(interface)
}

#[pyfunction]
fn parse_windows_monitors<'py>(py: Python<'py>, text: &str) -> PyResult<Vec<MonitorTuple<'py>>> {
    Ok(tuples(py, monitors::parse_windows_monitors(text).gaze()?))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(size_mm, m)?)?;
    m.add_function(wrap_pyfunction!(native_resolution, m)?)?;
    m.add_function(wrap_pyfunction!(edid_id, m)?)?;
    m.add_function(wrap_pyfunction!(display_id, m)?)?;
    m.add_function(wrap_pyfunction!(same_aspect_ignoring_orientation, m)?)?;
    m.add_function(wrap_pyfunction!(has_recommended_aspect, m)?)?;
    m.add_function(wrap_pyfunction!(primary, m)?)?;
    m.add_function(wrap_pyfunction!(initial_monitor, m)?)?;
    m.add_function(wrap_pyfunction!(previous_monitor, m)?)?;
    m.add_function(wrap_pyfunction!(next_monitor, m)?)?;
    m.add_function(wrap_pyfunction!(parse_xrandr, m)?)?;
    m.add_function(wrap_pyfunction!(drm_fallback, m)?)?;
    m.add_function(wrap_pyfunction!(instance_id_from_interface, m)?)?;
    m.add_function(wrap_pyfunction!(parse_windows_monitors, m)?)?;
    Ok(())
}
