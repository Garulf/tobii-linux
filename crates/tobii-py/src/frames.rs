//! tobii_linux.frames: stream column sets and camera image frames.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
use tobii_protocol::commands::Stream;
use tobii_protocol::frames::{self, ColumnValue, ImageFrame};

use crate::convert::bytes_like;
use crate::errors::{IntoPyResult, protocol_message};
use crate::ttp::{field_arg, fields_arg, fields_tuple, message_arg};

/// `(column_id, value fields)` for every column of a stream payload.
#[pyfunction]
fn stream_columns<'py>(py: Python<'py>, payload: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
    let columns = frames::stream_columns(&bytes_like(payload)?).py()?;
    PyList::new(
        py,
        columns.iter().map(|c| Ok((c.column_id, fields_tuple(py, &c.value)?))).collect::<PyResult<Vec<_>>>()?,
    )
}

#[pyfunction]
fn struct_code(field: &Bound<'_, PyAny>) -> PyResult<(u32, u32)> {
    frames::struct_code(&field_arg(field)?).py()
}

#[pyfunction]
fn take_value(items: &Bound<'_, PyAny>, index: usize) -> PyResult<usize> {
    frames::take_value(&fields_arg(items)?, index).py()
}

#[pyfunction]
fn uint32_value(field: &Bound<'_, PyAny>) -> PyResult<u32> {
    frames::uint32_value(&field_arg(field)?).py()
}

#[pyfunction]
fn int64_value(field: &Bound<'_, PyAny>) -> PyResult<i64> {
    frames::int64_value(&field_arg(field)?).py()
}

#[pyfunction]
fn blob_value<'py>(py: Python<'py>, field: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, frames::blob_value(&field_arg(field)?).py()?))
}

fn stream_arg(stream: u32) -> PyResult<Stream> {
    Stream::from_index(stream).ok_or_else(|| protocol_message("Unknown stream index"))
}

/// Values of exactly the `layout` columns (`{column id: wire type}`) of one data message.
#[pyfunction]
fn column_values<'py>(
    py: Python<'py>,
    message: &Bound<'py, PyAny>,
    stream: u32,
    layout: &Bound<'py, PyDict>,
) -> PyResult<Bound<'py, PyDict>> {
    let layout: Vec<(u32, u8)> =
        layout.iter().map(|(k, v)| Ok((k.extract()?, v.extract()?))).collect::<PyResult<_>>()?;
    let values = frames::column_values(&message_arg(message)?, stream_arg(stream)?, &layout).py()?;
    let result = PyDict::new(py);
    for ((column, _), value) in layout.iter().zip(values) {
        match value {
            ColumnValue::Int64(n) => result.set_item(column, n)?,
            ColumnValue::UInt32(n) => result.set_item(column, n)?,
            ColumnValue::Blob(bytes) => result.set_item(column, PyBytes::new(py, &bytes))?,
        }
    }
    Ok(result)
}

pub fn image_tuple<'py>(py: Python<'py>, frame: &ImageFrame) -> PyResult<Bound<'py, PyTuple>> {
    (frame.timestamp, frame.pixel_format, frame.width, frame.height, PyBytes::new(py, &frame.pixels)).into_pyobject(py)
}

/// `(timestamp, pixel_format, width, height, pixels)` of an image stream (0x501) message.
#[pyfunction]
fn image_frame<'py>(py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    image_tuple(py, &frames::image_frame(&message_arg(message)?).py()?)
}

/// `(timestamp, word2, width, stride, height, pixels)` of a primary camera (0x50e) message.
#[pyfunction]
fn primary_camera_frame<'py>(py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    let f = frames::primary_camera_frame(&message_arg(message)?).py()?;
    (f.timestamp, f.word2, f.width, f.stride, f.height, PyBytes::new(py, &f.pixels)).into_pyobject(py)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(stream_columns, m)?,
        wrap_pyfunction!(struct_code, m)?,
        wrap_pyfunction!(take_value, m)?,
        wrap_pyfunction!(uint32_value, m)?,
        wrap_pyfunction!(int64_value, m)?,
        wrap_pyfunction!(blob_value, m)?,
        wrap_pyfunction!(column_values, m)?,
        wrap_pyfunction!(image_frame, m)?,
        wrap_pyfunction!(primary_camera_frame, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("FRAME_STRUCT", frames::FRAME_STRUCT)?;
    m.add("COLUMN_STRUCT", frames::COLUMN_STRUCT)?;
    m.add("COLUMN_MEMBERS", frames::COLUMN_MEMBERS)?;
    m.add("HEAD_POSE_PIXEL_FORMAT", frames::HEAD_POSE_PIXEL_FORMAT)?;
    Ok(())
}
