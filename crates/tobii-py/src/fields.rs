//! tobii_linux.ttp_fields: typed items, the response reader and reply statuses.

use std::sync::Mutex;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
use tobii_protocol::fields::{self, StructHeader, Value};
use tobii_protocol::numeric;

use crate::convert::{bytes_like, floats, int, uint32};
use crate::errors::{IntoPyResult, protocol_message};
use crate::ttp::{PyField, field_arg, fields_arg, message_arg};

#[pyclass(frozen, eq, hash, name = "StructHeader", module = "tobii_linux.ttp_fields")]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PyStructHeader {
    #[pyo3(get)]
    item_count: u32,
    #[pyo3(get)]
    struct_id: u32,
}

impl From<StructHeader> for PyStructHeader {
    fn from(header: StructHeader) -> Self {
        Self { item_count: header.item_count, struct_id: header.struct_id }
    }
}

#[pymethods]
impl PyStructHeader {
    #[new]
    fn new(item_count: u32, struct_id: u32) -> Self {
        Self { item_count, struct_id }
    }

    #[classmethod]
    fn from_word(_class: &Bound<'_, pyo3::types::PyType>, word: u32) -> Self {
        StructHeader::from_word(word).into()
    }

    #[getter]
    fn word(&self) -> PyResult<u32> {
        StructHeader { item_count: self.item_count, struct_id: self.struct_id }.word().py()
    }

    fn __repr__(&self) -> String {
        format!("StructHeader(item_count={}, struct_id={})", self.item_count, self.struct_id)
    }
}

pub fn value_object<'py>(py: Python<'py>, value: Value) -> PyResult<Bound<'py, PyAny>> {
    Ok(match value {
        Value::Int(n) => n.into_pyobject(py)?.into_any(),
        Value::UInt(n) => n.into_pyobject(py)?.into_any(),
        Value::Float(x) => x.into_pyobject(py)?.into_any(),
        Value::Struct(header) => Bound::new(py, PyStructHeader::from(header))?.into_any(),
        Value::Bytes(bytes) => PyBytes::new(py, &bytes).into_any(),
        Value::UIntArray(values) => PyTuple::new(py, values)?.into_any(),
        Value::FloatArray(values) => PyTuple::new(py, values)?.into_any(),
        Value::BytesArray(items) => PyTuple::new(py, items.iter().map(|item| PyBytes::new(py, item)))?.into_any(),
    })
}

#[pyfunction]
fn value<'py>(py: Python<'py>, field: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    value_object(py, fields::value(&field_arg(field)?).py()?)
}

/// Every item of a reply payload, decoded.
#[pyfunction]
pub fn reply_values<'py>(py: Python<'py>, payload: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
    let values = fields::values(&bytes_like(payload)?).py()?;
    PyList::new(py, values.into_iter().map(|v| value_object(py, v)).collect::<PyResult<Vec<_>>>()?)
}

#[pyfunction]
fn float32(value: f64) -> f64 {
    numeric::round_f32(value)
}

#[pyfunction]
fn fixed16(raw: i32) -> f64 {
    numeric::fixed16(raw)
}

#[pyfunction]
fn fixed42(raw: i64) -> f64 {
    numeric::fixed42(raw)
}

#[pyfunction]
fn fixed16_raw(number: f64) -> PyResult<i32> {
    fields::fixed16_raw(number).py()
}

#[pyfunction]
fn fixed42_raw(number: f64) -> PyResult<i64> {
    fields::fixed42_raw(number).py()
}

fn int32_arg(number: &Bound<'_, PyAny>) -> PyResult<i32> {
    int(number).and_then(|n| i32::try_from(n).ok()).ok_or_else(|| protocol_message("Expected a signed 32-bit integer"))
}

#[pyfunction]
fn int32_field(number: &Bound<'_, PyAny>) -> PyResult<PyField> {
    Ok(PyField::from_core(&fields::int32_field(int32_arg(number)?)))
}

#[pyfunction]
fn uint32_field(number: &Bound<'_, PyAny>) -> PyResult<PyField> {
    Ok(PyField::from_core(&fields::uint32_field(uint32(number)?)))
}

#[pyfunction]
fn fixed16_field(number: f64) -> PyResult<PyField> {
    Ok(PyField::from_core(&fields::fixed16_field(number).py()?))
}

#[pyfunction]
fn fixed42_field(number: f64) -> PyResult<PyField> {
    Ok(PyField::from_core(&fields::fixed42_field(number).py()?))
}

#[pyfunction]
fn struct_field(item_count: u32, struct_id: u32) -> PyResult<PyField> {
    Ok(PyField::from_core(&fields::struct_field(item_count, struct_id).py()?))
}

#[pyfunction]
#[pyo3(signature = (count, descriptor=None))]
fn array_fields(count: &Bound<'_, PyAny>, descriptor: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<PyField>> {
    let descriptor = descriptor.map_or(Ok(0), uint32)?;
    let count = int(count)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| protocol_message("Array count does not fit the header"))?;
    Ok(fields::array_fields(count, descriptor).py()?.iter().map(PyField::from_core).collect())
}

#[pyfunction]
fn fixed42_array_field(numbers: Vec<f64>) -> PyResult<PyField> {
    Ok(PyField::from_core(&fields::fixed42_array_field(&numbers).py()?))
}

#[pyfunction]
fn triple_fixed42_fields(numbers: &Bound<'_, PyAny>) -> PyResult<Vec<PyField>> {
    Ok(fields::triple_fixed42_fields(floats::<3>(numbers)?).py()?.iter().map(PyField::from_core).collect())
}

#[pyfunction]
fn response_status(message: &Bound<'_, PyAny>) -> PyResult<u32> {
    Ok(fields::response_status(&message_arg(message)?))
}

#[pyfunction]
fn status_name(status: i64) -> String {
    fields::status_name(status)
}

/// Sequential typed reader over a response payload, in schema order.
#[pyclass(frozen, name = "Reader", module = "tobii_linux.ttp_fields")]
pub struct PyReader {
    payload: Vec<u8>,
    offset: Mutex<usize>,
}

impl PyReader {
    pub fn read<T>(&self, read: impl FnOnce(&mut fields::Reader) -> tobii_protocol::error::Result<T>) -> PyResult<T> {
        let mut offset = self.offset.lock().unwrap_or_else(|p| p.into_inner());
        let mut reader = fields::Reader::resume(&self.payload, *offset).py()?;
        let result = read(&mut reader);
        *offset = reader.offset();
        result.py()
    }
}

#[pymethods]
impl PyReader {
    #[new]
    fn new(payload: &Bound<'_, PyAny>) -> PyResult<Self> {
        let payload = bytes_like(payload)?;
        fields::Reader::new(&payload).py()?;
        Ok(Self { payload, offset: Mutex::new(2) })
    }

    #[getter]
    fn payload<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.payload)
    }

    #[getter]
    fn offset(&self) -> usize {
        *self.offset.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[getter]
    fn remaining(&self) -> usize {
        self.payload.len() - self.offset()
    }

    fn item(&self, wire: u8) -> PyResult<PyField> {
        self.read(|r| r.item(wire)).map(|field| PyField::from_core(&field))
    }

    fn int32(&self) -> PyResult<i32> {
        self.read(|r| r.int32())
    }

    fn uint32(&self) -> PyResult<u32> {
        self.read(|r| r.uint32())
    }

    fn fixed16(&self) -> PyResult<f64> {
        self.read(|r| r.fixed16())
    }

    fn fixed42(&self) -> PyResult<f64> {
        self.read(|r| r.fixed42())
    }

    #[pyo3(name = "struct")]
    fn structure(&self, header: u32) -> PyResult<()> {
        self.read(|r| r.structure(header))
    }

    fn array(&self) -> PyResult<u32> {
        self.read(|r| r.array())
    }

    fn fixed42_array<'py>(&self, py: Python<'py>, limit: usize) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.read(|r| r.fixed42_array(limit))?)
    }

    fn triple_fixed42(&self) -> PyResult<(f64, f64, f64)> {
        self.read(|r| r.triple_fixed42()).map(|[x, y, z]| (x, y, z))
    }
}

#[pyfunction]
fn encode_payload<'py>(py: Python<'py>, items: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &fields::encode_payload(&fields_arg(items)?).py()?))
}

fn tables(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    let response_types = PyDict::new(py);
    for (command, types) in fields::RESPONSE_FIELD_TYPES {
        response_types.set_item(command, PyTuple::new(py, types)?)?;
    }
    m.add("RESPONSE_FIELD_TYPES", response_types)?;
    m.add("DEVICE_ERROR_STATUS", PyDict::from_sequence(&PyList::new(py, fields::DEVICE_ERROR_STATUS)?.into_any())?)?;
    m.add("PARSER_KINDS", PyDict::from_sequence(&PyList::new(py, fields::PARSER_KINDS)?.into_any())?)?;
    m.add("PARSER_STATUS_NAMES", PyTuple::new(py, fields::PARSER_STATUS_NAMES)?)?;
    Ok(())
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyStructHeader>()?;
    m.add_class::<PyReader>()?;
    for function in [
        wrap_pyfunction!(value, m)?,
        wrap_pyfunction!(reply_values, m)?,
        wrap_pyfunction!(float32, m)?,
        wrap_pyfunction!(fixed16, m)?,
        wrap_pyfunction!(fixed42, m)?,
        wrap_pyfunction!(fixed16_raw, m)?,
        wrap_pyfunction!(fixed42_raw, m)?,
        wrap_pyfunction!(int32_field, m)?,
        wrap_pyfunction!(uint32_field, m)?,
        wrap_pyfunction!(fixed16_field, m)?,
        wrap_pyfunction!(fixed42_field, m)?,
        wrap_pyfunction!(struct_field, m)?,
        wrap_pyfunction!(array_fields, m)?,
        wrap_pyfunction!(fixed42_array_field, m)?,
        wrap_pyfunction!(triple_fixed42_fields, m)?,
        wrap_pyfunction!(response_status, m)?,
        wrap_pyfunction!(status_name, m)?,
        wrap_pyfunction!(encode_payload, m)?,
    ] {
        m.add_function(function)?;
    }
    tables(m)?;
    Ok(())
}
