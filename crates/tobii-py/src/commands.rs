//! tobii_linux.ttp_commands: request builders checked against the original builders.

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyTuple};
use tobii_protocol::commands as tc;

use crate::convert::{int, uint32};
use crate::errors::{IntoPyResult, protocol_message};
use crate::ttp::{PyField, PyMessage, fields_arg};

fn message(py: Python<'_>, built: tobii_protocol::Message) -> PyMessage {
    PyMessage::from_core(py, &built)
}

fn stream_arg(stream: &Bound<'_, PyAny>) -> PyResult<u32> {
    int(stream).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| protocol_message("Unknown stream index"))
}

#[pyfunction]
fn wire_stream_id(stream: &Bound<'_, PyAny>) -> PyResult<u32> {
    tc::wire_stream_id(stream_arg(stream)?).py()
}

#[pyfunction]
fn int32_field(value: &Bound<'_, PyAny>) -> PyResult<PyField> {
    let value = int(value)
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| protocol_message("Expected a signed 32-bit integer"))?;
    Ok(PyField::from_core(&tc::int32_field(value)))
}

#[pyfunction]
fn uint32_field(value: &Bound<'_, PyAny>) -> PyResult<PyField> {
    Ok(PyField::from_core(&tobii_protocol::fields::uint32_field(uint32(value)?)))
}

#[pyfunction]
fn fixed16_field(value: f64) -> PyField {
    PyField::from_core(&tc::fixed16_field(value))
}

#[pyfunction]
fn string_field(text: &str) -> PyResult<PyField> {
    Ok(PyField::from_core(&tc::string_field(text).py()?))
}

fn blob_arg<'a>(data: &'a Bound<'_, PyAny>) -> PyResult<&'a [u8]> {
    Ok(data.cast::<PyBytes>().map_err(|_| PyTypeError::new_err("Blob data must be bytes"))?.as_bytes())
}

#[pyfunction]
fn blob_field(data: &Bound<'_, PyAny>) -> PyResult<PyField> {
    Ok(PyField::from_core(&tc::blob_field(blob_arg(data)?)))
}

#[pyfunction]
fn struct_header(member_count: &Bound<'_, PyAny>, struct_type: &Bound<'_, PyAny>) -> PyResult<PyField> {
    let range = || protocol_message("Struct member count or type out of range");
    let count = int(member_count).and_then(|n| u32::try_from(n).ok()).ok_or_else(range)?;
    let kind = int(struct_type).and_then(|n| u32::try_from(n).ok()).ok_or_else(range)?;
    Ok(PyField::from_core(&tc::struct_header(count, kind).py()?))
}

#[pyfunction]
#[pyo3(signature = (request_id, command, fields=None))]
fn request(
    py: Python<'_>,
    request_id: &Bound<'_, PyAny>,
    command: &Bound<'_, PyAny>,
    fields: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyMessage> {
    let items = match fields {
        Some(fields) => fields_arg(fields)?,
        None => Vec::new(),
    };
    Ok(message(py, tc::request(uint32(request_id)?, uint32(command)?, &items).py()?))
}

#[pyfunction]
fn header_only_request(
    py: Python<'_>,
    request_id: &Bound<'_, PyAny>,
    command: &Bound<'_, PyAny>,
) -> PyResult<PyMessage> {
    let command = int(command)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| protocol_message("Command is not built by a header-only wrapper"))?;
    Ok(message(py, tc::header_only_request(uint32(request_id)?, command).py()?))
}

#[pyfunction]
#[pyo3(signature = (request_id, stream, columns=None))]
fn subscribe_stream(
    py: Python<'_>,
    request_id: &Bound<'_, PyAny>,
    stream: &Bound<'_, PyAny>,
    columns: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyMessage> {
    let columns: Vec<u32> = match columns {
        Some(columns) => columns.try_iter()?.map(|c| uint32(&c?)).collect::<PyResult<_>>()?,
        None => Vec::new(),
    };
    Ok(message(py, tc::subscribe_stream(uint32(request_id)?, stream_arg(stream)?, &columns).py()?))
}

#[pyfunction]
fn unsubscribe_stream(py: Python<'_>, request_id: &Bound<'_, PyAny>, stream: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(message(py, tc::unsubscribe_stream(uint32(request_id)?, stream_arg(stream)?).py()?))
}

#[pyfunction]
fn enumerate_stream_type_columns(
    py: Python<'_>,
    request_id: &Bound<'_, PyAny>,
    stream: &Bound<'_, PyAny>,
) -> PyResult<PyMessage> {
    Ok(message(py, tc::enumerate_stream_type_columns(uint32(request_id)?, stream_arg(stream)?).py()?))
}

#[pyfunction]
fn open_realm_salt(py: Python<'_>, request_id: &Bound<'_, PyAny>, realm: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(message(py, tc::open_realm_salt(uint32(request_id)?, uint32(realm)?)))
}

#[pyfunction]
fn realm_digest<'py>(py: Python<'py>, key: &[u8], salt: &[u8]) -> Bound<'py, PyBytes> {
    PyBytes::new(py, &tc::realm_digest(key, salt))
}

#[pyfunction]
#[pyo3(signature = (request_id, challenge, digest, extra=None))]
fn open_realm_response(
    py: Python<'_>,
    request_id: &Bound<'_, PyAny>,
    challenge: &Bound<'_, PyAny>,
    digest: &Bound<'_, PyAny>,
    extra: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyMessage> {
    let first = uint32(&challenge.get_item(0)?)?;
    let second = uint32(&challenge.get_item(1)?)?;
    let digest = blob_arg(digest)?;
    let extra = match extra {
        Some(extra) => blob_arg(extra)?.to_vec(),
        None => Vec::new(),
    };
    Ok(message(py, tc::open_realm_response(uint32(request_id)?, (first, second), digest, &extra).py()?))
}

#[pyfunction]
fn close_realm(py: Python<'_>, request_id: &Bound<'_, PyAny>, realm: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(message(py, tc::close_realm(uint32(request_id)?, uint32(realm)?)))
}

#[pyfunction]
fn pause_device(py: Python<'_>, request_id: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(message(py, tc::pause_device(uint32(request_id)?)))
}

#[pyfunction]
fn resume_device(py: Python<'_>, request_id: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(message(py, tc::resume_device(uint32(request_id)?)))
}

#[pyfunction]
fn set_illumination_mode(py: Python<'_>, request_id: &Bound<'_, PyAny>, mode: &str) -> PyResult<PyMessage> {
    Ok(message(py, tc::set_illumination_mode(uint32(request_id)?, mode).py()?))
}

#[pyfunction]
fn set_output_frequency(py: Python<'_>, request_id: &Bound<'_, PyAny>, frequency: f64) -> PyResult<PyMessage> {
    Ok(message(py, tc::set_output_frequency(uint32(request_id)?, frequency)))
}

#[pyfunction]
fn set_frequencies(
    py: Python<'_>,
    request_id: &Bound<'_, PyAny>,
    first: &Bound<'_, PyAny>,
    second: &Bound<'_, PyAny>,
) -> PyResult<PyMessage> {
    Ok(message(py, tc::set_frequencies(uint32(request_id)?, uint32(first)?, uint32(second)?)))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    for function in [
        wrap_pyfunction!(wire_stream_id, m)?,
        wrap_pyfunction!(int32_field, m)?,
        wrap_pyfunction!(uint32_field, m)?,
        wrap_pyfunction!(fixed16_field, m)?,
        wrap_pyfunction!(string_field, m)?,
        wrap_pyfunction!(blob_field, m)?,
        wrap_pyfunction!(struct_header, m)?,
        wrap_pyfunction!(request, m)?,
        wrap_pyfunction!(header_only_request, m)?,
        wrap_pyfunction!(subscribe_stream, m)?,
        wrap_pyfunction!(unsubscribe_stream, m)?,
        wrap_pyfunction!(enumerate_stream_type_columns, m)?,
        wrap_pyfunction!(open_realm_salt, m)?,
        wrap_pyfunction!(realm_digest, m)?,
        wrap_pyfunction!(open_realm_response, m)?,
        wrap_pyfunction!(close_realm, m)?,
        wrap_pyfunction!(pause_device, m)?,
        wrap_pyfunction!(resume_device, m)?,
        wrap_pyfunction!(set_illumination_mode, m)?,
        wrap_pyfunction!(set_output_frequency, m)?,
        wrap_pyfunction!(set_frequencies, m)?,
    ] {
        m.add_function(function)?;
    }
    let wire_ids = PyDict::new(py);
    for (index, id) in tc::WIRE_STREAM_IDS {
        wire_ids.set_item(index, id)?;
    }
    m.add("WIRE_STREAM_IDS", wire_ids)?;
    m.add("HEADER_ONLY_COMMANDS", PyTuple::new(py, tc::HEADER_ONLY_COMMANDS)?)?;
    Ok(())
}
