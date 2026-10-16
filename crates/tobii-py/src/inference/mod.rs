//! tobii_linux inference modules: one submodule per Python module, over tobii-inference.

use std::path::PathBuf;

use pyo3::exceptions::{PyImportError, PyOverflowError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_inference::{Error, Point, Runtime};

use crate::convert::bytes_like;

pub mod d1;
mod d1_quality;
mod face_detect;
mod legacy_ir;

pub fn error(error: Error) -> PyErr {
    match error {
        Error::Value(message) => PyValueError::new_err(message),
        Error::Overflow(message) => PyOverflowError::new_err(message),
        Error::Unavailable(message) => PyImportError::new_err(message),
        Error::OpenVino(message) => PyRuntimeError::new_err(message),
        Error::Io(error) => error.into(),
    }
}

pub trait InferenceResult<T> {
    fn py(self) -> PyResult<T>;
}

impl<T> InferenceResult<T> for Result<T, Error> {
    fn py(self) -> PyResult<T> {
        self.map_err(error)
    }
}

/// The shared OpenVINO runtime; an installed `openvino` wheel's `libs` directory is
/// searched after `TOBII_OPENVINO_LIBRARY`.
pub fn runtime(py: Python<'_>) -> PyResult<Runtime> {
    let spec = py.import("importlib.util")?.call_method1("find_spec", ("openvino",))?;
    let mut directories = Vec::new();
    if !spec.is_none() {
        for location in spec.getattr("submodule_search_locations")?.try_iter()? {
            directories.push(PathBuf::from(location?.extract::<String>()?).join("libs"));
        }
    }
    crate::exit::detached(py, || Runtime::shared(&directories)).py()
}

/// Little-endian float32 values of a bytes-like object.
pub fn floats(object: &Bound<'_, PyAny>) -> PyResult<Vec<f32>> {
    let data = bytes_like(object)?;
    if data.len() % 4 != 0 {
        return Err(PyValueError::new_err("Expected float32 data"));
    }
    Ok(data.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

pub fn float_bytes<'py>(py: Python<'py>, values: impl IntoIterator<Item = f32>) -> Bound<'py, PyBytes> {
    PyBytes::new(py, &values.into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>())
}

/// N×2 float32 points from an array's bytes and shape, or `None` for another shape.
pub fn points(data: &Bound<'_, PyAny>, shape: &[usize]) -> PyResult<Option<Vec<Point>>> {
    if shape.len() != 2 || shape[1] != 2 {
        return Ok(None);
    }
    Ok(Some(floats(data)?.as_chunks::<2>().0.to_vec()))
}

/// A 2×3 float32 transform from an array's bytes and shape, or `None` for another shape.
pub fn affine(data: &Bound<'_, PyAny>, shape: &[usize]) -> PyResult<Option<tobii_inference::Affine>> {
    if shape != [2, 3] {
        return Ok(None);
    }
    let values = floats(data)?;
    Ok(Some([[values[0], values[1], values[2]], [values[3], values[4], values[5]]]))
}

pub fn affine_bytes<'py>(py: Python<'py>, transform: &tobii_inference::Affine) -> Bound<'py, PyBytes> {
    float_bytes(py, transform.iter().flatten().copied())
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    crate::submodule(m, "legacy_ir", legacy_ir::register)?;
    crate::submodule(m, "d1", d1::register)?;
    crate::submodule(m, "d1_quality", d1_quality::register)?;
    crate::submodule(m, "face_detect", face_detect::register)
}
