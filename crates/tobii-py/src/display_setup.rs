//! tobii_linux.display_setup: the display-area model, the moving-lines wizard, EDID hints,
//! screenplane.setpm and the 1430/1440/2110 procedures.
//!
//! Areas cross the boundary as (top_left, top_right, bottom_left) triples and mountings as
//! (guides, width, angle, external, internal) tuples; the Python module wraps them in its
//! dataclasses.

use std::path::PathBuf;

use pyo3::IntoPyObjectExt;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_gaze::Operation;
use tobii_gaze::display::{self, DisplayArea, DrmMonitor, LineSetup, MountingGeometry, Vector};

use crate::convert::bytes_like;
use crate::errors::IntoPyResult;
use crate::gaze_operation::{IntoGazeResult, PyOperation, values_arg};

type Triple = (f64, f64, f64);
type AreaTuple = (Triple, Triple, Triple);
type MountingTuple = (i64, f64, f64, Triple, Triple);

fn triple([x, y, z]: Vector) -> Triple {
    (x, y, z)
}

pub fn area_tuple(area: &DisplayArea) -> AreaTuple {
    (triple(area.top_left), triple(area.top_right), triple(area.bottom_left))
}

/// A DisplayArea from any object with its corner attributes.
pub fn area_arg(area: &Bound<'_, PyAny>) -> PyResult<DisplayArea> {
    Ok(DisplayArea {
        top_left: area.getattr("top_left")?.extract()?,
        top_right: area.getattr("top_right")?.extract()?,
        bottom_left: area.getattr("bottom_left")?.extract()?,
    })
}

fn mounting_tuple(m: &MountingGeometry) -> MountingTuple {
    (m.guides, m.width_mm, m.angle_deg, triple(m.external_offset_mm), triple(m.internal_offset_mm))
}

fn mounting_arg(mounting: &Bound<'_, PyAny>) -> PyResult<MountingGeometry> {
    Ok(MountingGeometry {
        guides: mounting.getattr("guides")?.extract()?,
        width_mm: mounting.getattr("width_mm")?.extract()?,
        angle_deg: mounting.getattr("angle_deg")?.extract()?,
        external_offset_mm: mounting.getattr("external_offset_mm")?.extract()?,
        internal_offset_mm: mounting.getattr("internal_offset_mm")?.extract()?,
    })
}

#[pyfunction]
fn width_mm(area: &Bound<'_, PyAny>) -> PyResult<f64> {
    Ok(area_arg(area)?.width_mm())
}

#[pyfunction]
fn height_mm(area: &Bound<'_, PyAny>) -> PyResult<f64> {
    Ok(area_arg(area)?.height_mm())
}

#[pyfunction]
fn almost_equals(area: &Bound<'_, PyAny>, other: &Bound<'_, PyAny>, squared_mm: f64) -> PyResult<bool> {
    Ok(area_arg(area)?.almost_equals(&area_arg(other)?, squared_mm))
}

#[pyfunction]
fn area_from_floats(values: Vec<f64>) -> PyResult<AreaTuple> {
    Ok(area_tuple(&DisplayArea::from_floats(&values).gaze()?))
}

#[pyfunction]
fn flat_display_area(size_mm: [f64; 2]) -> AreaTuple {
    area_tuple(&display::flat_display_area(size_mm))
}

#[pyfunction]
fn compose_display_area(size_mm: [f64; 2], mounting: &Bound<'_, PyAny>, tracker_offset_mm: f64) -> PyResult<AreaTuple> {
    Ok(area_tuple(&display::compose_display_area(size_mm, &mounting_arg(mounting)?, tracker_offset_mm)))
}

#[pyfunction]
fn decompose_display_area(area: &Bound<'_, PyAny>, mounting: &Bound<'_, PyAny>) -> PyResult<((f64, f64), f64, f64)> {
    let setup = display::decompose_display_area(&area_arg(area)?, &mounting_arg(mounting)?);
    Ok(((setup.size_mm[0], setup.size_mm[1]), setup.tracker_offset_mm, setup.extra_angle_deg))
}

#[pyfunction]
fn size_from_tracker_lines(
    left: f64,
    right: f64,
    tracker_width_mm: f64,
    aspect_ratio: f64,
) -> PyResult<((f64, f64), f64)> {
    let ([width, height], offset) =
        display::size_from_tracker_lines(left, right, tracker_width_mm, aspect_ratio).gaze()?;
    Ok(((width, height), offset))
}

#[pyfunction]
fn tracker_lines(size_mm: [f64; 2], tracker_offset_mm: f64, tracker_width_mm: f64) -> (f64, f64) {
    display::tracker_lines(size_mm, tracker_offset_mm, tracker_width_mm)
}

#[pyfunction]
fn picture_proportion(mounting: &Bound<'_, PyAny>) -> PyResult<f64> {
    Ok(display::picture_proportion(&mounting_arg(mounting)?))
}

#[pyfunction]
fn clamp_lines(left: f64, right: f64) -> (f64, f64) {
    display::clamp_lines(left, right)
}

#[pyfunction]
fn is_mapped(device_area: Option<&Bound<'_, PyAny>>) -> PyResult<bool> {
    Ok(display::is_mapped(device_area.map(area_arg).transpose()?.as_ref()))
}

#[pyfunction]
fn line_instructions(mounting: &Bound<'_, PyAny>) -> PyResult<&'static str> {
    Ok(display::line_instructions(&mounting_arg(mounting)?))
}

type LineTuple = (f64, f64, (f64, f64), f64, f64, &'static str);

fn line_tuple(lines: &LineSetup) -> LineTuple {
    let [width, height] = lines.size_mm;
    (lines.left, lines.right, (width, height), lines.tracker_offset_mm, lines.aspect, lines.source)
}

#[pyfunction]
fn initial_lines(
    mounting: &Bound<'_, PyAny>,
    device_area: Option<&Bound<'_, PyAny>>,
    size_hint: Option<[f64; 2]>,
    screen_px: Option<[f64; 2]>,
) -> PyResult<LineTuple> {
    let area = device_area.map(area_arg).transpose()?;
    Ok(line_tuple(&display::initial_lines(&mounting_arg(mounting)?, area.as_ref(), size_hint, screen_px)))
}

#[pyfunction]
fn area_from_lines(
    left: f64,
    right: f64,
    aspect: f64,
    mounting: &Bound<'_, PyAny>,
) -> PyResult<((f64, f64), f64, AreaTuple)> {
    let ([width, height], offset, area) =
        display::area_from_lines(left, right, aspect, &mounting_arg(mounting)?).gaze()?;
    Ok(((width, height), offset, area_tuple(&area)))
}

#[pyfunction]
fn size_from_diagonal(diagonal_inches: f64, aspect: [f64; 2]) -> (f64, f64) {
    let [width, height] = display::size_from_diagonal(diagonal_inches, aspect);
    (width, height)
}

#[pyfunction]
fn edid_size_mm(edid: &Bound<'_, PyAny>) -> PyResult<(f64, f64)> {
    let [width, height] = display::edid_size_mm(&bytes_like(edid)?).gaze()?;
    Ok((width, height))
}

#[pyfunction]
fn edid_native_resolution(edid: &Bound<'_, PyAny>) -> PyResult<(u32, u32)> {
    let [width, height] = display::edid_native_resolution(&bytes_like(edid)?).gaze()?;
    Ok((width, height))
}

#[pyfunction]
fn size_hint_is_plausible(size_mm: [f64; 2], resolution_px: [f64; 2]) -> bool {
    display::size_hint_is_plausible(size_mm, resolution_px)
}

type DrmTuple = (String, String, (f64, f64), (u32, u32));

fn drm_tuple(monitor: DrmMonitor) -> DrmTuple {
    let ([w, h], [x, y]) = (monitor.size_mm, monitor.resolution_px);
    (monitor.connector, monitor.status, (w, h), (x, y))
}

#[pyfunction]
fn drm_monitors(py: Python<'_>, root: PathBuf) -> PyResult<Vec<DrmTuple>> {
    let monitors = crate::exit::detached(py, || display::drm_monitors(&root)).gaze()?;
    Ok(monitors.into_iter().map(drm_tuple).collect())
}

#[pyfunction]
fn display_area_from_reply(values: &Bound<'_, PyAny>) -> PyResult<AreaTuple> {
    Ok(area_tuple(&display::display_area_from_reply(&values_arg(values)?).py()?))
}

#[pyfunction]
fn mounting_from_reply(values: &Bound<'_, PyAny>) -> PyResult<MountingTuple> {
    Ok(mounting_tuple(&display::mounting_from_reply(&values_arg(values)?).py()?))
}

#[pyfunction]
fn screen_plane_setting<'py>(py: Python<'py>, area: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &display::screen_plane_setting(&area_arg(area)?).gaze()?))
}

#[pyfunction]
fn parse_screen_plane_setting(data: &Bound<'_, PyAny>) -> PyResult<AreaTuple> {
    Ok(area_tuple(&display::parse_screen_plane_setting(&bytes_like(data)?).py()?))
}

#[pyfunction]
fn save_display_area(path: PathBuf, area: &Bound<'_, PyAny>, overwrite: bool) -> PyResult<PathBuf> {
    display::save_display_area(&path, &area_arg(area)?, overwrite).gaze()
}

#[pyfunction]
fn load_display_area(path: PathBuf) -> PyResult<AreaTuple> {
    Ok(area_tuple(&display::load_display_area(&path).gaze()?))
}

fn area_object(py: Python<'_>, area: DisplayArea) -> PyResult<Py<PyAny>> {
    area_tuple(&area).into_py_any(py)
}

#[pyfunction]
fn read_display_area() -> PyOperation {
    PyOperation::new(Operation::new(display::read_display_area), area_object)
}

#[pyfunction]
fn read_mounting() -> PyOperation {
    PyOperation::new(Operation::new(display::read_mounting), |py, m| mounting_tuple(&m).into_py_any(py))
}

#[pyfunction]
fn apply_display_area(area: &Bound<'_, PyAny>) -> PyResult<PyOperation> {
    let area = area_arg(area)?;
    Ok(PyOperation::new(Operation::new(move |io| display::apply_display_area(io, area)), area_object))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("ET5_MOUNTING", mounting_tuple(&display::ET5_MOUNTING))?;
    m.add("EDID_HEADER", PyBytes::new(m.py(), &display::EDID_HEADER))?;
    m.add("DEFAULT_ASPECT", display::DEFAULT_ASPECT)?;
    m.add_function(wrap_pyfunction!(width_mm, m)?)?;
    m.add_function(wrap_pyfunction!(height_mm, m)?)?;
    m.add_function(wrap_pyfunction!(almost_equals, m)?)?;
    m.add_function(wrap_pyfunction!(area_from_floats, m)?)?;
    m.add_function(wrap_pyfunction!(flat_display_area, m)?)?;
    m.add_function(wrap_pyfunction!(compose_display_area, m)?)?;
    m.add_function(wrap_pyfunction!(decompose_display_area, m)?)?;
    m.add_function(wrap_pyfunction!(size_from_tracker_lines, m)?)?;
    m.add_function(wrap_pyfunction!(tracker_lines, m)?)?;
    m.add_function(wrap_pyfunction!(picture_proportion, m)?)?;
    m.add_function(wrap_pyfunction!(clamp_lines, m)?)?;
    m.add_function(wrap_pyfunction!(is_mapped, m)?)?;
    m.add_function(wrap_pyfunction!(line_instructions, m)?)?;
    m.add_function(wrap_pyfunction!(initial_lines, m)?)?;
    m.add_function(wrap_pyfunction!(area_from_lines, m)?)?;
    m.add_function(wrap_pyfunction!(size_from_diagonal, m)?)?;
    m.add_function(wrap_pyfunction!(edid_size_mm, m)?)?;
    m.add_function(wrap_pyfunction!(edid_native_resolution, m)?)?;
    m.add_function(wrap_pyfunction!(size_hint_is_plausible, m)?)?;
    m.add_function(wrap_pyfunction!(drm_monitors, m)?)?;
    m.add_function(wrap_pyfunction!(display_area_from_reply, m)?)?;
    m.add_function(wrap_pyfunction!(mounting_from_reply, m)?)?;
    m.add_function(wrap_pyfunction!(screen_plane_setting, m)?)?;
    m.add_function(wrap_pyfunction!(parse_screen_plane_setting, m)?)?;
    m.add_function(wrap_pyfunction!(save_display_area, m)?)?;
    m.add_function(wrap_pyfunction!(load_display_area, m)?)?;
    m.add_function(wrap_pyfunction!(read_display_area, m)?)?;
    m.add_function(wrap_pyfunction!(read_mounting, m)?)?;
    m.add_function(wrap_pyfunction!(apply_display_area, m)?)?;
    Ok(())
}
