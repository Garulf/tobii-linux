//! tobii_linux.camera_info: the hardware-info reply (0x848) as nested tuples in record order.

use pyo3::prelude::*;
use pyo3::types::{PyList, PyTuple};
use tobii_protocol::camera_info::{self, CameraRecord, HardwareInfo};

use crate::convert::{bytes_like, floats};
use crate::errors::IntoPyResult;
use crate::fields::PyReader;
use crate::ttp::{PyField, PyMessage, message_arg};

type CameraTuple =
    (i32, f64, f64, (f64, f64, f64), Vec<f64>, i32, i32, i32, Vec<f64>, (f64, f64, f64), (f64, f64, f64), f64);

fn triple([x, y, z]: [f64; 3]) -> (f64, f64, f64) {
    (x, y, z)
}

fn camera_tuple(c: &CameraRecord) -> CameraTuple {
    (
        c.int32_0x00,
        c.float32_0x04,
        c.float32_0x08,
        triple(c.triple_0x10),
        c.doubles_0x28.clone(),
        c.int32_0xa0,
        c.int32_0xa4,
        c.int32_0xa8,
        c.series_0xb0.clone(),
        triple(c.triple_0x2b0),
        triple(c.triple_0x2c8),
        c.double_0x2e0,
    )
}

fn info_tuple<'py>(py: Python<'py>, info: &HardwareInfo) -> PyResult<Bound<'py, PyTuple>> {
    let cameras = PyList::new(py, info.cameras_0x8.iter().map(camera_tuple))?;
    let points = PyList::new(py, info.points_0xbb0.iter().map(|p| triple(*p)))?;
    (cameras, points, info.int32_0xf70).into_pyobject(py)
}

/// A camera record from any object with the CameraRecord attributes.
fn camera_arg(camera: &Bound<'_, PyAny>) -> PyResult<CameraRecord> {
    let get = |name: &str| camera.getattr(name);
    Ok(CameraRecord {
        int32_0x00: get("int32_0x00")?.extract()?,
        float32_0x04: get("float32_0x04")?.extract()?,
        float32_0x08: get("float32_0x08")?.extract()?,
        triple_0x10: floats::<3>(&get("triple_0x10")?)?,
        doubles_0x28: get("doubles_0x28")?.extract()?,
        int32_0xa0: get("int32_0xa0")?.extract()?,
        int32_0xa4: get("int32_0xa4")?.extract()?,
        int32_0xa8: get("int32_0xa8")?.extract()?,
        series_0xb0: get("series_0xb0")?.extract()?,
        triple_0x2b0: floats::<3>(&get("triple_0x2b0")?)?,
        triple_0x2c8: floats::<3>(&get("triple_0x2c8")?)?,
        double_0x2e0: get("double_0x2e0")?.extract()?,
    })
}

fn info_arg(info: &Bound<'_, PyAny>) -> PyResult<HardwareInfo> {
    let cameras = info.getattr("cameras_0x8")?.try_iter()?.map(|c| camera_arg(&c?)).collect::<PyResult<_>>()?;
    let points = info.getattr("points_0xbb0")?.try_iter()?.map(|p| floats::<3>(&p?)).collect::<PyResult<_>>()?;
    Ok(HardwareInfo {
        cameras_0x8: cameras,
        points_0xbb0: points,
        int32_0xf70: info.getattr("int32_0xf70")?.extract()?,
    })
}

#[pyfunction]
fn read_camera(reader: &Bound<'_, PyReader>) -> PyResult<CameraTuple> {
    reader.get().read(camera_info::read_camera).map(|c| camera_tuple(&c))
}

#[pyfunction]
fn read_points(reader: &Bound<'_, PyReader>) -> PyResult<Vec<(f64, f64, f64)>> {
    reader.get().read(camera_info::read_points).map(|points| points.into_iter().map(triple).collect())
}

#[pyfunction]
fn read_hardware_info<'py>(py: Python<'py>, payload: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    info_tuple(py, &camera_info::read_hardware_info(&bytes_like(payload)?).py()?)
}

#[pyfunction]
fn parse_hardware_info<'py>(py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    info_tuple(py, &camera_info::parse_hardware_info(&message_arg(message)?).py()?)
}

#[pyfunction]
fn camera_fields(camera: &Bound<'_, PyAny>) -> PyResult<Vec<PyField>> {
    Ok(camera_info::camera_fields(&camera_arg(camera)?).py()?.iter().map(PyField::from_core).collect())
}

#[pyfunction]
#[pyo3(signature = (info, request_id, descriptors=(0, 0)))]
fn hardware_info_response(
    py: Python<'_>,
    info: &Bound<'_, PyAny>,
    request_id: u32,
    descriptors: (u32, u32),
) -> PyResult<PyMessage> {
    let message = camera_info::hardware_info_response(&info_arg(info)?, request_id, descriptors).py()?;
    Ok(PyMessage::from_core(py, &message))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(read_camera, m)?,
        wrap_pyfunction!(read_points, m)?,
        wrap_pyfunction!(read_hardware_info, m)?,
        wrap_pyfunction!(parse_hardware_info, m)?,
        wrap_pyfunction!(camera_fields, m)?,
        wrap_pyfunction!(hardware_info_response, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("CAMERA_HEADER", camera_info::CAMERA_HEADER)?;
    m.add("CAMERA_RECORD_SIZE", camera_info::CAMERA_RECORD_SIZE)?;
    m.add("CAMERA_SERIES_LIMIT", camera_info::CAMERA_SERIES_LIMIT)?;
    m.add("MAX_CAMERAS", camera_info::MAX_CAMERAS)?;
    m.add("MAX_POINTS", camera_info::MAX_POINTS)?;
    Ok(())
}
