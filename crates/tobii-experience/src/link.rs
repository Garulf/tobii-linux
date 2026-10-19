//! The link to `tobii-linux serve`: one thread that keeps a control connection open and
//! reconnects when the service goes away or restarts.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tobii_control::{ControlClient, ControlError, GazeSample, Incoming, Request, Status};

const CLIENT_NAME: &str = "experience";
const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the link thread reports.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkUpdate {
    /// The latest service status, or `None` when the service is unreachable.
    Status(Option<Status>),
    Gaze(GazeSample),
    /// A request the service refused or that could not be sent.
    Failed(String),
}

/// A thread that keeps a control connection open; dropping it stops the thread.
///
/// The service ends a gaze subscription with its connection, so the link remembers the last
/// `SubscribeGaze` value it was given and sends `SubscribeGaze(true)` again right after every
/// reconnect when that value is true.
#[derive(Debug)]
pub struct Link {
    requests: Sender<Request>,
    gaze: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Link {
    /// Starts the thread. `on_incoming` is called from that thread: with `Status(None)` when
    /// a connection attempt fails or the connection drops (once per outage), and with the
    /// service's status and gaze samples while connected.
    pub fn start(addr: SocketAddr, on_incoming: impl Fn(LinkUpdate) + Send + 'static) -> Link {
        let (requests, queue) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let gaze = Arc::new(AtomicBool::new(false));
        let worker = Worker { addr, on_incoming: Box::new(on_incoming), queue, stop: stop.clone(), gaze: gaze.clone() };
        let thread = std::thread::Builder::new()
            .name("tobii-experience-link".into())
            .spawn(move || worker.run())
            .expect("the OS refused to start the link thread");
        Link { requests, gaze, stop, thread: Some(thread) }
    }

    /// Queues `request` for the link thread. Requests that are queued while the link is not
    /// connected are dropped and reported as [`LinkUpdate::Failed`], except `SubscribeGaze`:
    /// its value is remembered and replayed after the next connect when it is true. The
    /// reply status arrives as [`LinkUpdate::Status`].
    pub fn send(&self, request: Request) {
        if let Request::SubscribeGaze(on) = request {
            self.gaze.store(on, Ordering::Relaxed);
        }
        let _ = self.requests.send(request);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Worker {
    addr: SocketAddr,
    on_incoming: Box<dyn Fn(LinkUpdate) + Send>,
    queue: Receiver<Request>,
    stop: Arc<AtomicBool>,
    gaze: Arc<AtomicBool>,
}

impl Worker {
    fn run(self) {
        let mut reported_outage = false;
        while !self.stopped() {
            if let Ok(client) = ControlClient::connect(self.addr, CLIENT_NAME, CONNECT_TIMEOUT) {
                reported_outage = false;
                self.serve(client);
            }
            if !reported_outage {
                (self.on_incoming)(LinkUpdate::Status(None));
                reported_outage = true;
            }
            self.wait_to_reconnect();
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn serve(&self, mut client: ControlClient) {
        if self.gaze.load(Ordering::Relaxed) && !self.dispatch(&mut client, Request::SubscribeGaze(true)) {
            return;
        }
        while !self.stopped() {
            while let Ok(request) = self.queue.try_recv() {
                if self.stopped() || !self.dispatch(&mut client, request) {
                    return;
                }
            }
            match client.next(POLL_INTERVAL) {
                Ok(Some(Incoming::Event(tobii_control::Event::Status(status)))) => {
                    (self.on_incoming)(LinkUpdate::Status(Some(status)));
                }
                Ok(Some(Incoming::Gaze(sample))) => (self.on_incoming)(LinkUpdate::Gaze(sample)),
                Ok(None) => {}
                Err(_) => return,
            }
        }
    }

    /// Sends one request; false when the connection is no longer usable.
    fn dispatch(&self, client: &mut ControlClient, request: Request) -> bool {
        match client.request(request, REQUEST_TIMEOUT) {
            Ok(status) => (self.on_incoming)(LinkUpdate::Status(Some(status))),
            Err(ControlError::Refused(reason)) => (self.on_incoming)(LinkUpdate::Failed(reason)),
            Err(error) => {
                (self.on_incoming)(LinkUpdate::Failed(error.to_string()));
                return false;
            }
        }
        true
    }

    fn wait_to_reconnect(&self) {
        let deadline = Instant::now() + RECONNECT_INTERVAL;
        while !self.stopped() {
            self.fail_queued_requests();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            std::thread::sleep(remaining.min(POLL_INTERVAL));
        }
    }

    fn fail_queued_requests(&self) {
        while let Ok(request) = self.queue.try_recv() {
            if matches!(request, Request::SubscribeGaze(_)) {
                continue;
            }
            (self.on_incoming)(LinkUpdate::Failed(format!("The service is not connected: {request:?} was dropped")));
        }
    }
}
