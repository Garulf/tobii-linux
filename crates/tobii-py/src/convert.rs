//! Argument conversions with the error types and messages of the Python modules.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes};

use crate::errors::protocol_message;

/// Bytes from `bytes`, `bytearray` or any object `bytes()` accepts (memoryview, array).
pub fn bytes_like(object: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = object.cast::<PyBytes>() {
        return Ok(bytes.as_bytes().to_vec());
    }
    if let Ok(array) = object.cast::<PyByteArray>() {
        return Ok(array.to_vec());
    }
    let converted = object.py().get_type::<PyBytes>().call1((object,))?;
    Ok(converted.cast::<PyBytes>()?.as_bytes().to_vec())
}

pub fn int(object: &Bound<'_, PyAny>) -> Option<i128> {
    object.extract::<i64>().map(i128::from).ok().or_else(|| object.extract::<u64>().map(i128::from).ok())
}

/// An unsigned 32-bit integer, refused with ProtocolError like `ttp.uint32`.
pub fn uint32(object: &Bound<'_, PyAny>) -> PyResult<u32> {
    int(object)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| protocol_message("Expected an unsigned 32-bit integer"))
}

pub fn floats<const N: usize>(object: &Bound<'_, PyAny>) -> PyResult<[f64; N]> {
    let values: Vec<f64> = object.extract()?;
    values
        .try_into()
        .map_err(|values: Vec<f64>| PyValueError::new_err(format!("Expected {N} values, got {}", values.len())))
}
