//! tobii_linux.ttp and tobii_linux.usb_framing: messages, items, decoders and channel framing.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};
use tobii_protocol::framing;
use tobii_protocol::ttp::{self, MAX_PAYLOAD};
use tobii_protocol::{Field, Message};

use crate::convert::{bytes_like, int, uint32};
use crate::errors::{IntoPyResult, protocol_message};

/// A TTP message. The payload stays one shared `bytes` object.
#[pyclass(frozen, name = "Message", module = "tobii_linux.ttp")]
pub struct PyMessage {
    #[pyo3(get)]
    kind: u32,
    #[pyo3(get)]
    request_id: u32,
    #[pyo3(get)]
    word2: u32,
    #[pyo3(get)]
    command: u32,
    #[pyo3(get)]
    word4: u32,
    #[pyo3(get)]
    payload: Py<PyBytes>,
}

impl PyMessage {
    pub fn from_core(py: Python<'_>, message: &Message) -> Self {
        Self {
            kind: message.kind,
            request_id: message.request_id,
            word2: message.word2,
            command: message.command,
            word4: message.word4,
            payload: PyBytes::new(py, &message.payload).unbind(),
        }
    }

    pub fn into_bound<'py>(py: Python<'py>, message: &Message) -> PyResult<Bound<'py, PyMessage>> {
        Bound::new(py, Self::from_core(py, message))
    }

    pub fn to_core(&self, py: Python<'_>) -> Message {
        Message::new(
            self.kind,
            self.request_id,
            self.word2,
            self.command,
            self.word4,
            self.payload.as_bytes(py).to_vec(),
        )
    }

    fn words(&self) -> [u32; 5] {
        [self.kind, self.request_id, self.word2, self.command, self.word4]
    }
}

pub fn message_arg(object: &Bound<'_, PyAny>) -> PyResult<Message> {
    let message = object.cast::<PyMessage>().map_err(|_| PyTypeError::new_err("Expected a ttp.Message"))?;
    Ok(message.get().to_core(object.py()))
}

pub fn messages_list<'py>(py: Python<'py>, messages: &[Message]) -> PyResult<Bound<'py, PyList>> {
    PyList::new(py, messages.iter().map(|message| PyMessage::from_core(py, message)))
}

#[pymethods]
impl PyMessage {
    #[new]
    #[pyo3(signature = (kind, request_id, word2, command, word4, payload=None))]
    fn new(
        py: Python<'_>,
        kind: &Bound<'_, PyAny>,
        request_id: &Bound<'_, PyAny>,
        word2: &Bound<'_, PyAny>,
        command: &Bound<'_, PyAny>,
        word4: &Bound<'_, PyAny>,
        payload: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let payload = match payload {
            None => PyBytes::new(py, b"").unbind(),
            Some(object) => {
                object.cast::<PyBytes>().map_err(|_| PyTypeError::new_err("payload must be bytes"))?.clone().unbind()
            }
        };
        Ok(Self {
            kind: uint32(kind)?,
            request_id: uint32(request_id)?,
            word2: uint32(word2)?,
            command: uint32(command)?,
            word4: uint32(word4)?,
            payload,
        })
    }

    #[pyo3(signature = (max_payload=MAX_PAYLOAD as i64))]
    fn encode<'py>(&self, py: Python<'py>, max_payload: i64) -> PyResult<Bound<'py, PyBytes>> {
        let limit = u32::try_from(max_payload.max(0)).unwrap_or(u32::MAX);
        let encoded = self.to_core(py).encode_limited(limit).py()?;
        Ok(PyBytes::new(py, &encoded))
    }

    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> bool {
        other.cast::<PyMessage>().is_ok_and(|other| {
            let other = other.get();
            self.words() == other.words() && self.payload.as_bytes(py) == other.payload.as_bytes(py)
        })
    }

    fn __hash__(&self, py: Python<'_>) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.words().hash(&mut hasher);
        self.payload.as_bytes(py).hash(&mut hasher);
        hasher.finish()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "Message(kind={}, request_id={}, word2={}, command={}, word4={}, payload={})",
            self.kind,
            self.request_id,
            self.word2,
            self.command,
            self.word4,
            self.payload.bind(py).repr()?
        ))
    }
}

/// One payload item. The tag is checked when the item is encoded, as in the dataclass.
#[pyclass(frozen, name = "Field", module = "tobii_linux.ttp")]
pub struct PyField {
    #[pyo3(get)]
    tag: i128,
    data: Vec<u8>,
}

impl PyField {
    pub fn from_core(field: &Field) -> Self {
        Self { tag: i128::from(field.tag), data: field.data.clone() }
    }

    pub fn to_core(&self) -> PyResult<Field> {
        let tag = u8::try_from(self.tag).map_err(|_| protocol_message("TLV tag must fit one byte"))?;
        Ok(Field::new(tag, self.data.clone()))
    }
}

pub fn field_arg(object: &Bound<'_, PyAny>) -> PyResult<Field> {
    object.cast::<PyField>().map_err(|_| PyTypeError::new_err("Expected a ttp.Field"))?.get().to_core()
}

/// Items of any iterable of fields, in order.
pub fn fields_arg(object: &Bound<'_, PyAny>) -> PyResult<Vec<Field>> {
    object.try_iter()?.map(|item| field_arg(&item?)).collect()
}

pub fn fields_list<'py>(py: Python<'py>, fields: &[Field]) -> PyResult<Bound<'py, PyList>> {
    PyList::new(py, fields.iter().map(PyField::from_core))
}

pub fn fields_tuple<'py>(py: Python<'py>, fields: &[Field]) -> PyResult<Bound<'py, pyo3::types::PyTuple>> {
    pyo3::types::PyTuple::new(py, fields.iter().map(PyField::from_core))
}

#[pymethods]
impl PyField {
    #[new]
    fn new(tag: &Bound<'_, PyAny>, data: &Bound<'_, PyAny>) -> PyResult<Self> {
        let tag = int(tag).ok_or_else(|| PyTypeError::new_err("TLV tag must be an integer"))?;
        let data = data.cast::<PyBytes>().map_err(|_| PyTypeError::new_err("TLV data must be bytes"))?;
        Ok(Self { tag, data: data.as_bytes().to_vec() })
    }

    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.data)
    }

    fn encode<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.to_core()?.encode().py()?))
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        other.cast::<PyField>().is_ok_and(|other| self.tag == other.get().tag && self.data == other.get().data)
    }

    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        (self.tag, &self.data).hash(&mut hasher);
        hasher.finish()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("Field(tag={}, data={})", self.tag, PyBytes::new(py, &self.data).repr()?))
    }
}

/// Incremental TTP decoder; it stays failed after a framing error.
#[pyclass(frozen, name = "Decoder", module = "tobii_linux.ttp")]
pub struct PyDecoder(Mutex<ttp::Decoder>);

#[pymethods]
impl PyDecoder {
    #[new]
    #[pyo3(signature = (max_payload=None))]
    fn new(max_payload: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let limit = max_payload
            .map_or(Some(i128::from(MAX_PAYLOAD)), int)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| PyValueError::new_err("Invalid payload limit"))?;
        Ok(Self(Mutex::new(ttp::Decoder::new(limit))))
    }

    fn feed<'py>(&self, py: Python<'py>, data: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
        let data = bytes_like(data)?;
        let messages = crate::exit::detached(py, || self.lock().feed(&data)).py()?;
        messages_list(py, &messages)
    }

    fn finish(&self, py: Python<'_>) -> PyResult<()> {
        crate::exit::detached(py, || self.lock().finish()).py()
    }

    #[getter]
    fn pending<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let pending = crate::exit::detached(py, || self.lock().pending().to_vec());
        PyBytes::new(py, &pending)
    }

    #[getter]
    fn max_payload(&self, py: Python<'_>) -> u32 {
        crate::exit::detached(py, || self.lock().max_payload())
    }

    #[getter]
    fn failed(&self, py: Python<'_>) -> bool {
        crate::exit::detached(py, || self.lock().failed())
    }
}

impl PyDecoder {
    fn lock(&self) -> std::sync::MutexGuard<'_, ttp::Decoder> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[pyfunction]
fn fields<'py>(py: Python<'py>, payload: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
    fields_list(py, &ttp::fields(&bytes_like(payload)?).py()?)
}

#[pyfunction]
fn uint32_array(values: &Bound<'_, PyAny>) -> PyResult<PyField> {
    let values: Vec<u32> = values.try_iter()?.map(|item| uint32(&item?)).collect::<PyResult<_>>()?;
    Ok(PyField::from_core(&ttp::uint32_array(&values)))
}

#[pyfunction]
fn read_uint32_array<'py>(py: Python<'py>, field: &Bound<'py, PyAny>) -> PyResult<Bound<'py, pyo3::types::PyTuple>> {
    pyo3::types::PyTuple::new(py, ttp::read_uint32_array(&field_arg(field)?).py()?)
}

#[pyfunction]
fn protocol_upgrade(py: Python<'_>, request_id: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(PyMessage::from_core(py, &ttp::protocol_upgrade(uint32(request_id)?)))
}

#[pyfunction]
fn hardware_info_request(py: Python<'_>, request_id: &Bound<'_, PyAny>) -> PyResult<PyMessage> {
    Ok(PyMessage::from_core(py, &ttp::hardware_info_request(uint32(request_id)?)))
}

#[pyfunction]
fn encode_payload<'py>(py: Python<'py>, items: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let items = fields_arg(items)?;
    Ok(PyBytes::new(py, &ttp::encode_payload(&items).py()?))
}

/// One bulk IN record: recipient bitmask and payload.
#[pyclass(frozen, name = "IncomingFrame", module = "tobii_linux.usb_framing")]
pub struct PyIncomingFrame {
    #[pyo3(get)]
    recipients: u32,
    #[pyo3(get)]
    payload: Py<PyBytes>,
}

fn channel_arg(channel: &Bound<'_, PyAny>) -> PyResult<u32> {
    int(channel)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n < framing::CHANNEL_COUNT)
        .ok_or_else(|| PyValueError::new_err("Channel must be between 0 and 11"))
}

#[pymethods]
impl PyIncomingFrame {
    #[new]
    fn new(recipients: u32, payload: Bound<'_, PyBytes>) -> Self {
        Self { recipients, payload: payload.unbind() }
    }

    fn for_channel(&self, channel: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(self.recipients & (1 << channel_arg(channel)?) != 0)
    }

    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> bool {
        other.cast::<PyIncomingFrame>().is_ok_and(|other| {
            self.recipients == other.get().recipients && self.payload.as_bytes(py) == other.get().payload.as_bytes(py)
        })
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("IncomingFrame(recipients={}, payload={})", self.recipients, self.payload.bind(py).repr()?))
    }
}

/// Incremental bulk IN parser; invalid framing poisons it.
#[pyclass(frozen, name = "IncomingFrames", module = "tobii_linux.usb_framing")]
pub struct PyIncomingFrames(Mutex<framing::IncomingFrames>);

impl PyIncomingFrames {
    fn lock(&self) -> std::sync::MutexGuard<'_, framing::IncomingFrames> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[pymethods]
impl PyIncomingFrames {
    #[new]
    #[pyo3(signature = (max_frame=None))]
    fn new(max_frame: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let limit = max_frame
            .map_or(Some(framing::MAX_FRAME as i128), int)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| PyValueError::new_err("Invalid frame limit"))?;
        Ok(Self(Mutex::new(framing::IncomingFrames::new(limit).py()?)))
    }

    fn feed<'py>(&self, py: Python<'py>, data: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
        let data = bytes_like(data)?;
        let frames = crate::exit::detached(py, || self.lock().feed(&data)).py()?;
        PyList::new(
            py,
            frames
                .iter()
                .map(|f| PyIncomingFrame { recipients: f.recipients, payload: PyBytes::new(py, &f.payload).unbind() }),
        )
    }

    #[getter]
    fn buffer<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let buffered = crate::exit::detached(py, || self.lock().buffered().to_vec());
        PyBytes::new(py, &buffered)
    }

    #[getter]
    fn max_frame(&self, py: Python<'_>) -> usize {
        crate::exit::detached(py, || self.lock().max_frame())
    }

    #[getter]
    fn failed(&self, py: Python<'_>) -> bool {
        crate::exit::detached(py, || self.lock().failed())
    }
}

#[pyfunction]
fn outgoing_frames<'py>(
    py: Python<'py>,
    channel: &Bound<'py, PyAny>,
    payload: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyList>> {
    let channel = channel_arg(channel)?;
    let frames = framing::outgoing_frames(channel, &bytes_like(payload)?).py()?;
    PyList::new(py, frames.iter().map(|frame| PyBytes::new(py, frame)))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyMessage>()?;
    m.add_class::<PyField>()?;
    m.add_class::<PyDecoder>()?;
    m.add_class::<PyIncomingFrame>()?;
    m.add_class::<PyIncomingFrames>()?;
    m.add_function(wrap_pyfunction!(fields, m)?)?;
    m.add_function(wrap_pyfunction!(uint32_array, m)?)?;
    m.add_function(wrap_pyfunction!(read_uint32_array, m)?)?;
    m.add_function(wrap_pyfunction!(protocol_upgrade, m)?)?;
    m.add_function(wrap_pyfunction!(hardware_info_request, m)?)?;
    m.add_function(wrap_pyfunction!(encode_payload, m)?)?;
    m.add_function(wrap_pyfunction!(outgoing_frames, m)?)?;
    m.add("MAX_PAYLOAD", MAX_PAYLOAD)?;
    m.add("HEADER_SIZE", ttp::HEADER_LEN)?;
    m.add("HARDWARE_INFO_COMMAND", ttp::HARDWARE_INFO_COMMAND)?;
    m.add("HARDWARE_INFO_MIN_PROTOCOL", ttp::HARDWARE_INFO_MIN_PROTOCOL)?;
    m.add("CHANNEL_COUNT", framing::CHANNEL_COUNT)?;
    m.add("MAX_FRAME", framing::MAX_FRAME)?;
    m.add("MAX_OUT_PAYLOAD", framing::MAX_OUT_PAYLOAD)?;
    Ok(())
}
