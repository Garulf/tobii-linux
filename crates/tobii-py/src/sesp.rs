//! tobii_linux.sesp: SESP framing, flatcc-shaped buffers, the service messages and the
//! table reader.
//!
//! `Scalar`, `Reference`, `HeadPose` and `Message` stay Python dataclasses; tables are the
//! native `Table` class reading the buffer they were decoded from.

use std::sync::Mutex;

use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PySet, PyTuple};
use tobii_service::sesp::{self, Field, FlatccBuilder, HeadPose, Kind, Table, Value};

use crate::convert::{bytes_like, floats};
use crate::errors::IntoPyResult;
use crate::prp::masked_i64;

create_exception!(tobii_linux.sesp, SespError, PyValueError);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn kind(form: &str) -> PyResult<Kind> {
    Ok(match form {
        "B" => Kind::U8,
        "b" => Kind::I8,
        "H" => Kind::U16,
        "h" => Kind::I16,
        "I" => Kind::U32,
        "i" => Kind::I32,
        "f" => Kind::F32,
        "Q" => Kind::U64,
        "q" => Kind::I64,
        "d" => Kind::F64,
        other => return Err(PyValueError::new_err(format!("Unsupported scalar format {other:?}"))),
    })
}

fn value(form: &str, object: &Bound<'_, PyAny>) -> PyResult<Value> {
    Ok(match kind(form)? {
        Kind::U8 => Value::U8(object.extract()?),
        Kind::I8 => Value::I8(object.extract()?),
        Kind::U16 => Value::U16(object.extract()?),
        Kind::I16 => Value::I16(object.extract()?),
        Kind::U32 => Value::U32(object.extract()?),
        Kind::I32 => Value::I32(object.extract()?),
        Kind::F32 => Value::F32(object.extract::<f64>()? as f32),
        Kind::U64 => Value::U64(object.extract()?),
        Kind::I64 => Value::I64(object.extract()?),
        Kind::F64 => Value::F64(object.extract()?),
    })
}

fn value_object<'py>(py: Python<'py>, value: Value) -> PyResult<Bound<'py, PyAny>> {
    Ok(match value {
        Value::U8(v) => v.into_pyobject(py)?.into_any(),
        Value::I8(v) => v.into_pyobject(py)?.into_any(),
        Value::U16(v) => v.into_pyobject(py)?.into_any(),
        Value::I16(v) => v.into_pyobject(py)?.into_any(),
        Value::U32(v) => v.into_pyobject(py)?.into_any(),
        Value::I32(v) => v.into_pyobject(py)?.into_any(),
        Value::F32(v) => f64::from(v).into_pyobject(py)?.into_any(),
        Value::U64(v) => v.into_pyobject(py)?.into_any(),
        Value::I64(v) => v.into_pyobject(py)?.into_any(),
        Value::F64(v) => v.into_pyobject(py)?.into_any(),
    })
}

/// A `Scalar(index, format, value)` or `Reference(index, position)` dataclass.
fn field_arg(field: &Bound<'_, PyAny>) -> PyResult<Field> {
    let index: usize = field.getattr("index")?.extract()?;
    if field.hasattr("position")? {
        return Ok(Field::Reference { index, position: field.getattr("position")?.extract()? });
    }
    let form: String = field.getattr("format")?.extract()?;
    Ok(Field::Scalar { index, value: value(&form, &field.getattr("value")?)? })
}

fn fields_arg(fields: &Bound<'_, PyAny>) -> PyResult<Vec<Field>> {
    fields.try_iter()?.map(|field| field_arg(&field?)).collect()
}

/// Reassembles messages from pipe reads of any size, as sesp_parse_message does.
#[pyclass(frozen, name = "FrameDecoder", module = "tobii_linux.sesp")]
pub struct PyFrameDecoder(Mutex<sesp::FrameDecoder>);

#[pymethods]
impl PyFrameDecoder {
    #[new]
    #[pyo3(signature = (limit=sesp::MAX_MESSAGE))]
    fn new(limit: usize) -> Self {
        Self(Mutex::new(sesp::FrameDecoder::new(limit)))
    }

    fn feed<'py>(&self, py: Python<'py>, data: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, PyBytes>>> {
        let data = bytes_like(data)?;
        let buffers = crate::exit::detached(py, || lock(&self.0).feed(&data)).py()?;
        Ok(buffers.iter().map(|buffer| PyBytes::new(py, buffer)).collect())
    }

    #[getter]
    fn pending<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let pending = crate::exit::detached(py, || lock(&self.0).pending().to_vec());
        PyBytes::new(py, &pending)
    }

    #[getter]
    fn limit(&self, py: Python<'_>) -> usize {
        crate::exit::detached(py, || lock(&self.0).limit())
    }
}

/// Back-to-front FlatBuffers builder with flatcc's layout for small tables.
#[pyclass(frozen, name = "FlatccBuilder", module = "tobii_linux.sesp")]
pub struct PyFlatccBuilder(Mutex<FlatccBuilder>);

#[pymethods]
impl PyFlatccBuilder {
    #[new]
    fn new() -> Self {
        Self(Mutex::new(FlatccBuilder::default()))
    }

    fn string(&self, py: Python<'_>, text: &Bound<'_, PyAny>) -> PyResult<i64> {
        let text = bytes_like(text)?;
        Ok(crate::exit::detached(py, || lock(&self.0).string(&text)))
    }

    /// Table of `fields` (Scalar or Reference) in the order the original builder adds them.
    fn table(&self, py: Python<'_>, fields: &Bound<'_, PyAny>) -> PyResult<i64> {
        let fields = fields_arg(fields)?;
        Ok(crate::exit::detached(py, || lock(&self.0).table(&fields)))
    }

    fn finish<'py>(&self, py: Python<'py>, root: i64) -> Bound<'py, PyBytes> {
        let buffer = crate::exit::detached(py, || lock(&self.0).finish(root));
        PyBytes::new(py, &buffer)
    }
}

/// Field access on one FlatBuffers table, bounds-checked.
#[pyclass(frozen, name = "Table", module = "tobii_linux.sesp")]
pub struct PyTable {
    buffer: Py<PyBytes>,
    position: usize,
}

impl PyTable {
    fn create(py: Python<'_>, buffer: Py<PyBytes>, position: i64) -> PyResult<Self> {
        let position = Table::new(buffer.as_bytes(py), position).py()?.position();
        Ok(Self { buffer, position })
    }

    fn with<'a, T>(&'a self, py: Python<'_>, read: impl FnOnce(Table<'a>) -> T) -> T {
        read(Table::new(self.buffer.as_bytes(py), self.position as i64).expect("validated on creation"))
    }

    fn sibling(&self, py: Python<'_>, table: Option<Table>) -> Option<Self> {
        table.map(|table| Self { buffer: self.buffer.clone_ref(py), position: table.position() })
    }
}

#[pymethods]
impl PyTable {
    #[new]
    fn new(py: Python<'_>, buffer: &Bound<'_, PyAny>, position: i64) -> PyResult<Self> {
        Self::create(py, PyBytes::new(py, &bytes_like(buffer)?).unbind(), position)
    }

    #[pyo3(signature = (index, form, default=None))]
    fn scalar<'py>(
        &self,
        py: Python<'py>,
        index: usize,
        form: &str,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let kind = kind(form)?;
        match self.with(py, |table| table.scalar(index, kind)).py()? {
            Some(value) => value_object(py, value),
            None => Ok(default.unwrap_or_else(|| 0i64.into_pyobject(py).expect("int").into_any())),
        }
    }

    fn table(&self, py: Python<'_>, index: usize) -> PyResult<Option<Self>> {
        let table = self.with(py, |table| table.table(index)).py()?;
        Ok(self.sibling(py, table))
    }

    fn string<'py>(&self, py: Python<'py>, index: usize) -> PyResult<Option<Bound<'py, PyBytes>>> {
        let text = self.with(py, |table| table.string(index).map(|text| text.map(<[u8]>::to_vec))).py()?;
        Ok(text.map(|text| PyBytes::new(py, &text)))
    }

    #[getter]
    fn buffer(&self, py: Python<'_>) -> Py<PyBytes> {
        self.buffer.clone_ref(py)
    }

    #[getter]
    fn position(&self) -> usize {
        self.position
    }

    #[getter]
    fn vtable(&self, py: Python<'_>) -> usize {
        self.with(py, |table| table.vtable())
    }

    #[getter]
    fn vtable_size(&self, py: Python<'_>) -> usize {
        self.with(py, |table| table.vtable_size())
    }

    #[getter]
    fn table_size(&self, py: Python<'_>) -> usize {
        self.with(py, |table| table.table_size())
    }
}

fn payload_table<'a>(py: Python<'a>, payload: &'a Option<Bound<'a, PyTable>>) -> PyResult<Option<Table<'a>>> {
    payload
        .as_ref()
        .map(|table| {
            let table = table.get();
            Table::new(table.buffer.as_bytes(py), table.position as i64).py()
        })
        .transpose()
}

#[pyfunction]
fn frame<'py>(py: Python<'py>, buffer: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &sesp::frame(&bytes_like(buffer)?)))
}

/// One framed message: payload table, then the root table {sequence, type, payload}.
#[pyfunction]
#[pyo3(signature = (sequence, kind, payload_fields, strings=None))]
fn message<'py>(
    py: Python<'py>,
    sequence: u32,
    kind: u8,
    payload_fields: &Bound<'py, PyAny>,
    strings: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyBytes>> {
    let fields = fields_arg(payload_fields)?;
    let strings: Vec<(usize, Vec<u8>)> = match strings {
        Some(strings) => strings
            .try_iter()?
            .map(|entry| {
                let entry = entry?;
                Ok((entry.get_item(0)?.extract()?, bytes_like(&entry.get_item(1)?)?))
            })
            .collect::<PyResult<_>>()?,
        None => Vec::new(),
    };
    Ok(PyBytes::new(py, &sesp::message(sequence, kind, &fields, &strings)))
}

#[pyfunction]
fn wire_error(error: i64) -> u32 {
    sesp::wire_error(error)
}

/// Type 3: error, then capability flags 1..15 (`capabilities` lists the set ones).
#[pyfunction]
#[pyo3(signature = (sequence, error, capabilities=None))]
fn encode_initialize_response<'py>(
    py: Python<'py>,
    sequence: u32,
    error: i64,
    capabilities: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyBytes>> {
    let mut flags = Vec::new();
    if let Some(capabilities) = capabilities {
        for index in 1..=sesp::CAPABILITY_COUNT {
            if capabilities.contains(index)? {
                flags.push(index);
            }
        }
    }
    Ok(PyBytes::new(py, &sesp::encode_initialize_response(sequence, error, &flags)))
}

#[pyfunction]
fn encode_error_response(py: Python<'_>, sequence: u32, kind: u8, error: i64) -> Bound<'_, PyBytes> {
    PyBytes::new(py, &sesp::encode_error_response(sequence, kind, error))
}

#[pyfunction]
fn encode_stream_status_response(py: Python<'_>, sequence: u32) -> Bound<'_, PyBytes> {
    PyBytes::new(py, &sesp::encode_stream_status_response(sequence))
}

fn validity(object: &Bound<'_, PyAny>) -> PyResult<i32> {
    Ok(if object.eq(sesp::VALID)? { sesp::VALID } else { sesp::INVALID })
}

fn head_pose_arg(pose: &Bound<'_, PyAny>) -> PyResult<HeadPose> {
    let rotation_validity: Vec<Bound<'_, PyAny>> =
        pose.getattr("rotation_validity")?.try_iter()?.collect::<PyResult<_>>()?;
    let rotation_validity: [Bound<'_, PyAny>; 3] = rotation_validity
        .try_into()
        .map_err(|values: Vec<_>| PyValueError::new_err(format!("Expected 3 values, got {}", values.len())))?;
    let [first, second, third] = &rotation_validity;
    Ok(HeadPose {
        timestamp_us: masked_i64(&pose.getattr("timestamp_us")?)?,
        position: floats::<3>(&pose.getattr("position")?)?,
        rotation: floats::<3>(&pose.getattr("rotation")?)?,
        position_validity: validity(&pose.getattr("position_validity")?)?,
        rotation_validity: [validity(first)?, validity(second)?, validity(third)?],
    })
}

/// Type 8: timestamp high and low words, position, rotation, then the validities.
#[pyfunction]
fn encode_head_pose<'py>(py: Python<'py>, sequence: u32, pose: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &sesp::encode_head_pose(sequence, &head_pose_arg(pose)?)))
}

/// `(sequence, kind, payload table)` of one buffer.
#[pyfunction]
fn decode(py: Python<'_>, buffer: &Bound<'_, PyAny>) -> PyResult<(u32, u8, Option<PyTable>)> {
    let buffer = PyBytes::new(py, &bytes_like(buffer)?).unbind();
    let message = sesp::decode(buffer.as_bytes(py)).py()?;
    let (sequence, kind) = (message.sequence, message.kind);
    let payload = message.payload.map(|table| table.position());
    Ok((sequence, kind, payload.map(|position| PyTable { buffer: buffer.clone_ref(py), position })))
}

#[pyfunction]
#[pyo3(signature = (kind, payload=None))]
fn initialize_request(py: Python<'_>, kind: u8, payload: Option<Bound<'_, PyTable>>) -> PyResult<(String, u32)> {
    let request = sesp::initialize_request(kind, payload_table(py, &payload)?.as_ref()).py()?;
    Ok((request.device_url, request.subscription_id))
}

type HeadPoseTuple = (i64, (f64, f64, f64), (f64, f64, f64), i32, (i32, i32, i32));

#[pyfunction]
#[pyo3(signature = (kind, payload=None))]
fn head_pose(py: Python<'_>, kind: u8, payload: Option<Bound<'_, PyTable>>) -> PyResult<HeadPoseTuple> {
    let pose = sesp::head_pose(kind, payload_table(py, &payload)?.as_ref()).py()?;
    let triple = |[a, b, c]: [f64; 3]| (a, b, c);
    let [first, second, third] = pose.rotation_validity;
    Ok((
        pose.timestamp_us,
        triple(pose.position),
        triple(pose.rotation),
        pose.position_validity,
        (first, second, third),
    ))
}

#[pyfunction]
#[pyo3(signature = (kind, payload=None))]
fn capabilities<'py>(
    py: Python<'py>,
    kind: u8,
    payload: Option<Bound<'py, PyTable>>,
) -> PyResult<(u32, Bound<'py, PySet>)> {
    let (error, flags) = sesp::capabilities(kind, payload_table(py, &payload)?.as_ref()).py()?;
    Ok((error, PySet::new(py, flags)?))
}

#[pyfunction]
fn decode_hello(data: &Bound<'_, PyAny>) -> PyResult<(u32, String)> {
    let hello = sesp::decode_hello(&bytes_like(data)?).py()?;
    Ok((hello.channel, hello.reply_pipe))
}

#[pyfunction]
fn encode_hello<'py>(py: Python<'py>, channel: u32, reply_pipe: &str) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &sesp::encode_hello(channel, reply_pipe).py()?))
}

#[pyfunction]
fn encode_slot(py: Python<'_>, index: u32) -> Bound<'_, PyBytes> {
    PyBytes::new(py, &sesp::encode_slot(index))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("SespError", m.py().get_type::<SespError>())?;
    m.add_class::<PyFrameDecoder>()?;
    m.add_class::<PyFlatccBuilder>()?;
    m.add_class::<PyTable>()?;
    for function in [
        wrap_pyfunction!(frame, m)?,
        wrap_pyfunction!(message, m)?,
        wrap_pyfunction!(wire_error, m)?,
        wrap_pyfunction!(encode_initialize_response, m)?,
        wrap_pyfunction!(encode_error_response, m)?,
        wrap_pyfunction!(encode_stream_status_response, m)?,
        wrap_pyfunction!(encode_head_pose, m)?,
        wrap_pyfunction!(decode, m)?,
        wrap_pyfunction!(initialize_request, m)?,
        wrap_pyfunction!(head_pose, m)?,
        wrap_pyfunction!(capabilities, m)?,
        wrap_pyfunction!(decode_hello, m)?,
        wrap_pyfunction!(encode_hello, m)?,
        wrap_pyfunction!(encode_slot, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("MAGIC", sesp::MAGIC)?;
    m.add("MAX_MESSAGE", sesp::MAX_MESSAGE)?;
    m.add("HEADER_SIZE", sesp::HEADER_SIZE)?;
    m.add("CAPABILITY_COUNT", sesp::CAPABILITY_COUNT)?;
    m.add("PIPE_NAME", sesp::PIPE_NAME)?;
    m.add("CHANNELS", PyTuple::new(m.py(), sesp::CHANNELS)?)?;
    Ok(())
}
