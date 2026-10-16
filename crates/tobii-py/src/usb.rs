//! tobii_linux.usb and tobii_linux.usb_channel: probing, the bulk transport and channels.

use std::sync::{Arc, Mutex, MutexGuard};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
use tobii_usb::traffic::{self, LoggingTransport, TrafficLog};
use tobii_usb::{Alternate, BulkTransport, Endpoint, ProbedDevice, Transport, UsbChannel, UsbError};

use crate::convert::{bytes_like, int};
use crate::errors::{IntoPyResult, usb_error_type};

type EndpointTuple = (u8, u8, u16, u8);

fn endpoint_tuple(ep: &Endpoint) -> EndpointTuple {
    (ep.address, ep.attributes, ep.max_packet_size, ep.interval)
}

fn device_dict<'py>(py: Python<'py>, device: &ProbedDevice) -> PyResult<Bound<'py, PyDict>> {
    let descriptor = PyDict::new(py);
    for (name, value) in device.device.fields() {
        descriptor.set_item(name, value)?;
    }
    let configurations = PyList::empty(py);
    for configuration in &device.configurations {
        let interfaces = PyList::empty(py);
        for alt in &configuration.interfaces {
            let entry = PyDict::new(py);
            entry.set_item("interface", alt.interface)?;
            entry.set_item("alternate", alt.alternate)?;
            entry.set_item("interface_class", alt.interface_class)?;
            let endpoints = PyList::empty(py);
            for ep in &alt.endpoints {
                let item = PyDict::new(py);
                item.set_item("address", ep.address)?;
                item.set_item("attributes", ep.attributes)?;
                item.set_item("max_packet_size", ep.max_packet_size)?;
                item.set_item("interval", ep.interval)?;
                endpoints.append(item)?;
            }
            entry.set_item("endpoints", endpoints)?;
            interfaces.append(entry)?;
        }
        let item = PyDict::new(py);
        item.set_item("value", configuration.value)?;
        item.set_item("interfaces", interfaces)?;
        configurations.append(item)?;
    }
    let item = PyDict::new(py);
    item.set_item("bus", device.bus)?;
    item.set_item("address", device.address)?;
    item.set_item("device", descriptor)?;
    item.set_item("configurations", configurations)?;
    Ok(item)
}

/// Eye Tracker 5 devices by standard descriptors only, without opening or claiming them.
#[pyfunction]
fn probe<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
    let devices = crate::exit::detached(py, tobii_usb::probe).py()?;
    PyList::new(py, devices.iter().map(|device| device_dict(py, device)).collect::<PyResult<Vec<_>>>()?)
}

#[pyfunction]
fn open_transport(py: Python<'_>, bus: u8, address: u8) -> PyResult<PyBulkTransport> {
    let transport = crate::exit::detached(py, || tobii_usb::open_transport(bus, address)).py()?;
    Ok(PyBulkTransport { inner: Arc::new(transport) })
}

fn alternate_arg(object: &Bound<'_, PyAny>) -> PyResult<Alternate> {
    let endpoints = object
        .getattr("endpoints")?
        .try_iter()?
        .map(|ep| {
            let ep = ep?;
            Ok(Endpoint {
                address: ep.getattr("address")?.extract()?,
                attributes: ep.getattr("attributes")?.extract()?,
                max_packet_size: ep.getattr("max_packet_size")?.extract()?,
                interval: ep.getattr("interval")?.extract()?,
            })
        })
        .collect::<PyResult<_>>()?;
    Ok(Alternate {
        interface: object.getattr("interface")?.extract()?,
        alternate: object.getattr("alternate")?.extract()?,
        interface_class: object.getattr("interface_class")?.extract()?,
        endpoints,
    })
}

/// `(incoming, outgoing)` endpoint tuples of the descriptor-confirmed streaming pipes.
#[pyfunction]
fn bulk_pair(alternatives: &Bound<'_, PyAny>) -> PyResult<(EndpointTuple, EndpointTuple)> {
    let alternatives: Vec<Alternate> =
        alternatives.try_iter()?.map(|alt| alternate_arg(&alt?)).collect::<PyResult<_>>()?;
    let (incoming, outgoing) = tobii_usb::bulk_pair(&alternatives).py()?;
    Ok((endpoint_tuple(&incoming), endpoint_tuple(&outgoing)))
}

/// Python ints outside the target type become a value the core rejects with its own message.
fn clamp<T: TryFrom<i128> + Default>(object: &Bound<'_, PyAny>) -> T {
    int(object).and_then(|n| T::try_from(n).ok()).unwrap_or_default()
}

fn setup_field<T: TryFrom<i128>>(object: Option<&Bound<'_, PyAny>>) -> PyResult<T> {
    let value = object.map_or(Some(0), int);
    value.and_then(|n| T::try_from(n).ok()).ok_or_else(|| PyValueError::new_err("Control setup fields out of range"))
}

#[pyclass(frozen, name = "BulkTransport", module = "tobii_linux.usb")]
pub struct PyBulkTransport {
    pub inner: Arc<BulkTransport>,
}

#[pymethods]
impl PyBulkTransport {
    #[getter]
    fn closed(&self) -> bool {
        self.inner.is_closed()
    }

    #[getter]
    fn incoming(&self) -> EndpointTuple {
        endpoint_tuple(&self.inner.incoming)
    }

    #[getter]
    fn outgoing(&self) -> EndpointTuple {
        endpoint_tuple(&self.inner.outgoing)
    }

    fn close(&self, py: Python<'_>) {
        crate::exit::detached(py, || self.inner.close());
    }

    #[pyo3(signature = (size, *, timeout_ms=None))]
    fn read<'py>(
        &self,
        py: Python<'py>,
        size: &Bound<'py, PyAny>,
        timeout_ms: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let size: usize = clamp(size);
        let timeout = timeout_ms.map_or(1000, clamp);
        let data = crate::exit::detached(py, || self.inner.read(size, timeout)).py()?;
        Ok(PyBytes::new(py, &data))
    }

    #[pyo3(signature = (data, *, timeout_ms=None))]
    fn write(&self, py: Python<'_>, data: &Bound<'_, PyAny>, timeout_ms: Option<&Bound<'_, PyAny>>) -> PyResult<usize> {
        let data = bytes_like(data)?;
        let timeout = timeout_ms.map_or(1000, clamp);
        crate::exit::detached(py, || self.inner.write(&data, timeout)).py()
    }

    #[pyo3(signature = (request, *, value=None, index=None, length, timeout_ms=None))]
    fn control_in<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'py, PyAny>,
        value: Option<&Bound<'py, PyAny>>,
        index: Option<&Bound<'py, PyAny>>,
        length: &Bound<'py, PyAny>,
        timeout_ms: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let length = int(length)
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n <= 4096)
            .ok_or_else(|| PyValueError::new_err("Control length must be between 0 and 4096"))?;
        let timeout = timeout_ms.map_or(1000, clamp);
        let (request, value, index) = (setup_field(Some(request))?, setup_field(value)?, setup_field(index)?);
        let data = crate::exit::detached(py, || self.inner.control_in(request, value, index, length, timeout)).py()?;
        Ok(PyBytes::new(py, &data))
    }

    #[pyo3(signature = (request, *, value=None, index=None, data=None, timeout_ms=None))]
    fn control_out(
        &self,
        py: Python<'_>,
        request: &Bound<'_, PyAny>,
        value: Option<&Bound<'_, PyAny>>,
        index: Option<&Bound<'_, PyAny>>,
        data: Option<&Bound<'_, PyAny>>,
        timeout_ms: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<usize> {
        let data = data.map_or(Ok(Vec::new()), bytes_like)?;
        if data.len() > 4096 {
            return Err(PyValueError::new_err("Control payload exceeds local 4096-byte limit"));
        }
        let timeout = timeout_ms.map_or(1000, clamp);
        let (request, value, index) = (setup_field(Some(request))?, setup_field(value)?, setup_field(index)?);
        crate::exit::detached(py, || self.inner.control_out(request, value, index, &data, timeout)).py()
    }

    fn __enter__(slf: Bound<'_, Self>) -> PyResult<Bound<'_, Self>> {
        if slf.get().inner.is_closed() {
            return Err(pyo3::exceptions::PyRuntimeError::new_err("USB transport is closed"));
        }
        Ok(slf)
    }

    #[pyo3(signature = (*_exc))]
    fn __exit__(&self, py: Python<'_>, _exc: &Bound<'_, PyTuple>) -> bool {
        self.close(py);
        false
    }
}

/// A transport implemented in Python (tests, traffic logging).
struct PyTransport(Py<PyAny>);

fn python_error(py: Python<'_>, error: PyErr) -> tobii_usb::Error {
    let usb = usb_error_type(py).filter(|class| error.is_instance(py, class));
    if usb.is_some() {
        let value = error.value(py);
        let fields = (|| -> PyResult<UsbError> {
            Ok(UsbError {
                operation: value.getattr("args")?.get_item(0).and_then(|o| o.extract()).unwrap_or_default(),
                code: value.getattr("code")?.extract()?,
                transferred: value.getattr("transferred")?.extract()?,
                partial_data: bytes_like(&value.getattr("partial_data")?)?,
            })
        })();
        if let Ok(usb) = fields {
            return tobii_usb::Error::Usb(usb);
        }
    }
    tobii_usb::Error::Foreign(Box::new(error))
}

impl PyTransport {
    fn call<T>(
        &self,
        name: &str,
        args: impl for<'py> FnOnce(Python<'py>) -> PyResult<(Bound<'py, PyTuple>, Bound<'py, PyDict>)>,
        convert: impl for<'py> FnOnce(Bound<'py, PyAny>) -> PyResult<T>,
    ) -> tobii_usb::Result<T> {
        crate::exit::reattach(|py| {
            let result = args(py)
                .and_then(|(args, kwargs)| self.0.bind(py).call_method(name, args, Some(&kwargs)))
                .and_then(convert);
            result.map_err(|error| python_error(py, error))
        })
        .unwrap_or_else(|| Err(tobii_usb::Error::state("The Python interpreter is shutting down")))
    }
}

fn timeout_kwargs(py: Python<'_>, timeout_ms: u32) -> PyResult<Bound<'_, PyDict>> {
    let kwargs = PyDict::new(py);
    kwargs.set_item("timeout_ms", timeout_ms)?;
    Ok(kwargs)
}

impl Transport for PyTransport {
    fn read(&self, size: usize, timeout_ms: u32) -> tobii_usb::Result<Vec<u8>> {
        self.call(
            "read",
            |py| Ok(((size,).into_pyobject(py)?, timeout_kwargs(py, timeout_ms)?)),
            |data| bytes_like(&data),
        )
    }

    fn write(&self, data: &[u8], timeout_ms: u32) -> tobii_usb::Result<usize> {
        self.call(
            "write",
            |py| Ok(((PyBytes::new(py, data),).into_pyobject(py)?, timeout_kwargs(py, timeout_ms)?)),
            |count| count.extract(),
        )
    }

    fn control_out(
        &self,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
        timeout_ms: u32,
    ) -> tobii_usb::Result<usize> {
        self.call(
            "control_out",
            |py| {
                let kwargs = PyDict::new(py);
                kwargs.set_item("value", value)?;
                if index != 0 {
                    kwargs.set_item("index", index)?;
                }
                if !data.is_empty() {
                    kwargs.set_item("data", PyBytes::new(py, data))?;
                }
                kwargs.set_item("timeout_ms", timeout_ms)?;
                Ok(((request,).into_pyobject(py)?, kwargs))
            },
            |count| Ok(count.extract::<Option<usize>>()?.unwrap_or(0)),
        )
    }

    fn control_in(
        &self,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
        timeout_ms: u32,
    ) -> tobii_usb::Result<Vec<u8>> {
        self.call(
            "control_in",
            |py| {
                let kwargs = PyDict::new(py);
                kwargs.set_item("value", value)?;
                kwargs.set_item("index", index)?;
                kwargs.set_item("length", length)?;
                kwargs.set_item("timeout_ms", timeout_ms)?;
                Ok(((request,).into_pyobject(py)?, kwargs))
            },
            |data| bytes_like(&data),
        )
    }
}

fn transport_arg(transport: &Bound<'_, PyAny>) -> Arc<dyn Transport> {
    if let Ok(native) = transport.cast::<PyBulkTransport>() {
        return native.get().inner.clone();
    }
    if let Ok(logging) = transport.cast::<PyLoggingTransport>() {
        return logging.get().inner.clone();
    }
    Arc::new(PyTransport(transport.clone().unbind()))
}

/// traffic_log.TrafficLog: one JSON line per transfer.
#[pyclass(frozen, name = "TrafficLog", module = "tobii_linux.traffic_log")]
pub struct PyTrafficLog(Arc<TrafficLog>);

#[pymethods]
impl PyTrafficLog {
    #[new]
    fn new(path: std::path::PathBuf) -> PyResult<Self> {
        Ok(Self(Arc::new(TrafficLog::create(path)?)))
    }

    /// Records `op` with JSON-serialisable values.
    #[pyo3(signature = (op, **values))]
    fn record(&self, py: Python<'_>, op: &str, values: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
        let mut entries = Vec::new();
        if let Some(values) = values {
            let dumps = py.import("json")?.getattr("dumps")?;
            for (key, value) in values.iter() {
                let text: String = dumps.call1((value,))?.extract()?;
                let value = serde_json::from_str(&text).map_err(|e| PyValueError::new_err(e.to_string()))?;
                entries.push((key.extract::<String>()?, value));
            }
        }
        self.0
            .record(op, entries.iter().map(|(key, value): &(String, serde_json::Value)| (key.as_str(), value.clone())));
        Ok(())
    }

    fn close(&self) {
        self.0.close();
    }
}

/// traffic_log.LoggingTransport: records every transfer of the wrapped transport.
#[pyclass(frozen, name = "LoggingTransport", module = "tobii_linux.traffic_log")]
pub struct PyLoggingTransport {
    inner: Arc<LoggingTransport<dyn Transport>>,
    #[pyo3(get)]
    transport: Py<PyAny>,
    #[pyo3(get)]
    log: Py<PyTrafficLog>,
}

#[pymethods]
impl PyLoggingTransport {
    #[new]
    fn new(transport: &Bound<'_, PyAny>, log: Bound<'_, PyTrafficLog>) -> Self {
        let inner = Arc::new(LoggingTransport::new(transport_arg(transport), Arc::clone(&log.get().0)));
        Self { inner, transport: transport.clone().unbind(), log: log.unbind() }
    }

    #[pyo3(signature = (size, *, timeout_ms=1000))]
    fn read<'py>(&self, py: Python<'py>, size: usize, timeout_ms: u32) -> PyResult<Bound<'py, PyBytes>> {
        let data = crate::exit::detached(py, || self.inner.read(size, timeout_ms)).py()?;
        Ok(PyBytes::new(py, &data))
    }

    #[pyo3(signature = (data, *, timeout_ms=1000))]
    fn write(&self, py: Python<'_>, data: &Bound<'_, PyAny>, timeout_ms: u32) -> PyResult<usize> {
        let data = bytes_like(data)?;
        crate::exit::detached(py, || self.inner.write(&data, timeout_ms)).py()
    }

    #[pyo3(signature = (request, *, value=0, index=0, data=None, timeout_ms=1000))]
    fn control_out(
        &self,
        py: Python<'_>,
        request: u8,
        value: u16,
        index: u16,
        data: Option<&Bound<'_, PyAny>>,
        timeout_ms: u32,
    ) -> PyResult<usize> {
        let data = data.map_or(Ok(Vec::new()), bytes_like)?;
        crate::exit::detached(py, || self.inner.control_out(request, value, index, &data, timeout_ms)).py()
    }

    #[pyo3(signature = (request, *, value=0, index=0, length, timeout_ms=1000))]
    fn control_in<'py>(
        &self,
        py: Python<'py>,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
        timeout_ms: u32,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let data =
            crate::exit::detached(py, || self.inner.control_in(request, value, index, length, timeout_ms)).py()?;
        Ok(PyBytes::new(py, &data))
    }

    fn __getattr__<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
        self.transport.bind(py).getattr(name)
    }
}

/// `(time, 'out' or 'in', Message or error text)` for the messages of `channel` in a log.
#[pyfunction]
#[pyo3(signature = (path, channel=0))]
fn replay_messages<'py>(py: Python<'py>, path: std::path::PathBuf, channel: u32) -> PyResult<Bound<'py, PyList>> {
    let replayed = crate::exit::detached(py, || traffic::replay_messages(path, channel)).py()?;
    let rows = PyList::empty(py);
    for row in replayed {
        let message = match &row.message {
            Ok(message) => crate::ttp::PyMessage::into_bound(py, message)?.into_any(),
            Err(text) => text.into_pyobject(py)?.into_any(),
        };
        rows.append((row.time, row.direction.name(), message))?;
    }
    Ok(rows)
}

/// One vendor channel; the IN endpoint of a native transport is read continuously while open.
#[pyclass(frozen, name = "USBChannel", module = "tobii_linux.usb_channel")]
pub struct PyUsbChannel {
    inner: Mutex<UsbChannel>,
    #[pyo3(get)]
    transport: Py<PyAny>,
}

impl PyUsbChannel {
    pub fn lock(&self) -> MutexGuard<'_, UsbChannel> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[pymethods]
impl PyUsbChannel {
    #[new]
    #[pyo3(signature = (transport, channel=None, *, control_timeout_ms=None))]
    fn new(
        transport: &Bound<'_, PyAny>,
        channel: Option<&Bound<'_, PyAny>>,
        control_timeout_ms: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let channel = channel.map_or(Some(0), int).and_then(|n| u32::try_from(n).ok()).unwrap_or(u32::MAX);
        let control_timeout = control_timeout_ms.map_or(1000, clamp);
        let inner = UsbChannel::new(transport_arg(transport), channel, control_timeout).py()?;
        Ok(Self { inner: Mutex::new(inner), transport: transport.clone().unbind() })
    }

    #[getter]
    fn channel(&self, py: Python<'_>) -> u8 {
        crate::exit::detached(py, || self.lock().channel())
    }

    #[getter]
    fn control_timeout_ms(&self, py: Python<'_>) -> u32 {
        crate::exit::detached(py, || self.lock().control_timeout_ms())
    }

    #[getter]
    fn opened(&self, py: Python<'_>) -> bool {
        crate::exit::detached(py, || self.lock().is_open())
    }

    #[getter]
    fn used(&self, py: Python<'_>) -> bool {
        crate::exit::detached(py, || self.lock().is_used())
    }

    #[getter]
    fn failed(&self, py: Python<'_>) -> bool {
        crate::exit::detached(py, || self.lock().is_failed())
    }

    #[getter]
    fn continuous(&self, py: Python<'_>) -> bool {
        crate::exit::detached(py, || self.lock().is_continuous())
    }

    fn __enter__(slf: Bound<'_, Self>) -> PyResult<Bound<'_, Self>> {
        let channel = slf.get();
        crate::exit::detached(slf.py(), || channel.lock().open()).py()?;
        Ok(slf)
    }

    #[pyo3(signature = (exc_type, _exc_value, _traceback))]
    fn __exit__(
        &self,
        py: Python<'_>,
        exc_type: &Bound<'_, PyAny>,
        _exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        match self.close(py) {
            Err(error) if exc_type.is_none() => Err(error),
            _ => Ok(false),
        }
    }

    fn close(&self, py: Python<'_>) -> PyResult<()> {
        crate::exit::detached(py, || self.lock().close()).py()
    }

    #[pyo3(signature = (payload, *, timeout_ms=None))]
    fn write(
        &self,
        py: Python<'_>,
        payload: &Bound<'_, PyAny>,
        timeout_ms: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<usize> {
        let payload = bytes_like(payload)?;
        let timeout = timeout_ms.map_or(3000, clamp);
        crate::exit::detached(py, || self.lock().write(&payload, timeout)).py()
    }

    #[pyo3(signature = (*, timeout_ms=None))]
    fn read<'py>(&self, py: Python<'py>, timeout_ms: Option<&Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyBytes>> {
        let timeout = timeout_ms.map_or(1000, clamp);
        let data = crate::exit::detached(py, || self.lock().read(timeout)).py()?;
        Ok(PyBytes::new(py, &data))
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBulkTransport>()?;
    m.add_class::<PyUsbChannel>()?;
    m.add_class::<PyTrafficLog>()?;
    m.add_class::<PyLoggingTransport>()?;
    m.add_function(wrap_pyfunction!(replay_messages, m)?)?;
    m.add_function(wrap_pyfunction!(probe, m)?)?;
    m.add_function(wrap_pyfunction!(open_transport, m)?)?;
    m.add_function(wrap_pyfunction!(bulk_pair, m)?)?;
    m.add("VID", tobii_usb::VID)?;
    m.add("PID", tobii_usb::PID)?;
    m.add("MAX_TRANSFER", tobii_usb::MAX_TRANSFER)?;
    m.add("LIBUSB_ERROR_TIMEOUT", tobii_usb::error::LIBUSB_ERROR_TIMEOUT)?;
    Ok(())
}
