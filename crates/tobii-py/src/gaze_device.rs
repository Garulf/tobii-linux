//! tobii_linux.gaze_device: calibration commands, persistent files, setting files and the
//! calibration records of the device.
//!
//! Records cross the boundary as tuples in dataclass field order; the Python module wraps
//! them in its dataclasses and enums.

use pyo3::IntoPyObjectExt;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};
use tobii_gaze::device::{self, CalibrationPointData, EyeQuality, LocalCalibration, PointStatus, StimulusPoint};
use tobii_gaze::{Eye, Operation};
use tobii_protocol::Message;

use crate::convert::bytes_like;
use crate::errors::IntoPyResult;
use crate::gaze_operation::{
    IntoGazeResult, PyOperation, StimulusTuple, register as register_operation, stimulus_tuple, values_arg,
};
use crate::ttp::{PyMessage, fields_list};

pub fn eye_arg(eye: u32) -> PyResult<Eye> {
    Eye::from_u32(eye).ok_or_else(|| PyValueError::new_err(format!("{eye} is not a valid Eye")))
}

fn message(py: Python<'_>, message: Result<Message, tobii_protocol::ProtocolError>) -> PyResult<Bound<'_, PyMessage>> {
    PyMessage::into_bound(py, &message.py()?)
}

#[pyfunction]
fn header_only(py: Python<'_>, request_id: u32, command: u32) -> PyResult<Bound<'_, PyMessage>> {
    message(py, device::header_only(request_id, command))
}

#[pyfunction]
fn calibration_start_eye(py: Python<'_>, request_id: u32, eye: u32) -> PyResult<Bound<'_, PyMessage>> {
    message(py, Ok(device::calibration_start_eye(request_id, eye_arg(eye)?)))
}

/// One of the point commands (1030, 1031, 1032, 1080, 1081) with two or three coordinates.
#[pyfunction]
fn point_request(
    py: Python<'_>,
    request_id: u32,
    command: u32,
    coordinates: Vec<f64>,
    eye: u32,
) -> PyResult<Bound<'_, PyMessage>> {
    use tobii_gaze::device::gaze_command as gc;
    let eye = eye_arg(eye)?;
    let built = match (command, coordinates.as_slice()) {
        (gc::CALIBRATION_COLLECT_2D, &[x, y]) => device::calibration_collect_2d(request_id, x, y, eye),
        (gc::CALIBRATION_COLLECT_PER_EYE_2D, &[x, y]) => device::calibration_collect_per_eye_2d(request_id, x, y, eye),
        (gc::CALIBRATION_DISCARD_2D, &[x, y]) => device::calibration_discard_2d(request_id, x, y, eye),
        (gc::CALIBRATION_COLLECT_3D, &[x, y, z]) => device::calibration_collect_3d(request_id, [x, y, z], eye),
        (gc::CALIBRATION_DISCARD_3D, &[x, y, z]) => device::calibration_discard_3d(request_id, [x, y, z], eye),
        _ => return Err(PyValueError::new_err("Not a point command with matching coordinates")),
    };
    message(py, built)
}

#[pyfunction]
fn calibration_apply<'py>(
    py: Python<'py>,
    request_id: u32,
    calibration: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyMessage>> {
    message(py, device::calibration_apply(request_id, &bytes_like(calibration)?))
}

#[pyfunction]
fn set_display_area(
    py: Python<'_>,
    request_id: u32,
    top_left: [f64; 3],
    top_right: [f64; 3],
    bottom_left: [f64; 3],
) -> PyResult<Bound<'_, PyMessage>> {
    message(py, device::set_display_area(request_id, top_left, top_right, bottom_left))
}

#[pyfunction]
fn set_enabled_eye(py: Python<'_>, request_id: u32, eye: u32) -> PyResult<Bound<'_, PyMessage>> {
    message(py, Ok(device::set_enabled_eye(request_id, eye_arg(eye)?)))
}

#[pyfunction]
fn enable_extension(py: Python<'_>, request_id: u32, extension: u32) -> PyResult<Bound<'_, PyMessage>> {
    message(py, Ok(device::enable_extension(request_id, extension)))
}

#[pyfunction]
fn get_configuration_key<'py>(py: Python<'py>, request_id: u32, name: &str) -> PyResult<Bound<'py, PyMessage>> {
    message(py, device::get_configuration_key(request_id, name))
}

#[pyfunction]
fn persistent_file_read<'py>(py: Python<'py>, request_id: u32, name: &str) -> PyResult<Bound<'py, PyMessage>> {
    message(py, device::persistent_file_read(request_id, name))
}

#[pyfunction]
fn persistent_file_write<'py>(
    py: Python<'py>,
    request_id: u32,
    name: &str,
    data: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyMessage>> {
    message(py, device::persistent_file_write(request_id, name, &bytes_like(data)?))
}

#[pyfunction]
fn persistent_file_erase<'py>(py: Python<'py>, request_id: u32, name: &str) -> PyResult<Bound<'py, PyMessage>> {
    message(py, device::persistent_file_erase(request_id, name))
}

/// (key, value) pairs in reply order, later keys replacing earlier ones.
#[pyfunction]
fn status_entries<'py>(py: Python<'py>, values: &Bound<'py, PyAny>) -> PyResult<Vec<(i64, Bound<'py, PyBytes>)>> {
    let entries = device::status_entries(&values_arg(values)?).py()?;
    Ok(entries.into_iter().map(|(key, text)| (key, PyBytes::new(py, &text))).collect())
}

#[pyfunction]
fn device_calibration_id(values: &Bound<'_, PyAny>) -> PyResult<Option<u32>> {
    device::device_calibration_id(&values_arg(values)?).gaze()
}

#[pyfunction]
fn setting_file_payload<'py>(py: Python<'py>, data: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let data = bytes_like(data)?;
    Ok(PyBytes::new(py, device::setting_file_payload(&data).py()?))
}

#[pyfunction]
#[pyo3(signature = (payload, kind=1))]
fn setting_file<'py>(py: Python<'py>, payload: &Bound<'py, PyAny>, kind: u32) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &device::setting_file(&bytes_like(payload)?, kind)))
}

pub fn local_arg(calibration: &Bound<'_, PyAny>) -> PyResult<LocalCalibration> {
    Ok(LocalCalibration {
        calibration_id: calibration.getattr("calibration_id")?.extract()?,
        blob: bytes_like(&calibration.getattr("blob")?)?,
    })
}

pub fn local_tuple<'py>(py: Python<'py>, calibration: &LocalCalibration) -> (u32, Bound<'py, PyBytes>) {
    (calibration.calibration_id, PyBytes::new(py, &calibration.blob))
}

#[pyfunction]
fn calibration_setting<'py>(py: Python<'py>, calibration: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &device::calibration_setting(&local_arg(calibration)?).py()?))
}

#[pyfunction]
fn parse_calibration_setting<'py>(py: Python<'py>, data: &Bound<'py, PyAny>) -> PyResult<(u32, Bound<'py, PyBytes>)> {
    Ok(local_tuple(py, &device::parse_calibration_setting(&bytes_like(data)?).py()?))
}

type Triple = (f64, f64, f64);

#[pyfunction]
fn screen_plane_setting(data: &Bound<'_, PyAny>) -> PyResult<(Triple, Triple, Triple)> {
    let [a, b, c] = device::screen_plane_setting(&bytes_like(data)?).py()?.map(|[x, y, z]| (x, y, z));
    Ok((a, b, c))
}

#[pyfunction]
fn stimulus_points(values: &Bound<'_, PyAny>) -> PyResult<Vec<StimulusTuple>> {
    Ok(device::stimulus_points(&values_arg(values)?).py()?.iter().map(stimulus_tuple).collect())
}

pub fn stimulus_arg(point: &Bound<'_, PyAny>) -> PyResult<StimulusPoint> {
    let eye = |name: &str| -> PyResult<EyeQuality> {
        let quality = point.getattr(name)?;
        Ok(EyeQuality {
            status: quality.getattr("status")?.extract()?,
            bias: quality.getattr("bias")?.extract()?,
            precision: quality.getattr("precision")?.extract()?,
        })
    };
    Ok(StimulusPoint { point: point.getattr("point")?.extract()?, left: eye("left")?, right: eye("right")? })
}

/// Reply fields of 1120 for StimulusPoint-like objects.
#[pyfunction]
fn stimulus_points_payload<'py>(py: Python<'py>, points: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
    let points = points.try_iter()?.map(|p| stimulus_arg(&p?)).collect::<PyResult<Vec<_>>>()?;
    fields_list(py, &device::stimulus_points_payload(&points).py()?)
}

type PointTuple = ((f64, f64), u8, (f64, f64), u8, (f64, f64));

pub fn point_tuple(p: &CalibrationPointData) -> PointTuple {
    let pair = |[x, y]: [f64; 2]| (x, y);
    (pair(p.point), p.left_status as u8, pair(p.left_mapping), p.right_status as u8, pair(p.right_mapping))
}

#[pyfunction]
fn calibration_points(blob: &Bound<'_, PyAny>) -> PyResult<Vec<PointTuple>> {
    Ok(device::calibration_points(&bytes_like(blob)?).py()?.iter().map(point_tuple).collect())
}

#[pyfunction]
fn calibration_record<'py>(
    py: Python<'py>,
    point: [f64; 2],
    left: ([f64; 2], i32),
    right: ([f64; 2], i32),
) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &device::calibration_record(point, left, right).gaze()?))
}

#[pyfunction]
fn blob_calibration_id(blob: &Bound<'_, PyAny>) -> PyResult<u32> {
    device::blob_calibration_id(&bytes_like(blob)?).py()
}

#[pyfunction]
#[pyo3(signature = (local, allow_apply=false))]
fn synchronize_calibration(local: Option<&Bound<'_, PyAny>>, allow_apply: bool) -> PyResult<PyOperation> {
    let local = local.map(local_arg).transpose()?;
    let operation = Operation::new(move |io| device::synchronize_calibration(io, local, allow_apply));
    Ok(PyOperation::new(operation, |py, state| (state as u8).into_py_any(py)))
}

#[pyfunction]
fn retrieve_calibration() -> PyOperation {
    PyOperation::new(Operation::new(device::retrieve_calibration), |py, blob| PyBytes::new(py, &blob).into_py_any(py))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    register_operation(m)?;
    m.add("GAZE_COMMANDS", device::GAZE_COMMANDS.to_vec())?;
    m.add("HEADER_ONLY", device::HEADER_ONLY.to_vec())?;
    m.add("READ_ONLY", device::READ_ONLY.to_vec())?;
    m.add("DISPLAY_AREA_TRAILER", device::DISPLAY_AREA_TRAILER)?;
    m.add("STATUS_STRUCT", device::STATUS_STRUCT)?;
    m.add("STATUS_CALIBRATION_ID", device::STATUS_CALIBRATION_ID)?;
    m.add("SETTING_FILE_VERSION", device::SETTING_FILE_VERSION)?;
    m.add("STIMULUS_POINT_STRUCT", device::STIMULUS_POINT_STRUCT)?;
    m.add("STIMULUS_POSITION_STRUCT", device::STIMULUS_POSITION_STRUCT)?;
    m.add("STIMULUS_POINT_ITEMS", device::STIMULUS_POINT_ITEMS)?;
    let statuses = [PointStatus::FailedOrInvalid, PointStatus::ValidButNotUsed, PointStatus::ValidAndUsed];
    m.add("POINT_STATUSES", statuses.map(|s| (s.name(), s as u8)).to_vec())?;
    m.add_function(wrap_pyfunction!(header_only, m)?)?;
    m.add_function(wrap_pyfunction!(calibration_start_eye, m)?)?;
    m.add_function(wrap_pyfunction!(point_request, m)?)?;
    m.add_function(wrap_pyfunction!(calibration_apply, m)?)?;
    m.add_function(wrap_pyfunction!(set_display_area, m)?)?;
    m.add_function(wrap_pyfunction!(set_enabled_eye, m)?)?;
    m.add_function(wrap_pyfunction!(enable_extension, m)?)?;
    m.add_function(wrap_pyfunction!(get_configuration_key, m)?)?;
    m.add_function(wrap_pyfunction!(persistent_file_read, m)?)?;
    m.add_function(wrap_pyfunction!(persistent_file_write, m)?)?;
    m.add_function(wrap_pyfunction!(persistent_file_erase, m)?)?;
    m.add_function(wrap_pyfunction!(status_entries, m)?)?;
    m.add_function(wrap_pyfunction!(device_calibration_id, m)?)?;
    m.add_function(wrap_pyfunction!(setting_file_payload, m)?)?;
    m.add_function(wrap_pyfunction!(setting_file, m)?)?;
    m.add_function(wrap_pyfunction!(calibration_setting, m)?)?;
    m.add_function(wrap_pyfunction!(parse_calibration_setting, m)?)?;
    m.add_function(wrap_pyfunction!(screen_plane_setting, m)?)?;
    m.add_function(wrap_pyfunction!(stimulus_points, m)?)?;
    m.add_function(wrap_pyfunction!(stimulus_points_payload, m)?)?;
    m.add_function(wrap_pyfunction!(calibration_points, m)?)?;
    m.add_function(wrap_pyfunction!(calibration_record, m)?)?;
    m.add_function(wrap_pyfunction!(blob_calibration_id, m)?)?;
    m.add_function(wrap_pyfunction!(synchronize_calibration, m)?)?;
    m.add_function(wrap_pyfunction!(retrieve_calibration, m)?)?;
    Ok(())
}
