//! tobii_linux.prp_service: relay framing, the connect response, property values and the
//! cached user presence of the PRP service. The asyncio service itself stays in Python.

use std::sync::{Mutex, MutexGuard};

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};
use tobii_service::prp_service::{self as service, PropertySource, UserPresence};
use tobii_service::prp_session::STATUS_NOT_SUPPORTED;

use crate::convert::bytes_like;
use crate::errors::IntoPyResult;
use crate::prp::{i32_arg, masked_i64, masked_u32, u32_arg};
use crate::prp_session::{connect_response_tuple, property_tuple, value_tuple};

type Value<'py> = (Bound<'py, PyBytes>, Bound<'py, PyBytes>);
type PropertyTuple<'py> = (u32, i32, u32, Bound<'py, PyBytes>, Bound<'py, PyBytes>);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

#[pyfunction]
#[pyo3(signature = (kind, payload=None))]
fn encode_frame<'py>(py: Python<'py>, kind: u32, payload: Option<&Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyBytes>> {
    let payload = payload.map(bytes_like).transpose()?.unwrap_or_default();
    Ok(PyBytes::new(py, &service::encode_frame(kind, &payload).py()?))
}

/// Kind and payload size of an eight-byte relay frame header.
#[pyfunction]
fn decode_frame_header(header: &Bound<'_, PyAny>) -> PyResult<(u32, usize)> {
    service::decode_frame_header(&bytes_like(header)?).py()
}

#[pyfunction]
fn encode_names<'py>(py: Python<'py>, names: Vec<String>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &service::encode_names(&names).py()?))
}

#[pyfunction]
fn decode_names(payload: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    service::decode_names(&bytes_like(payload)?).py()
}

#[pyfunction]
fn encode_pipe_open<'py>(py: Python<'py>, client_pid: u32, pipe: &str) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &service::encode_pipe_open(client_pid, pipe).py()?))
}

#[pyfunction]
fn decode_pipe_open(payload: &Bound<'_, PyAny>) -> PyResult<(u32, String)> {
    service::decode_pipe_open(&bytes_like(payload)?).py()
}

#[pyfunction]
fn packet_operation(packet: &Bound<'_, PyAny>) -> PyResult<u32> {
    service::packet_operation(&bytes_like(packet)?).py()
}

/// `(tag, command)` of any operation 9 request.
#[pyfunction]
fn request_header(packet: &Bound<'_, PyAny>) -> PyResult<(u32, u32)> {
    service::request_header(&bytes_like(packet)?).py()
}

#[pyfunction]
fn error_response(py: Python<'_>, tag: u32, command: u32, status: i32) -> Bound<'_, PyBytes> {
    PyBytes::new(py, &service::error_response(tag, command, status))
}

#[pyfunction]
fn word_value(py: Python<'_>, value: u32) -> Value<'_> {
    value_tuple(py, service::word_value(value))
}

#[pyfunction]
fn presence_value<'py>(py: Python<'py>, value: u32, since_us: &Bound<'py, PyAny>) -> PyResult<Value<'py>> {
    Ok(value_tuple(py, service::presence_value(value, masked_i64(since_us)?)))
}

#[pyfunction]
#[pyo3(signature = (tag, property_id, status=None))]
fn property_error<'py>(
    py: Python<'py>,
    tag: &Bound<'py, PyAny>,
    property_id: &Bound<'py, PyAny>,
    status: Option<&Bound<'py, PyAny>>,
) -> PyResult<PropertyTuple<'py>> {
    let status = status.map(i32_arg).transpose()?.unwrap_or(STATUS_NOT_SUPPORTED);
    let response = service::property_error(u32_arg(tag)?, masked_u32(property_id)?, status);
    Ok(property_tuple(py, &response))
}

#[pyfunction]
#[pyo3(signature = (tag, property_id, status=None))]
fn subscribe_error<'py>(
    py: Python<'py>,
    tag: &Bound<'py, PyAny>,
    property_id: &Bound<'py, PyAny>,
    status: Option<&Bound<'py, PyAny>>,
) -> PyResult<PropertyTuple<'py>> {
    let status = status.map(i32_arg).transpose()?.unwrap_or(STATUS_NOT_SUPPORTED);
    let response = service::subscribe_error(u32_arg(tag)?, masked_u32(property_id)?, status);
    Ok(property_tuple(py, &response))
}

/// ConnectResponse fields of the real ET5 service's reply for `serial`.
#[pyfunction]
fn device_connect_response<'py>(
    py: Python<'py>,
    serial: &str,
    license_count: usize,
    streams: Vec<i32>,
) -> PyResult<Bound<'py, PyTuple>> {
    connect_response_tuple(py, &service::device_connect_response(serial, license_count, &streams))
}

#[pyfunction]
fn presence_subscribe_time(tracker_time_us: i64) -> i64 {
    service::presence_subscribe_time(tracker_time_us)
}

/// Property 0xB as the service caches it (0x1400fd220, cache 0x14010e930).
#[pyclass(frozen, name = "UserPresence", module = "tobii_linux.prp_service")]
pub struct PyUserPresence(Mutex<UserPresence>);

impl PyUserPresence {
    fn with<T: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut UserPresence) -> T + Send) -> T {
        crate::exit::detached(py, || f(&mut lock(&self.0)))
    }
}

#[pymethods]
impl PyUserPresence {
    #[new]
    fn new() -> Self {
        Self(Mutex::new(UserPresence::default()))
    }

    /// Forget the record, as the service does when a device is added again (0x14010e8d0).
    fn clear(&self, py: Python<'_>) {
        self.with(py, UserPresence::clear);
    }

    /// Cache one platmod record; True when the service would push it as a notification.
    fn record(&self, py: Python<'_>, timestamp_us: i64, platmod_value: i64) -> bool {
        self.with(py, |presence| presence.record(timestamp_us, platmod_value))
    }

    #[getter]
    fn value(&self, py: Python<'_>) -> Option<u32> {
        self.with(py, |presence| presence.value)
    }

    #[setter]
    fn set_value(&self, py: Python<'_>, value: Option<u32>) {
        self.with(py, |presence| presence.value = value);
    }

    #[getter]
    fn since_us(&self, py: Python<'_>) -> i64 {
        self.with(py, |presence| presence.since_us)
    }

    #[setter]
    fn set_since_us(&self, py: Python<'_>, since_us: i64) {
        self.with(py, |presence| presence.since_us = since_us);
    }

    #[getter]
    fn present(&self, py: Python<'_>) -> bool {
        self.with(py, |presence| presence.present())
    }

    fn value_block<'py>(&self, py: Python<'py>) -> PyResult<Value<'py>> {
        Ok(value_tuple(py, self.with(py, |presence| presence.value_block()).py()?))
    }

    fn notification<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.with(py, |presence| presence.notification()).py()?))
    }
}

/// Value of a property for a get or a subscribe, or a refusal, from the service state.
#[pyfunction]
#[pyo3(signature = (tag, property_id, subscribe, display_area, display_id, calibration_id, presence, field_of_use))]
#[allow(clippy::too_many_arguments, reason = "one argument per piece of service state the answer reads")]
fn answer_property<'py>(
    py: Python<'py>,
    tag: &Bound<'py, PyAny>,
    property_id: &Bound<'py, PyAny>,
    subscribe: bool,
    display_area: Option<Vec<f64>>,
    display_id: Option<String>,
    calibration_id: u32,
    presence: &Bound<'py, PyUserPresence>,
    field_of_use: Option<i32>,
) -> PyResult<PropertyTuple<'py>> {
    let presence = presence.get().with(py, |presence| *presence);
    let source = PropertySource {
        display_area: display_area.as_deref(),
        display_id: display_id.as_deref(),
        calibration_id,
        presence: &presence,
        field_of_use,
        paused: false,
    };
    let response = service::answer_property(&source, u32_arg(tag)?, masked_u32(property_id)?, subscribe).py()?;
    Ok(property_tuple(py, &response))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyUserPresence>()?;
    for function in [
        wrap_pyfunction!(encode_frame, m)?,
        wrap_pyfunction!(decode_frame_header, m)?,
        wrap_pyfunction!(encode_names, m)?,
        wrap_pyfunction!(decode_names, m)?,
        wrap_pyfunction!(encode_pipe_open, m)?,
        wrap_pyfunction!(decode_pipe_open, m)?,
        wrap_pyfunction!(packet_operation, m)?,
        wrap_pyfunction!(request_header, m)?,
        wrap_pyfunction!(error_response, m)?,
        wrap_pyfunction!(word_value, m)?,
        wrap_pyfunction!(presence_value, m)?,
        wrap_pyfunction!(property_error, m)?,
        wrap_pyfunction!(subscribe_error, m)?,
        wrap_pyfunction!(device_connect_response, m)?,
        wrap_pyfunction!(presence_subscribe_time, m)?,
        wrap_pyfunction!(answer_property, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("RELAY_HEADER_SIZE", service::RELAY_HEADER_SIZE)?;
    m.add("MAX_FRAME", service::MAX_FRAME)?;
    m.add("GAZE_STREAMS", PyTuple::new(m.py(), service::GAZE_STREAMS)?)?;
    m.add("LICENSED_STREAMS", PyTuple::new(m.py(), service::LICENSED_STREAMS)?)?;
    m.add("GAZE_SERVICE_STREAMS", PyTuple::new(m.py(), service::GAZE_SERVICE_STREAMS)?)?;
    m.add("LICENSED_SUBSCRIPTIONS", PyTuple::new(m.py(), service::LICENSED_SUBSCRIPTIONS)?)?;
    m.add("SUBSCRIBE_ONLY_PROPERTIES", PyTuple::new(m.py(), service::SUBSCRIBE_ONLY_PROPERTIES)?)?;
    m.add("PROPERTY_ZERO_WORDS", PyTuple::new(m.py(), service::PROPERTY_ZERO_WORDS)?)?;
    m.add("TRACK_BOX_MM", PyTuple::new(m.py(), service::TRACK_BOX_MM)?)?;
    m.add(
        "PROPERTY_BLOCK_SIZES",
        service::PROPERTY_BLOCK_SIZES.into_iter().collect::<std::collections::HashMap<_, _>>(),
    )?;
    Ok(())
}
