//! Python exceptions for the core error types.
//!
//! USBError and ShortWriteError carry attributes and are constructed by Python code too,
//! so they are defined in Python and registered here; until then plain RuntimeError is
//! raised.

use pyo3::create_exception;
use pyo3::exceptions::{PyIndexError, PyRuntimeError, PyTimeoutError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBytes, PyTuple, PyType};
use tobii_device::DeviceError as CoreDeviceError;
use tobii_protocol::{FramingError, ProtocolError as CoreProtocolError};

create_exception!(tobii_linux.ttp, ProtocolError, pyo3::exceptions::PyValueError);
create_exception!(tobii_linux.driver, DeviceError, PyRuntimeError);
create_exception!(tobii_linux.driver, NoTrackerError, DeviceError);

static USB_ERROR: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static SHORT_WRITE_ERROR: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// Called by tobii_linux.usb and tobii_linux.usb_channel with their exception classes.
#[pyfunction]
#[pyo3(signature = (usb_error=None, short_write_error=None))]
fn register_errors(py: Python<'_>, usb_error: Option<Py<PyType>>, short_write_error: Option<Py<PyType>>) {
    if let Some(class) = usb_error {
        let _ = USB_ERROR.set(py, class);
    }
    if let Some(class) = short_write_error {
        let _ = SHORT_WRITE_ERROR.set(py, class);
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("ProtocolError", py.get_type::<ProtocolError>())?;
    m.add("DeviceError", py.get_type::<DeviceError>())?;
    m.add("NoTrackerError", py.get_type::<NoTrackerError>())?;
    m.add_function(wrap_pyfunction!(register_errors, m)?)?;
    Ok(())
}

pub fn usb_error_type(py: Python<'_>) -> Option<&Bound<'_, PyType>> {
    USB_ERROR.get(py).map(|class| class.bind(py))
}

pub fn protocol(error: CoreProtocolError) -> PyErr {
    ProtocolError::new_err(error.0)
}

pub fn protocol_message(message: &str) -> PyErr {
    ProtocolError::new_err(message.to_string())
}

pub fn framing(error: FramingError) -> PyErr {
    PyValueError::new_err(error.0)
}

fn instance<'py>(
    class: Option<&Py<PyType>>,
    py: Python<'py>,
    args: PyResult<Bound<'py, PyTuple>>,
    fallback: String,
) -> PyErr {
    let Some(class) = class else {
        return PyRuntimeError::new_err(fallback);
    };
    match args.and_then(|args| class.bind(py).call1(args)) {
        Ok(value) => PyErr::from_value(value),
        Err(error) => error,
    }
}

/// The original Python exception when a boxed error carries one.
fn foreign(error: Box<dyn std::error::Error + Send + Sync>) -> Result<PyErr, Box<dyn std::error::Error + Send + Sync>> {
    error.downcast::<PyErr>().map(|error| *error)
}

pub fn usb(error: tobii_usb::Error) -> PyErr {
    Python::attach(|py| match error {
        tobii_usb::Error::Usb(e) => {
            let message = e.to_string();
            let args = (e.operation, e.code, e.transferred, PyBytes::new(py, &e.partial_data)).into_pyobject(py);
            instance(USB_ERROR.get(py), py, args, message)
        }
        tobii_usb::Error::Invalid(message) => PyValueError::new_err(message),
        tobii_usb::Error::State(message) => PyRuntimeError::new_err(message),
        error @ tobii_usb::Error::ShortWrite { .. } => {
            let tobii_usb::Error::ShortWrite { transferred, expected } = error else { unreachable!() };
            instance(SHORT_WRITE_ERROR.get(py), py, (transferred, expected).into_pyobject(py), error.to_string())
        }
        error @ tobii_usb::Error::DeadlineExpired => PyTimeoutError::new_err(error.to_string()),
        tobii_usb::Error::Framing(error) => framing(error),
        tobii_usb::Error::Foreign(error) => match foreign(error) {
            Ok(error) => error,
            Err(other) => match other.downcast::<CoreProtocolError>() {
                Ok(error) => protocol(*error),
                Err(other) => match other.downcast::<std::io::Error>() {
                    Ok(error) => (*error).into(),
                    Err(other) => PyRuntimeError::new_err(other.to_string()),
                },
            },
        },
    })
}

pub fn device(error: CoreDeviceError) -> PyErr {
    match error {
        CoreDeviceError::NoTracker(message) => NoTrackerError::new_err(message),
        error @ CoreDeviceError::NoCamera { .. } => PyIndexError::new_err(error.to_string()),
        CoreDeviceError::Protocol(error) => protocol(error),
        CoreDeviceError::Usb(error) => usb(error),
        CoreDeviceError::Channel(error) => match foreign(error.source) {
            Ok(error) => error,
            Err(other) => match other.downcast::<tobii_usb::Error>() {
                Ok(error) => usb(*error),
                Err(other) => PyRuntimeError::new_err(other.to_string()),
            },
        },
        error => DeviceError::new_err(error.to_string()),
    }
}

pub trait IntoPyResult<T> {
    fn py(self) -> PyResult<T>;
}

impl<T> IntoPyResult<T> for Result<T, CoreProtocolError> {
    fn py(self) -> PyResult<T> {
        self.map_err(protocol)
    }
}

impl<T> IntoPyResult<T> for Result<T, FramingError> {
    fn py(self) -> PyResult<T> {
        self.map_err(framing)
    }
}

impl<T> IntoPyResult<T> for Result<T, tobii_usb::Error> {
    fn py(self) -> PyResult<T> {
        self.map_err(usb)
    }
}

impl<T> IntoPyResult<T> for Result<T, CoreDeviceError> {
    fn py(self) -> PyResult<T> {
        self.map_err(device)
    }
}
