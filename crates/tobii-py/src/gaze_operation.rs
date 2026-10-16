//! Shared by the gaze configuration modules: resumable device operations, their errors and
//! reply value conversions.
//!
//! An operation returned to Python is driven by a loop in the Python wrapper: each step
//! names an action as a tuple, the wrapper performs it on its session or UI and reports the
//! outcome. The Rust state machine never calls Python, so no Rust frame waits on Python code.

use std::sync::Mutex;

use pyo3::IntoPyObjectExt;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyOverflowError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyDict, PyFloat, PyInt, PyTuple};
use tobii_gaze::GazeError;
use tobii_gaze::device::StimulusPoint;
use tobii_gaze::operation::{Action, Call, External, Input, LogValue, Operation, OperationError, Step};
use tobii_protocol::fields::{StructHeader, Value};

use crate::errors::protocol;
use crate::fields::PyStructHeader;
use crate::ttp::{PyMessage, message_arg};

create_exception!(tobii_linux.calibration, CalibrationError, PyRuntimeError);
create_exception!(tobii_linux.calibration_profiles, ProfileError, PyValueError);

pub fn gaze_error(error: GazeError) -> PyErr {
    match error {
        GazeError::Protocol(error) => protocol(error),
        GazeError::Calibration(message) => CalibrationError::new_err(message),
        GazeError::Profile(message) => ProfileError::new_err(message),
        GazeError::Value(message) => PyValueError::new_err(message),
        error @ GazeError::Overflow => PyOverflowError::new_err(error.to_string()),
        GazeError::Io(error) => error.into(),
    }
}

pub trait IntoGazeResult<T> {
    fn gaze(self) -> PyResult<T>;
}

impl<T, E: Into<GazeError>> IntoGazeResult<T> for Result<T, E> {
    fn gaze(self) -> PyResult<T> {
        self.map_err(|error| gaze_error(error.into()))
    }
}

/// A Python exception raised by the wrapper's session or UI.
pub struct PyExternal {
    error: PyErr,
    message: String,
    interrupt: bool,
}

impl External for PyExternal {
    fn message(&self) -> String {
        self.message.clone()
    }

    fn is_interrupt(&self) -> bool {
        self.interrupt
    }
}

/// A reply value as `ttp_fields.reply_values` returns it.
pub fn value_arg(object: &Bound<'_, PyAny>) -> PyResult<Value> {
    if object.is_instance_of::<PyInt>() {
        return Ok(match object.extract::<i64>() {
            Ok(n) => Value::Int(n),
            Err(_) => Value::UInt(object.extract()?),
        });
    }
    if object.is_instance_of::<PyFloat>() {
        return Ok(Value::Float(object.extract()?));
    }
    if object.cast::<PyStructHeader>().is_ok() {
        let item_count = object.getattr("item_count")?.extract()?;
        let struct_id = object.getattr("struct_id")?.extract()?;
        return Ok(Value::Struct(StructHeader { item_count, struct_id }));
    }
    if let Ok(bytes) = object.cast::<PyBytes>() {
        return Ok(Value::Bytes(bytes.as_bytes().to_vec()));
    }
    if let Ok(bytes) = object.cast::<PyByteArray>() {
        return Ok(Value::Bytes(bytes.to_vec()));
    }
    if let Ok(items) = object.cast::<PyTuple>() {
        if let Ok(values) = items.extract::<Vec<u32>>() {
            return Ok(Value::UIntArray(values));
        }
        if let Ok(values) = items.extract::<Vec<f64>>() {
            return Ok(Value::FloatArray(values));
        }
        if let Ok(values) = items.extract::<Vec<Vec<u8>>>() {
            return Ok(Value::BytesArray(values));
        }
    }
    Err(PyTypeError::new_err(format!("Unexpected reply value {}", object.repr()?)))
}

pub fn values_arg(object: &Bound<'_, PyAny>) -> PyResult<Vec<Value>> {
    object.try_iter()?.map(|item| value_arg(&item?)).collect()
}

pub type EyeTuple = (i64, f64, f64);
pub type StimulusTuple = ((f64, f64, f64), EyeTuple, EyeTuple);

pub fn stimulus_tuple(point: &StimulusPoint) -> StimulusTuple {
    let eye = |q: &tobii_gaze::device::EyeQuality| (q.status, q.bias, q.precision);
    let [x, y, z] = point.point;
    ((x, y, z), eye(&point.left), eye(&point.right))
}

type Convert<T> = fn(Python<'_>, T) -> PyResult<Py<PyAny>>;
type Finish = Box<dyn FnOnce(Python<'_>) -> PyResult<Py<PyAny>> + Send>;

trait Resumable: Send {
    fn resume(&mut self, input: Input<PyExternal>, now: f64) -> Step<Finish, PyExternal>;
}

struct Typed<T> {
    operation: Operation<T, PyExternal>,
    convert: Convert<T>,
}

impl<T: Send + 'static> Resumable for Typed<T> {
    fn resume(&mut self, input: Input<PyExternal>, now: f64) -> Step<Finish, PyExternal> {
        match self.operation.resume(input, now) {
            Step::Action(action) => Step::Action(action),
            Step::Finished(result) => {
                let convert = self.convert;
                Step::Finished(result.map(|value| -> Finish { Box::new(move |py| convert(py, value)) }))
            }
        }
    }
}

struct State {
    operation: Box<dyn Resumable>,
    call: Option<Call>,
}

/// A device procedure in progress; see the module documentation.
#[pyclass(frozen, name = "Operation", module = "tobii_linux._native.gaze_device")]
pub struct PyOperation(Mutex<State>);

impl PyOperation {
    pub fn new<T: Send + 'static>(operation: Operation<T, PyExternal>, convert: Convert<T>) -> Self {
        Self(Mutex::new(State { operation: Box::new(Typed { operation, convert }), call: None }))
    }

    fn resume(&self, py: Python<'_>, input: Input<PyExternal>, now: f64) -> PyResult<Py<PyAny>> {
        let step = crate::exit::detached(py, || {
            let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let step = state.operation.resume(input, now);
            state.call = match &step {
                Step::Action(Action::Call(call)) => Some(call.clone()),
                _ => None,
            };
            step
        });
        match step {
            Step::Action(action) => action_tuple(py, action),
            Step::Finished(Ok(finish)) => ("done", finish(py)?).into_py_any(py),
            Step::Finished(Err(error)) => finished_with(py, error),
        }
    }
}

fn exception(error: OperationError<PyExternal>) -> PyErr {
    match error {
        OperationError::Gaze(error) => gaze_error(error),
        OperationError::External(external) => external.error,
        OperationError::Failed { step, cause } => {
            let cause = exception(*cause);
            CalibrationError::new_err(format!("{step} failed: {cause}"))
        }
    }
}

/// `("failed", step, cause)` for a calibration step failure; other errors are raised.
fn finished_with(py: Python<'_>, error: OperationError<PyExternal>) -> PyResult<Py<PyAny>> {
    match error {
        OperationError::Failed { step, cause } => {
            let cause = exception(*cause).into_value(py);
            ("failed", step, cause).into_py_any(py)
        }
        error => Err(exception(error)),
    }
}

fn log_value(py: Python<'_>, value: LogValue) -> PyResult<Py<PyAny>> {
    match value {
        LogValue::Int(n) => n.into_py_any(py),
        LogValue::Float(x) => x.into_py_any(py),
        LogValue::Bool(flag) => flag.into_py_any(py),
        LogValue::Text(text) => text.into_py_any(py),
        LogValue::StimulusPoints(points) => points.iter().map(stimulus_tuple).collect::<Vec<_>>().into_py_any(py),
    }
}

fn action_tuple(py: Python<'_>, action: Action) -> PyResult<Py<PyAny>> {
    match action {
        Action::Call(call) => ("call", call.timeout_s, call.tolerant).into_py_any(py),
        Action::Ui(event) => {
            let name = event.name();
            ("ui", name, crate::calibration::ui_arguments(py, event)?).into_py_any(py)
        }
        Action::Check => ("check",).into_py_any(py),
        Action::Read => ("read",).into_py_any(py),
        Action::PendingStreams { take } => ("streams", take).into_py_any(py),
        Action::ClearStreams => ("clear",).into_py_any(py),
        Action::Log(record) => {
            let values = PyDict::new(py);
            for (name, value) in record.values {
                values.set_item(name, log_value(py, value)?)?;
            }
            ("log", record.kind, values).into_py_any(py)
        }
    }
}

#[pymethods]
impl PyOperation {
    fn start(&self, py: Python<'_>, now: f64) -> PyResult<Py<PyAny>> {
        self.resume(py, Input::Start, now)
    }

    fn replied(&self, py: Python<'_>, values: &Bound<'_, PyAny>, now: f64) -> PyResult<Py<PyAny>> {
        let values = values_arg(values)?;
        self.resume(py, Input::Replied(values), now)
    }

    /// The pending call or UI callback raised `error`.
    fn failed(&self, py: Python<'_>, error: &Bound<'_, PyAny>, now: f64) -> PyResult<Py<PyAny>> {
        let interrupt = !error.is_instance_of::<PyException>();
        let message = error.str()?.to_string();
        let external = PyExternal { error: PyErr::from_value(error.clone()), message, interrupt };
        self.resume(py, Input::Failed(external), now)
    }

    fn done(&self, py: Python<'_>, now: f64) -> PyResult<Py<PyAny>> {
        self.resume(py, Input::Done, now)
    }

    fn streams(&self, py: Python<'_>, messages: &Bound<'_, PyAny>, now: f64) -> PyResult<Py<PyAny>> {
        let messages = messages.try_iter()?.map(|m| message_arg(&m?)).collect::<PyResult<Vec<_>>>()?;
        self.resume(py, Input::Streams(messages), now)
    }

    /// The pending request with `request_id`, for `session.call(operation.request)`.
    fn request<'py>(&self, py: Python<'py>, request_id: u32) -> PyResult<Bound<'py, PyMessage>> {
        let call =
            crate::exit::detached(py, || self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).call.clone());
        let call = call.ok_or_else(|| PyRuntimeError::new_err("The operation is not waiting for a call"))?;
        PyMessage::into_bound(py, &call.request(request_id))
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyOperation>()
}
