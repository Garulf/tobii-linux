//! tobii_linux.driver, streams and supervisor: sessions, stream reading, clocks and the link.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use pyo3::exceptions::{PyOSError, PyRuntimeError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
use tobii_device::channel::{Channel, ChannelError};
use tobii_device::clock::ClockOffsets;
use tobii_device::session::{ImageFrames, TrackerSession};
use tobii_device::stop::{StopFlag, StopSignal};
use tobii_device::streams::{
    DeviceClockWatch, HostSuspendWatch, ItemPump, Pending, StreamItem, StreamReader, StreamValue, SuspendWatch,
};
use tobii_device::supervisor::{Backoff, LinkFailure, LinkHost, LinkState, TrackerLink};
use tobii_protocol::Message;
use tobii_protocol::commands::Stream;

use crate::convert::bytes_like;
use crate::errors::{IntoPyResult, NoTrackerError, protocol_message};
use crate::frames::image_tuple;
use crate::gaze::frame_tuple;
use crate::ttp::{PyMessage, message_arg, messages_list};
use crate::usb::PyUsbChannel;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// The channel a session runs on: a native USB channel, or any Python object with
/// `read(timeout_ms=...)` and `write(data)`.
enum SessionChannel {
    Native(Py<PyUsbChannel>),
    Python(Py<PyAny>),
}

fn python_channel_error(py: Python<'_>, error: PyErr) -> ChannelError {
    let fatal = error.is_instance_of::<PyOSError>(py) || error.is_instance_of::<PyRuntimeError>(py);
    ChannelError::new(fatal, error)
}

impl Channel for SessionChannel {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        match self {
            SessionChannel::Native(channel) => Channel::write(&mut *channel.get().lock(), data),
            SessionChannel::Python(channel) => crate::exit::reattach(|py| {
                channel
                    .call_method1(py, "write", (PyBytes::new(py, data),))
                    .map(drop)
                    .map_err(|e| python_channel_error(py, e))
            })
            .unwrap_or_else(|| Err(finalizing())),
        }
    }

    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        match self {
            SessionChannel::Native(channel) => Channel::read(&mut *channel.get().lock(), timeout_ms),
            SessionChannel::Python(channel) => crate::exit::reattach(|py| {
                let kwargs = PyDict::new(py);
                kwargs.set_item("timeout_ms", timeout_ms).map_err(|e| python_channel_error(py, e))?;
                let data = channel.bind(py).call_method("read", (), Some(&kwargs));
                data.and_then(|data| if data.is_none() { Ok(Vec::new()) } else { bytes_like(&data) })
                    .map_err(|e| python_channel_error(py, e))
            })
            .unwrap_or_else(|| Err(finalizing())),
        }
    }
}

fn finalizing() -> ChannelError {
    ChannelError::new(true, "The Python interpreter is shutting down")
}

type Session = TrackerSession<SessionChannel>;
type Triple = (f64, f64, f64);

/// The native half of driver.TrackerSession; the Python class keeps `pending_streams`.
#[pyclass(frozen, name = "Session", module = "tobii_linux._native.device")]
pub struct PySession(Mutex<Session>);

impl PySession {
    fn with<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut Session) -> R + Send) -> R {
        crate::exit::detached(py, || f(&mut lock(&self.0)))
    }
}

fn seconds(timeout_s: f64) -> Duration {
    Duration::try_from_secs_f64(timeout_s.max(0.0)).unwrap_or(Duration::MAX)
}

fn message_object(py: Python<'_>, message: &Message) -> PyResult<Py<PyAny>> {
    Ok(PyMessage::into_bound(py, message)?.into_any().unbind())
}

#[pymethods]
impl PySession {
    #[new]
    fn new(channel: &Bound<'_, PyAny>) -> Self {
        let channel = match channel.cast::<PyUsbChannel>() {
            Ok(native) => SessionChannel::Native(native.clone().unbind()),
            Err(_) => SessionChannel::Python(channel.clone().unbind()),
        };
        Self(Mutex::new(TrackerSession::new(channel)))
    }

    #[getter]
    fn dead(&self, py: Python<'_>) -> bool {
        self.with(py, |s| s.is_dead())
    }

    #[setter]
    fn set_dead(&self, py: Python<'_>, dead: bool) {
        self.with(py, |s| s.set_dead(dead));
    }

    #[getter]
    fn request_id(&self, py: Python<'_>) -> u32 {
        self.with(py, |s| s.request_id())
    }

    fn next_id(&self, py: Python<'_>) -> u32 {
        self.with(py, |s| s.next_id())
    }

    fn read<'py>(&self, py: Python<'py>, timeout_ms: u32) -> PyResult<Bound<'py, PyBytes>> {
        let data = self.with(py, |s| s.read(timeout_ms)).py()?;
        Ok(PyBytes::new(py, &data))
    }

    fn write(&self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let data = bytes_like(data)?;
        self.with(py, |s| s.write(&data)).py()
    }

    /// Bytes through the session's decoder (and observer).
    fn decode<'py>(&self, py: Python<'py>, data: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
        let data = bytes_like(data)?;
        let messages = self.with(py, |s| s.decode(&data)).py()?;
        messages_list(py, &messages)
    }

    fn decoder_pending<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let pending = self.with(py, |s| s.decoder_mut().pending().to_vec());
        PyBytes::new(py, &pending)
    }

    fn decoder_finish(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, |s| s.decoder_mut().finish()).py()
    }

    fn take_pending<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let pending = self.with(py, |s| s.take_pending_streams());
        messages_list(py, &pending)
    }

    /// Whether the channel is native, so loops can run without calling back into Python.
    #[getter]
    fn native_channel(&self, py: Python<'_>) -> bool {
        self.with(py, |s| matches!(s.channel(), SessionChannel::Native(_)))
    }

    /// The reply to `request_id` among decoded messages; earlier stream data becomes pending.
    fn route(&self, py: Python<'_>, messages: &Bound<'_, PyAny>, request_id: u32) -> PyResult<Option<Py<PyAny>>> {
        let messages = messages_arg(messages)?;
        let reply = self.with(py, |s| s.route(messages, request_id));
        reply.map(|reply| message_object(py, &reply)).transpose()
    }

    /// The reply of a call, or DeviceError for a missing reply (marking the session dead) or a failed status.
    fn finish_call(
        &self,
        py: Python<'_>,
        message: &Bound<'_, PyAny>,
        reply: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let message = message_arg(message)?;
        let reply = reply.map(message_arg).transpose()?;
        let reply = self.with(py, |s| s.finish_call(&message, reply)).py()?;
        message_object(py, &reply)
    }

    fn exchange(&self, py: Python<'_>, message: &Bound<'_, PyAny>, timeout_s: f64) -> PyResult<Option<Py<PyAny>>> {
        let message = message_arg(message)?;
        let reply = self.with(py, |s| s.request(&message, seconds(timeout_s))).py()?;
        reply.map(|reply| message_object(py, &reply)).transpose()
    }

    fn call(&self, py: Python<'_>, message: &Bound<'_, PyAny>, timeout_s: f64) -> PyResult<Py<PyAny>> {
        let message = message_arg(message)?;
        let reply = self.with(py, |s| s.call_message(&message, seconds(timeout_s))).py()?;
        message_object(py, &reply)
    }

    #[pyo3(signature = (realm, key, extra=None))]
    fn open_realm(&self, py: Python<'_>, realm: u32, key: &[u8], extra: Option<&[u8]>) -> PyResult<()> {
        let (key, extra) = (key.to_vec(), extra.unwrap_or_default().to_vec());
        self.with(py, |s| s.open_realm(realm, &key, &extra)).py()
    }

    /// `[(command, reply)]` of protocol upgrade and unlock.
    fn initialize<'py>(&self, py: Python<'py>) -> PyResult<Vec<(u32, Py<PyAny>)>> {
        let replies = self.with(py, |s| s.initialize().map(|()| s.startup_replies().to_vec())).py()?;
        replies.iter().map(|(command, reply)| Ok((*command, message_object(py, reply)?))).collect()
    }

    fn camera_calibration(&self, py: Python<'_>, camera: usize) -> PyResult<(Triple, Triple)> {
        let c = self.with(py, |s| s.camera_calibration(camera)).py()?;
        let [x, y, z] = c.translation;
        let [a, b, d] = c.angles;
        Ok(((x, y, z), (a, b, d)))
    }

    fn subscribe(&self, py: Python<'_>, stream: u32) -> PyResult<Py<PyAny>> {
        let stream = stream_arg(stream)?;
        let reply = self.with(py, |s| s.subscribe(stream)).py()?;
        message_object(py, &reply)
    }

    /// Called with every decoded message; None removes the observer.
    fn set_observer(&self, py: Python<'_>, observer: Option<Py<PyAny>>) {
        let observer = observer.map(|callback| {
            Box::new(move |message: &Message| {
                crate::exit::reattach(|py| {
                    let called = message_object(py, message).and_then(|m| callback.call1(py, (m,)));
                    if let Err(error) = called {
                        error.write_unraisable(py, None);
                    }
                });
            }) as Box<dyn FnMut(&Message) + Send>
        });
        self.with(py, |s| s.set_observer(observer));
    }
}

fn stream_arg(stream: u32) -> PyResult<Stream> {
    Stream::from_index(stream).ok_or_else(|| protocol_message("Unknown stream index"))
}

fn messages_arg(messages: &Bound<'_, PyAny>) -> PyResult<Vec<Message>> {
    messages.try_iter()?.map(|m| message_arg(&m?)).collect()
}

/// State of TrackerSession.frames(): image frames until the device stays silent.
#[pyclass(frozen, name = "ImageFrames", module = "tobii_linux._native.device")]
pub struct PyImageFrames {
    frames: Mutex<ImageFrames>,
    session: Py<PySession>,
}

#[pymethods]
impl PyImageFrames {
    #[new]
    fn new(session: Py<PySession>, pending: &Bound<'_, PyAny>, timeout_s: f64) -> PyResult<Self> {
        Ok(Self { frames: Mutex::new(ImageFrames::new(messages_arg(pending)?, seconds(timeout_s))), session })
    }

    fn pending_frame<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyTuple>>> {
        let frame = crate::exit::detached(py, || lock(&self.frames).pending_frame()).py()?;
        frame.map(|frame| image_tuple(py, &frame)).transpose()
    }

    fn remaining_ms(&self, py: Python<'_>) -> Option<u32> {
        crate::exit::detached(py, || lock(&self.frames).remaining_ms())
    }

    fn received(&self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let session = self.session.get();
        let data = bytes_like(data)?;
        crate::exit::detached(py, || lock(&self.frames).received(&mut lock(&session.0), &data)).py()
    }

    /// `(timestamp, pixel_format, width, height, pixels)` of the next frame, None at the end.
    fn next_frame<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyTuple>>> {
        let session = self.session.get();
        let frame = crate::exit::detached(py, || lock(&self.frames).next(&mut lock(&session.0))).py()?;
        frame.map(|frame| image_tuple(py, &frame)).transpose()
    }
}

fn item_tuple<'py>(py: Python<'py>, item: StreamItem) -> PyResult<Bound<'py, PyTuple>> {
    let message = message_object(py, &item.message)?;
    let (kind, value): (&str, Bound<'py, PyAny>) = match &item.value {
        StreamValue::Image(frame) => ("image", image_tuple(py, frame)?.into_any()),
        StreamValue::Gaze(frame) => ("gaze", frame_tuple(py, frame)?.into_any()),
        StreamValue::Presence(p) => ("presence", (p.timestamp_us, p.value).into_pyobject(py)?.into_any()),
        StreamValue::Raw => ("raw", message.bind(py).clone()),
    };
    (item.stream.index(), kind, value, message).into_pyobject(py)
}

/// The stream set of streams.StreamReader, its subscriptions and counters.
#[pyclass(frozen, name = "StreamReader", module = "tobii_linux._native.device")]
pub struct PyStreamReader(Mutex<StreamReader>);

fn counts(map: &std::collections::BTreeMap<Stream, u64>) -> Vec<(u32, u64)> {
    map.iter().map(|(stream, count)| (stream.index(), *count)).collect()
}

#[pymethods]
impl PyStreamReader {
    #[new]
    fn new(streams: Vec<u32>, optional: Vec<u32>) -> PyResult<Self> {
        let streams: Vec<Stream> = streams.into_iter().map(stream_arg).collect::<PyResult<_>>()?;
        let optional: Vec<Stream> = optional.into_iter().map(stream_arg).collect::<PyResult<_>>()?;
        Ok(Self(Mutex::new(StreamReader::new(&streams, &optional))))
    }

    fn subscribe(&self, py: Python<'_>, session: &Bound<'_, PySession>) -> PyResult<()> {
        let session = session.get();
        crate::exit::detached(py, || lock(&self.0).subscribe(&mut lock(&session.0))).py()
    }

    /// Names of the streams unsubscribed.
    fn unsubscribe(&self, py: Python<'_>, session: &Bound<'_, PySession>) -> PyResult<Vec<&'static str>> {
        let session = session.get();
        let streams = crate::exit::detached(py, || lock(&self.0).unsubscribe(&mut lock(&session.0))).py()?;
        Ok(streams.iter().map(|s| s.name()).collect())
    }

    /// `(stream, kind, value, message)` for one message, None when not ours, dropped or malformed.
    fn decode<'py>(&self, py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyTuple>>> {
        let message = message_arg(message)?;
        let item = crate::exit::detached(py, || lock(&self.0).decode(message));
        item.map(|item| item_tuple(py, item)).transpose()
    }

    #[getter]
    fn subscribed(&self, py: Python<'_>) -> Vec<u32> {
        crate::exit::detached(py, || lock(&self.0).subscribed.iter().map(|s| s.index()).collect())
    }

    #[setter]
    fn set_subscribed(&self, py: Python<'_>, streams: Vec<u32>) -> PyResult<()> {
        let streams: Vec<Stream> = streams.into_iter().map(stream_arg).collect::<PyResult<_>>()?;
        crate::exit::detached(py, || lock(&self.0).subscribed = streams);
        Ok(())
    }

    #[getter]
    fn refused(&self, py: Python<'_>) -> Vec<(u32, String)> {
        crate::exit::detached(py, || lock(&self.0).refused.iter().map(|(s, e)| (s.index(), e.clone())).collect())
    }

    #[getter]
    fn received(&self, py: Python<'_>) -> Vec<(u32, u64)> {
        crate::exit::detached(py, || counts(&lock(&self.0).stats.received))
    }

    #[getter]
    fn dropped(&self, py: Python<'_>) -> Vec<(u32, u64)> {
        crate::exit::detached(py, || counts(&lock(&self.0).stats.dropped))
    }

    #[getter]
    fn decode_errors(&self, py: Python<'_>) -> Vec<(u32, u64)> {
        crate::exit::detached(py, || counts(&lock(&self.0).stats.decode_errors))
    }

    #[getter]
    fn unexpected(&self, py: Python<'_>) -> u64 {
        crate::exit::detached(py, || lock(&self.0).stats.unexpected)
    }

    #[getter]
    fn last_error(&self, py: Python<'_>) -> Option<String> {
        crate::exit::detached(py, || lock(&self.0).stats.last_error.clone())
    }
}

/// `is_set()` of a Python event (threading.Event or anything with that method).
struct PyStop(Py<PyAny>);

/// A native StopEvent directly, any other event through Python.
enum Stop {
    Native(StopFlag),
    Python(PyStop),
}

impl Stop {
    fn new(event: Py<PyAny>, py: Python<'_>) -> Self {
        match event.bind(py).cast::<PyStopEvent>() {
            Ok(native) => Stop::Native(native.get().0.clone()),
            Err(_) => Stop::Python(PyStop(event)),
        }
    }
}

impl StopSignal for Stop {
    fn is_set(&self) -> bool {
        match self {
            Stop::Native(flag) => flag.is_set(),
            Stop::Python(event) => event.is_set(),
        }
    }

    fn wait(&self, timeout: Duration) -> bool {
        match self {
            Stop::Native(flag) => flag.wait(timeout),
            Stop::Python(event) => event.wait(timeout),
        }
    }
}

impl StopSignal for PyStop {
    fn is_set(&self) -> bool {
        crate::exit::reattach(|py| self.0.call_method0(py, "is_set").and_then(|v| v.is_truthy(py)).unwrap_or(true))
            .unwrap_or(true)
    }

    fn wait(&self, timeout: Duration) -> bool {
        crate::exit::reattach(|py| {
            self.0.call_method1(py, "wait", (timeout.as_secs_f64(),)).and_then(|v| v.is_truthy(py)).unwrap_or(true)
        })
        .unwrap_or(true)
    }
}

/// `resumed()` of a Python suspend watch; an error stops the stream like a resume would.
struct PySuspend {
    watch: Py<PyAny>,
    error: Arc<Mutex<Option<PyErr>>>,
}

impl SuspendWatch for PySuspend {
    fn resumed(&mut self) -> bool {
        crate::exit::reattach(|py| match self.watch.call_method0(py, "resumed").and_then(|v| v.is_truthy(py)) {
            Ok(resumed) => resumed,
            Err(error) => {
                *lock(&self.error) = Some(error);
                true
            }
        })
        .unwrap_or(false)
    }
}

/// A native SuspendWatch directly, any other watch through Python.
enum Suspend {
    Native(Py<PySuspendWatch>),
    Python(PySuspend),
}

impl SuspendWatch for Suspend {
    fn resumed(&mut self) -> bool {
        match self {
            Suspend::Native(watch) => lock(&watch.get().watch).resumed(),
            Suspend::Python(watch) => watch.resumed(),
        }
    }
}

/// The resumable loop of StreamReader.items().
#[pyclass(frozen, name = "ItemPump", module = "tobii_linux._native.device")]
pub struct PyItemPump(Mutex<ItemPump>);

#[pymethods]
impl PyItemPump {
    #[new]
    fn new(pending: &Bound<'_, PyAny>, silence_timeout_s: f64) -> PyResult<Self> {
        Ok(Self(Mutex::new(ItemPump::new(messages_arg(pending)?, seconds(silence_timeout_s)))))
    }

    /// The next item read on this thread; None once `stop_event` is set.
    #[pyo3(signature = (reader, session, stop_event, suspend_watch=None))]
    fn next<'py>(
        &self,
        py: Python<'py>,
        reader: &Bound<'py, PyStreamReader>,
        session: &Bound<'py, PySession>,
        stop_event: Py<PyAny>,
        suspend_watch: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Option<Bound<'py, PyTuple>>> {
        let (reader, session) = (reader.get(), session.get());
        let stop = Stop::new(stop_event, py);
        let error = Arc::new(Mutex::new(None));
        let mut suspend = suspend_watch.map(|watch| match watch.cast_into::<PySuspendWatch>() {
            Ok(native) => Suspend::Native(native.unbind()),
            Err(error_value) => {
                Suspend::Python(PySuspend { watch: error_value.into_inner().unbind(), error: Arc::clone(&error) })
            }
        });
        let item = crate::exit::detached(py, || {
            let watch = suspend.as_mut().map(|w| w as &mut dyn SuspendWatch);
            lock(&self.0).next(&mut lock(&reader.0), &mut lock(&session.0), watch, &stop)
        });
        if let Some(error) = lock(&error).take() {
            return Err(error);
        }
        if let Some(Suspend::Native(watch)) = &suspend
            && let Some(error) = lock(&watch.get().error).take()
        {
            return Err(error);
        }
        item.py()?.map(|item| item_tuple(py, item)).transpose()
    }

    /// The next item among received messages: an item tuple, "stopped" or None.
    fn pending_item<'py>(
        &self,
        py: Python<'py>,
        reader: &Bound<'py, PyStreamReader>,
        stop_event: Py<PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let reader = reader.get();
        let stop = Stop::new(stop_event, py);
        let pending = match &stop {
            Stop::Native(_) => crate::exit::detached(py, || lock(&self.0).pending_item(&mut lock(&reader.0), &stop)),
            Stop::Python(_) => lock(&self.0).pending_item(&mut lock(&reader.0), &stop),
        };
        Ok(match pending {
            Pending::Item(item) => item_tuple(py, *item)?.into_any(),
            Pending::Stopped => "stopped".into_pyobject(py)?.into_any(),
            Pending::Empty => py.None().into_bound(py),
        })
    }

    /// Bytes of one read and whether the host was suspended meanwhile.
    fn received(
        &self,
        py: Python<'_>,
        session: &Bound<'_, PySession>,
        data: &Bound<'_, PyAny>,
        resumed: bool,
    ) -> PyResult<()> {
        let session = session.get();
        let data = bytes_like(data)?;
        crate::exit::detached(py, || lock(&self.0).received(&mut lock(&session.0), &data, resumed)).py()
    }
}

/// The state of driver.ClockMapper; the Python class reads its clock and passes the values.
#[pyclass(frozen, name = "ClockOffsets", module = "tobii_linux._native.device")]
pub struct PyClockOffsets(Mutex<ClockOffsets>);

#[pymethods]
impl PyClockOffsets {
    #[new]
    #[pyo3(signature = (window=64, generation=None))]
    fn new(window: usize, generation: Option<i64>) -> Self {
        Self(Mutex::new(ClockOffsets::new(window, generation)))
    }

    fn observe(&self, py: Python<'_>, generation: Option<i64>, now_us: i64, device_us: i64) {
        crate::exit::detached(py, || lock(&self.0).observe(generation, now_us, device_us));
    }

    fn map(&self, py: Python<'_>, now_us: i64, generation: Option<i64>, device_us: i64) -> i64 {
        crate::exit::detached(py, || lock(&self.0).map(now_us, generation, device_us))
    }

    #[getter]
    fn offset(&self, py: Python<'_>) -> Option<i64> {
        crate::exit::detached(py, || lock(&self.0).offset())
    }

    #[getter]
    fn offsets(&self, py: Python<'_>) -> Vec<i64> {
        crate::exit::detached(py, || lock(&self.0).offsets().iter().copied().collect())
    }
}

/// A threading.Event that native loops can check without calling into Python.
#[pyclass(frozen, name = "StopEvent", module = "tobii_linux.streams")]
pub struct PyStopEvent(StopFlag);

#[pymethods]
impl PyStopEvent {
    #[new]
    fn new() -> Self {
        Self(StopFlag::new())
    }

    fn set(&self) {
        self.0.set();
    }

    fn clear(&self) {
        self.0.clear();
    }

    fn is_set(&self) -> bool {
        self.0.is_set()
    }

    #[pyo3(signature = (timeout=None))]
    fn wait(&self, py: Python<'_>, timeout: Option<f64>) -> bool {
        let timeout = timeout.map_or(Duration::MAX, seconds);
        crate::exit::detached(py, || self.0.wait(timeout))
    }
}

#[pyclass(frozen, name = "DeviceClockWatch", module = "tobii_linux.streams")]
pub struct PyDeviceClockWatch(Mutex<DeviceClockWatch>);

#[pymethods]
impl PyDeviceClockWatch {
    #[new]
    #[pyo3(signature = (rewind_us=tobii_device::streams::DEVICE_CLOCK_REWIND_US))]
    fn new(rewind_us: i64) -> Self {
        Self(Mutex::new(DeviceClockWatch { rewind_us, latest: None }))
    }

    fn observe(&self, py: Python<'_>, device_us: i64) -> PyResult<()> {
        crate::exit::detached(py, || lock(&self.0).observe(device_us)).py()
    }

    #[getter]
    fn latest(&self, py: Python<'_>) -> Option<i64> {
        crate::exit::detached(py, || lock(&self.0).latest)
    }

    #[getter]
    fn rewind_us(&self, py: Python<'_>) -> i64 {
        crate::exit::detached(py, || lock(&self.0).rewind_us)
    }
}

type SuspendedTime = Box<dyn FnMut() -> f64 + Send>;

#[pyclass(frozen, name = "SuspendWatch", module = "tobii_linux.streams")]
pub struct PySuspendWatch {
    watch: Mutex<HostSuspendWatch<SuspendedTime>>,
    error: Arc<Mutex<Option<PyErr>>>,
}

#[pymethods]
impl PySuspendWatch {
    #[new]
    #[pyo3(signature = (suspended_time=None, threshold_s=tobii_device::streams::SUSPEND_GAP_S))]
    fn new(py: Python<'_>, suspended_time: Option<Py<PyAny>>, threshold_s: f64) -> PyResult<Self> {
        let error: Arc<Mutex<Option<PyErr>>> = Arc::default();
        let stash = Arc::clone(&error);
        let source: SuspendedTime = match suspended_time {
            None => Box::new(tobii_device::streams::suspended_time_s),
            Some(callable) => Box::new(move || {
                crate::exit::reattach(|py| {
                    callable.call0(py).and_then(|v| v.extract::<f64>(py)).unwrap_or_else(|e| {
                        lock(&stash).get_or_insert(e);
                        0.0
                    })
                })
                .unwrap_or_default()
            }),
        };
        let watch = crate::exit::detached(py, || HostSuspendWatch::new(source, threshold_s));
        if let Some(e) = lock(&error).take() {
            return Err(e);
        }
        Ok(Self { watch: Mutex::new(watch), error })
    }

    fn resumed(&self, py: Python<'_>) -> PyResult<bool> {
        let resumed = crate::exit::detached(py, || lock(&self.watch).resumed());
        match lock(&self.error).take() {
            Some(error) => Err(error),
            None => Ok(resumed),
        }
    }
}

#[pyfunction]
fn suspended_time_s() -> f64 {
    tobii_device::streams::suspended_time_s()
}

#[pyclass(frozen, name = "Backoff", module = "tobii_linux.supervisor")]
pub struct PyBackoff(Mutex<Backoff>);

#[pymethods]
impl PyBackoff {
    #[new]
    #[pyo3(signature = (first_s=0.5, max_s=2.0, healthy_s=10.0))]
    fn new(first_s: f64, max_s: f64, healthy_s: f64) -> Self {
        Self(Mutex::new(Backoff::new(first_s, max_s, healthy_s)))
    }

    fn delay(&self, session_s: f64) -> f64 {
        lock(&self.0).delay(session_s)
    }

    #[getter]
    fn first_s(&self) -> f64 {
        lock(&self.0).first_s
    }

    #[setter]
    fn set_first_s(&self, value: f64) {
        lock(&self.0).first_s = value;
    }

    #[getter]
    fn max_s(&self) -> f64 {
        lock(&self.0).max_s
    }

    #[setter]
    fn set_max_s(&self, value: f64) {
        lock(&self.0).max_s = value;
    }

    #[getter]
    fn healthy_s(&self) -> f64 {
        lock(&self.0).healthy_s
    }

    #[setter]
    fn set_healthy_s(&self, value: f64) {
        lock(&self.0).healthy_s = value;
    }

    #[getter]
    fn next_s(&self) -> f64 {
        lock(&self.0).next_s
    }

    #[setter]
    fn set_next_s(&self, value: f64) {
        lock(&self.0).next_s = value;
    }

    fn __repr__(&self) -> String {
        let b = lock(&self.0);
        format!("Backoff(first_s={}, max_s={}, healthy_s={})", b.first_s, b.max_s, b.healthy_s)
    }
}

/// A Python exception from a session attempt.
struct PyFailure(PyErr);

impl std::fmt::Display for PyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Python::attach(|py| match self.0.value(py).str() {
            Ok(text) => write!(f, "{text}"),
            Err(_) => write!(f, "{}", self.0),
        })
    }
}

impl LinkFailure for PyFailure {
    fn is_no_tracker(&self) -> bool {
        Python::attach(|py| self.0.is_instance_of::<NoTrackerError>(py))
    }

    fn kind(&self) -> String {
        Python::attach(|py| self.0.get_type(py).name().map(|n| n.to_string()).unwrap_or_else(|_| "Exception".into()))
    }
}

/// open_session (context manager factory), run_session and on_state of supervisor.TrackerLink.
struct PyLinkHost<'py> {
    py: Python<'py>,
    open_session: Bound<'py, PyAny>,
    run_session: Bound<'py, PyAny>,
    on_state: Bound<'py, PyAny>,
    clock: Bound<'py, PyAny>,
    stopping: Bound<'py, PyAny>,
}

impl PyLinkHost<'_> {
    fn session(&self, opened: &mut dyn FnMut()) -> PyResult<()> {
        let manager = self.open_session.call0()?;
        let exit = manager.getattr("__exit__")?;
        let session = manager.getattr("__enter__")?.call0()?;
        opened();
        match self.run_session.call1((session, &self.stopping)) {
            Ok(_) => {
                exit.call1((self.py.None(), self.py.None(), self.py.None()))?;
                Ok(())
            }
            Err(error) => {
                let value = error.value(self.py);
                let suppressed = exit.call1((error.get_type(self.py), value, error.traceback(self.py)))?.is_truthy()?;
                if suppressed { Ok(()) } else { Err(error) }
            }
        }
    }
}

impl LinkHost for PyLinkHost<'_> {
    type Error = PyFailure;

    fn on_state(&mut self, state: LinkState, detail: &str) -> Result<(), PyFailure> {
        self.on_state.call1((state.name(), detail)).map(drop).map_err(PyFailure)
    }

    fn attempt(&mut self, _stop: &dyn StopSignal, opened: &mut dyn FnMut()) -> Result<(), PyFailure> {
        self.session(opened).map_err(PyFailure)
    }

    fn now_s(&mut self) -> f64 {
        self.clock.call0().and_then(|v| v.extract()).unwrap_or(0.0)
    }
}

/// The loop of supervisor.TrackerLink; counters stay readable while it runs.
#[pyclass(frozen, name = "TrackerLink", module = "tobii_linux._native.device")]
pub struct PyTrackerLink(Arc<tobii_device::supervisor::LinkCounters>);

#[pymethods]
impl PyTrackerLink {
    #[new]
    fn new() -> Self {
        Self(Arc::default())
    }

    #[getter]
    fn sessions(&self) -> u64 {
        self.0.sessions()
    }

    #[getter]
    fn failures(&self) -> u64 {
        self.0.failures()
    }

    /// Runs `link` (a supervisor.TrackerLink) until its `stopping` event is set.
    fn run(&self, link: &Bound<'_, PyAny>) -> PyResult<()> {
        let stopping = link.getattr("stopping")?;
        let backoff = link.getattr("backoff")?;
        let stop = PyStop(stopping.clone().unbind());
        let mut host = PyLinkHost {
            py: link.py(),
            open_session: link.getattr("open_session")?,
            run_session: link.getattr("run_session")?,
            on_state: link.getattr("on_state")?,
            clock: link.getattr("clock")?,
            stopping,
        };
        let mut core = TrackerLink { backoff: Backoff::default(), counters: Arc::clone(&self.0) };
        let native = backoff.cast::<PyBackoff>().ok().map(|b| b.clone().unbind());
        core.run_with(&mut host, &stop, |_, session_s| match &native {
            Some(native) => native.get().delay(session_s),
            None => backoff.call_method1("delay", (session_s,)).and_then(|d| d.extract()).unwrap_or(0.0),
        });
        Ok(())
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySession>()?;
    m.add_class::<PyImageFrames>()?;
    m.add_class::<PyStreamReader>()?;
    m.add_class::<PyItemPump>()?;
    m.add_class::<PyClockOffsets>()?;
    m.add_class::<PyStopEvent>()?;
    m.add_class::<PyDeviceClockWatch>()?;
    m.add_class::<PySuspendWatch>()?;
    m.add_class::<PyBackoff>()?;
    m.add_class::<PyTrackerLink>()?;
    m.add_function(wrap_pyfunction!(suspended_time_s, m)?)?;
    m.add("UNLOCK_REALM_KEY", PyBytes::new(m.py(), tobii_device::session::UNLOCK_REALM_KEY))?;
    m.add("HEAD_POSE_REALM", tobii_device::session::HEAD_POSE_REALM)?;
    m.add("HEAD_POSE_REALM_KEY", PyBytes::new(m.py(), &tobii_device::session::HEAD_POSE_REALM_KEY))?;
    m.add("HEAD_POSE_REALM_EXTRA", PyBytes::new(m.py(), &tobii_device::session::HEAD_POSE_REALM_EXTRA))?;
    Ok(())
}
