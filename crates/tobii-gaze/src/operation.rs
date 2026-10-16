//! Device procedures as resumable state machines.
//!
//! Backup, restore, display setup, profile switching and the calibration sequence are
//! written once, as `async` code against [`Io`], and run as an [`Operation`]: each call to
//! [`Operation::resume`] advances the procedure to its next [`Action`] (send a request, run
//! a UI callback, read the device, log) and waits for the caller to report its outcome as
//! an [`Input`]. Nothing here performs I/O, sleeps or spawns threads.
//!
//! The caller owns the session and the loop. [`run`] drives an operation on a
//! [`TrackerSession`]; the Python wrappers drive the same operations on Python sessions, so
//! no Rust loop waits on Python code. The request sequence is therefore the same whoever
//! drives it.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use thiserror::Error;

use tobii_device::{Channel, DeviceError, TrackerSession};
use tobii_protocol::Message;
use tobii_protocol::commands::{NOTIFICATION_KIND, STREAM_DATA_KIND};
use tobii_protocol::fields::Value;

use crate::calibration::{READ_SLICE_MS, UiEvent};
use crate::device::StimulusPoint;
use crate::error::GazeError;

/// An error raised by the driver (a failed call, a UI callback, a read) that an operation
/// either handles or hands back unchanged.
pub trait External: Send + 'static {
    /// The error as text, for logs and for recognizing device statuses.
    fn message(&self) -> String;
    /// Whether the error is an interruption (Python's `BaseException` outside `Exception`):
    /// cleanup still runs, but the error is raised as is rather than wrapped.
    fn is_interrupt(&self) -> bool {
        false
    }
}

/// A request to send: the message with request id 0; the driver assigns the id.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub message: Message,
    /// Reply deadline in seconds; `None` leaves the session's default.
    pub timeout_s: Option<f64>,
    /// A cleanup call whose failure the operation only logs; an interruption raised while
    /// sending it propagates at once.
    pub tolerant: bool,
}

impl Call {
    pub fn request(&self, request_id: u32) -> Message {
        Message { request_id, ..self.message.clone() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    StimulusPoints(Vec<StimulusPoint>),
}

/// A progress record: a kind and named values in order.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRecord {
    pub kind: &'static str,
    pub values: Vec<(&'static str, LogValue)>,
}

/// What the driver must do next.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send the request and report [`Input::Replied`] with the reply values, or
    /// [`Input::Failed`].
    Call(Call),
    /// Run the UI callback while still reading the device, then report [`Input::Done`] or
    /// [`Input::Failed`].
    Ui(UiEvent),
    /// Ask the UI whether the user cancelled: [`Input::Done`] or [`Input::Failed`].
    Check,
    /// Read the device once (about 100 ms), keeping stream data in the session's pending
    /// streams and logging notifications: [`Input::Done`] or [`Input::Failed`].
    Read,
    /// Report the session's pending stream messages ([`Input::Streams`]), clearing them
    /// when `take` is set.
    PendingStreams { take: bool },
    /// Clear the session's pending stream messages: [`Input::Done`].
    ClearStreams,
    /// Record progress: [`Input::Done`].
    Log(LogRecord),
}

#[derive(Debug)]
pub enum Input<E> {
    /// The first resume.
    Start,
    Replied(Vec<Value>),
    Failed(E),
    Done,
    Streams(Vec<Message>),
}

/// How an operation failed.
#[derive(Debug)]
pub enum OperationError<E> {
    Gaze(GazeError),
    /// A driver error, to be raised unchanged.
    External(E),
    /// `calibration.CalibrationFailed`: a calibration step failed after the device was told
    /// to stop.
    Failed {
        step: String,
        cause: Box<OperationError<E>>,
    },
}

impl<E: External> std::fmt::Display for OperationError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationError::Gaze(error) => error.fmt(f),
            OperationError::External(error) => f.write_str(&error.message()),
            OperationError::Failed { step, cause } => write!(f, "{step} failed: {cause}"),
        }
    }
}

impl<E: External + std::fmt::Debug> std::error::Error for OperationError<E> {}

impl<E> From<GazeError> for OperationError<E> {
    fn from(error: GazeError) -> Self {
        OperationError::Gaze(error)
    }
}

impl<E: External> OperationError<E> {
    pub fn calibration(message: impl Into<String>) -> Self {
        OperationError::Gaze(GazeError::Calibration(message.into()))
    }
}

impl<E> From<tobii_protocol::ProtocolError> for OperationError<E> {
    fn from(error: tobii_protocol::ProtocolError) -> Self {
        OperationError::Gaze(error.into())
    }
}

pub type OperationResult<T, E> = Result<T, OperationError<E>>;

pub enum Step<T, E> {
    Action(Action),
    Finished(OperationResult<T, E>),
}

impl<T, E> std::fmt::Debug for Step<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Step::Action(action) => f.debug_tuple("Action").field(action).finish(),
            Step::Finished(result) => f.debug_tuple("Finished").field(&result.is_ok()).finish(),
        }
    }
}

struct Slot<E> {
    action: Option<Action>,
    input: Option<Input<E>>,
    now: f64,
}

/// The procedure's side of the exchange with the driver.
pub struct Io<E>(Arc<Mutex<Slot<E>>>);

impl<E> Clone for Io<E> {
    fn clone(&self) -> Self {
        Io(Arc::clone(&self.0))
    }
}

impl<E> std::fmt::Debug for Io<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Io").finish_non_exhaustive()
    }
}

struct Exchange<'a, E> {
    io: &'a Io<E>,
    action: Option<Action>,
}

impl<E> Future for Exchange<'_, E> {
    type Output = Input<E>;

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Input<E>> {
        let action = self.action.take();
        let mut slot = self.io.lock();
        if let Some(action) = action {
            slot.action = Some(action);
            slot.input = None;
            return Poll::Pending;
        }
        match slot.input.take() {
            Some(input) => Poll::Ready(input),
            None => Poll::Pending,
        }
    }
}

fn unexpected<E>(input: &Input<E>, action: &str) -> ! {
    let kind = match input {
        Input::Start => "start",
        Input::Replied(_) => "reply",
        Input::Failed(_) => "failure",
        Input::Done => "done",
        Input::Streams(_) => "streams",
    };
    panic!("driver answered {action} with {kind}")
}

impl<E> Io<E> {
    fn lock(&self) -> MutexGuard<'_, Slot<E>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn exchange(&self, action: Action) -> Input<E> {
        Exchange { io: self, action: Some(action) }.await
    }

    /// Monotonic seconds when the driver reported the latest input.
    pub fn now(&self) -> f64 {
        self.lock().now
    }

    pub async fn call(&self, message: Message, timeout_s: Option<f64>) -> Result<Vec<Value>, E> {
        self.send(Call { message, timeout_s, tolerant: false }).await
    }

    pub async fn send(&self, call: Call) -> Result<Vec<Value>, E> {
        match self.exchange(Action::Call(call)).await {
            Input::Replied(values) => Ok(values),
            Input::Failed(error) => Err(error),
            other => unexpected(&other, "a call"),
        }
    }

    async fn completion(&self, action: Action, name: &str) -> Result<(), E> {
        match self.exchange(action).await {
            Input::Done => Ok(()),
            Input::Failed(error) => Err(error),
            other => unexpected(&other, name),
        }
    }

    pub async fn ui(&self, event: UiEvent) -> Result<(), E> {
        self.completion(Action::Ui(event), "a UI event").await
    }

    pub async fn check(&self) -> Result<(), E> {
        self.completion(Action::Check, "a check").await
    }

    pub async fn read(&self) -> Result<(), E> {
        self.completion(Action::Read, "a read").await
    }

    pub async fn pending_streams(&self, take: bool) -> Vec<Message> {
        match self.exchange(Action::PendingStreams { take }).await {
            Input::Streams(messages) => messages,
            other => unexpected(&other, "a stream request"),
        }
    }

    pub async fn clear_streams(&self) {
        if let Input::Failed(_) = self.exchange(Action::ClearStreams).await {
            panic!("driver failed to clear streams")
        }
    }

    pub async fn log(&self, kind: &'static str, values: Vec<(&'static str, LogValue)>) {
        if let Input::Failed(_) = self.exchange(Action::Log(LogRecord { kind, values })).await {
            panic!("driver failed to log")
        }
    }
}

type Procedure<T, E> = Pin<Box<dyn Future<Output = OperationResult<T, E>> + Send>>;

/// A device procedure in progress.
pub struct Operation<T, E> {
    io: Io<E>,
    procedure: Option<Procedure<T, E>>,
}

impl<T, E> std::fmt::Debug for Operation<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operation").field("finished", &self.procedure.is_none()).finish()
    }
}

impl<T, E: Send + 'static> Operation<T, E> {
    pub fn new<F, P>(procedure: F) -> Self
    where
        F: FnOnce(Io<E>) -> P,
        P: Future<Output = OperationResult<T, E>> + Send + 'static,
    {
        let io = Io(Arc::new(Mutex::new(Slot { action: None, input: None, now: 0.0 })));
        let procedure = Box::pin(procedure(io.clone()));
        Self { io, procedure: Some(procedure) }
    }

    /// Reports the outcome of the previous action (or [`Input::Start`]) at monotonic time
    /// `now` in seconds, and returns the next action or the result.
    ///
    /// # Panics
    ///
    /// When resumed after it finished.
    pub fn resume(&mut self, input: Input<E>, now: f64) -> Step<T, E> {
        {
            let mut slot = self.io.lock();
            slot.input = Some(input);
            slot.now = now;
        }
        let procedure = self.procedure.as_mut().expect("operation resumed after it finished");
        let mut context = Context::from_waker(Waker::noop());
        match procedure.as_mut().poll(&mut context) {
            Poll::Ready(result) => {
                self.procedure = None;
                Step::Finished(result)
            }
            Poll::Pending => {
                let action = self.io.lock().action.take().expect("a pending procedure waits for an action");
                Step::Action(action)
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        self.procedure.is_none()
    }
}

/// What [`run`] reports a failure as.
#[derive(Debug, Error)]
pub enum DriverError {
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error("{0}")]
    Ui(Box<dyn std::error::Error + Send + Sync>),
}

impl External for DriverError {
    fn message(&self) -> String {
        self.to_string()
    }
}

/// The display side of a calibration driven by [`run`].
pub trait CalibrationUi: Send {
    /// Shows `event` and returns once the user has seen it; may block.
    fn event(&mut self, event: &UiEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    /// Fails when the user cancelled.
    fn check(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// A [`CalibrationUi`] that shows nothing.
#[derive(Debug, Default)]
pub struct NoUi;

impl CalibrationUi for NoUi {
    fn event(&mut self, _: &UiEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// Monotonic seconds since the first call, for [`Operation::resume`].
fn monotonic(start: Instant) -> f64 {
    start.elapsed().as_secs_f64()
}

fn read_once<C: Channel>(
    session: &mut TrackerSession<C>,
    log: &mut dyn FnMut(&LogRecord),
    notifications: &mut Vec<Message>,
) -> Result<(), DeviceError> {
    for message in session.receive(READ_SLICE_MS)? {
        if message.kind == STREAM_DATA_KIND {
            session.pending_streams.push(message);
        } else if message.kind == NOTIFICATION_KIND {
            let payload: String = message.payload.iter().map(|b| format!("{b:02x}")).collect();
            log(&LogRecord {
                kind: "notification",
                values: vec![("command", LogValue::Int(message.command.into())), ("payload", LogValue::Text(payload))],
            });
            notifications.push(message);
        }
    }
    Ok(())
}

fn call<C: Channel>(session: &mut TrackerSession<C>, call: &Call) -> Result<Vec<Value>, DeviceError> {
    let message = call.request(session.next_id());
    let timeout = call.timeout_s.map_or(tobii_device::session::REQUEST_TIMEOUT, Duration::from_secs_f64);
    let reply = session.call_message(&message, timeout)?;
    TrackerSession::<C>::reply_values(&reply)
}

/// Drives `operation` on a native session to its end, on the calling thread.
///
/// UI callbacks run on a scoped thread while this thread keeps reading the device: a
/// tracker whose subscribed stream goes unread for about two seconds stops answering for
/// the rest of its USB session (docs/gaze-calibration.md). Notifications read meanwhile are
/// appended to `notifications` and logged.
pub fn run<T, C: Channel>(
    mut operation: Operation<T, DriverError>,
    session: &mut TrackerSession<C>,
    ui: &mut dyn CalibrationUi,
    log: &mut dyn FnMut(&LogRecord),
    notifications: &mut Vec<Message>,
) -> OperationResult<T, DriverError> {
    let start = Instant::now();
    let mut input = Input::Start;
    loop {
        let action = match operation.resume(input, monotonic(start)) {
            Step::Finished(result) => return result,
            Step::Action(action) => action,
        };
        input = match action {
            Action::Call(request) => match call(session, &request) {
                Ok(values) => Input::Replied(values),
                Err(error) => Input::Failed(error.into()),
            },
            Action::Ui(event) => {
                let outcome = std::thread::scope(|scope| {
                    let shown = scope.spawn(|| ui.event(&event));
                    let mut read_error = None;
                    while !shown.is_finished() && read_error.is_none() {
                        read_error = read_once(session, log, notifications).err();
                    }
                    let shown = shown.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                    match (shown, read_error) {
                        (Err(error), _) => Err(DriverError::Ui(error)),
                        (Ok(()), Some(error)) => Err(error.into()),
                        (Ok(()), None) => Ok(()),
                    }
                });
                outcome.map_or_else(Input::Failed, |()| Input::Done)
            }
            Action::Check => ui.check().map_or_else(|error| Input::Failed(DriverError::Ui(error)), |()| Input::Done),
            Action::Read => match read_once(session, log, notifications) {
                Ok(()) => Input::Done,
                Err(error) => Input::Failed(error.into()),
            },
            Action::PendingStreams { take } => {
                Input::Streams(if take { session.take_pending_streams() } else { session.pending_streams.clone() })
            }
            Action::ClearStreams => {
                session.pending_streams.clear();
                Input::Done
            }
            Action::Log(record) => {
                log(&record);
                Input::Done
            }
        };
    }
}

/// Drives an operation that only sends requests (backups, display setup, profiles).
pub fn run_calls<T, C: Channel>(
    operation: Operation<T, DriverError>,
    session: &mut TrackerSession<C>,
) -> OperationResult<T, DriverError> {
    run(operation, session, &mut NoUi, &mut |_| {}, &mut Vec::new())
}
