//! tobii_linux model and asset modules: one submodule per Python module, over tobii-models.

use pyo3::create_exception;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use tobii_models::{Error, FileTree};

use crate::convert::bytes_like;

mod face_assets;
mod lbf;
mod model_digests;


pub fn error(error: Error) -> PyErr {
    match error {
        Error::Format(message) => PyValueError::new_err(message),
        Error::Io(error) => error.into(),
    }
}

pub trait ModelResult<T> {
    fn py(self) -> PyResult<T>;
}

impl<T> ModelResult<T> for Result<T, Error> {
    fn py(self) -> PyResult<T> {
        self.map_err(error)
    }
}

/// `{path: bytes}` dict from a Python mapping of paths to bytes-like values.
pub fn tree_arg(tree: &Bound<'_, PyDict>) -> PyResult<FileTree> {
    tree.iter().map(|(path, data)| Ok((path.extract::<String>()?, bytes_like(&data)?))).collect()
}

pub fn tree_dict<'py>(py: Python<'py>, tree: &FileTree) -> PyResult<Bound<'py, PyDict>> {
    let result = PyDict::new(py);
    for (path, data) in tree.iter() {
        result.set_item(path, PyBytes::new(py, data))?;
    }
    Ok(result)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    crate::submodule(m, "model_digests", model_digests::register)?;
    crate::submodule(m, "face_assets", face_assets::register)?;
    crate::submodule(m, "lbf", lbf::register)
}
