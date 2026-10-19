//! `cli.service_link` of the Python tests: a TrackerLink thread feeding a service from
//! scripted trackers through a fake frame processor.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tobii_device::supervisor::{Backoff, LinkCounters, TrackerLink};
use tobii_device::{CameraCalibration, DeviceError, StopFlag, TrackerSession};
use tobii_headpose::pipeline::HeadPoseSample;
use tobii_linux::cli::{ServiceLink, service_feed};
use tobii_linux::error::Result;
use tobii_linux::service::clock::ServiceTime;
use tobii_linux::service::{Corners, PrpService};
use tobii_linux::tracker::{FrameProcessor, HostTime};

use super::fake_tracker::FakeTracker;

pub const WAIT: Duration = Duration::from_secs(5);
/// First delay of the link's backoff, in seconds.
pub const BACKOFF_FIRST_S: f64 = 0.02;

/// Counts calibrations and keeps the display areas it is given; every sample is invalid.
pub struct FakeProcessor {
    starts: Arc<AtomicUsize>,
    areas: Arc<Mutex<Vec<Corners>>>,
}

impl FrameProcessor for FakeProcessor {
    fn set_display_area(&mut self, area: &Corners) {
        self.areas.lock().unwrap().push(*area);
    }

    fn start(&mut self, _: &CameraCalibration) {
        self.starts.fetch_add(1, Ordering::SeqCst);
    }

    fn process(&mut self, frame: &tobii_protocol::frames::ImageFrame) -> Result<HeadPoseSample> {
        Ok(HeadPoseSample {
            timestamp_us: frame.timestamp,
            valid: false,
            position_mm: [0.0; 3],
            rotation_rad: [0.0; 3],
        })
    }
}

/// The tracker the next session finds, if any.
pub type Plugged = Arc<Mutex<Option<FakeTracker>>>;

pub fn plug(slot: &Plugged, device: Option<FakeTracker>) {
    *slot.lock().unwrap() = device;
}

fn open(slot: &Plugged) -> Result<TrackerSession<FakeTracker>> {
    let device = slot.lock().unwrap().clone();
    let device = device.ok_or_else(|| DeviceError::NoTracker("No Eye Tracker 5 is visible to libusb".into()))?;
    let mut session = TrackerSession::new(device);
    session.initialize()?;
    Ok(session)
}

/// A TrackerLink thread feeding `service` from the tracker in a slot.
pub struct Link {
    stop: StopFlag,
    counters: Arc<LinkCounters>,
    pub starts: Arc<AtomicUsize>,
    pub areas: Arc<Mutex<Vec<Corners>>>,
    pub streams: Arc<Mutex<Option<BTreeSet<u32>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Link {
    pub fn start(service: &PrpService, tet_tcp: bool, slot: &Plugged) -> Self {
        Self::with_display(service, tet_tcp, slot, None)
    }

    /// A link whose sessions use `display_area` instead of the tracker's.
    pub fn with_display(service: &PrpService, tet_tcp: bool, slot: &Plugged, display_area: Option<Corners>) -> Self {
        let (events, received) = tokio::sync::mpsc::unbounded_channel();
        {
            let service = service.clone();
            tokio::task::spawn_local(async move { service.follow(received).await });
        }
        let mut feed = service_feed(&events, tet_tcp);
        let streams: Arc<Mutex<Option<BTreeSet<u32>>>> = Arc::default();
        if let Some(device) = feed.device.take() {
            let streams = Arc::clone(&streams);
            feed.device = Some(Box::new(move |replies| {
                *streams.lock().unwrap() = Some(replies.streams.clone());
                device(replies);
            }));
        }
        let clock = HostTime(Arc::new(ServiceTime(Arc::clone(service.clock()))));
        let (paused, gate) = (service.pause_flag(), service.release_gate());
        let mut link = TrackerLink::new(Backoff::new(BACKOFF_FIRST_S, 0.05, 10.0));
        let (stop, counters, starts) = (StopFlag::new(), Arc::clone(&link.counters), Arc::new(AtomicUsize::new(0)));
        let areas: Arc<Mutex<Vec<Corners>>> = Arc::default();
        let thread = {
            let (stop, starts, slot) = (stop.clone(), Arc::clone(&starts), Arc::clone(slot));
            let areas = Arc::clone(&areas);
            std::thread::spawn(move || {
                let mut processor = FakeProcessor { starts, areas };
                let mut open = || open(&slot);
                let mut host = ServiceLink {
                    open: &mut open,
                    processor: &mut processor,
                    clock,
                    feed: &feed,
                    events,
                    paused,
                    gate,
                    display_area,
                };
                link.run(&mut host, &stop);
            })
        };
        Self { stop, counters, starts, areas, streams, thread: Some(thread) }
    }

    pub fn sessions_and_failures(&self) -> (u64, u64) {
        (self.counters.sessions(), self.counters.failures())
    }

    /// Stops the link and waits for its thread; true when it finished in time.
    pub async fn stop(&mut self) -> bool {
        self.stop.set();
        let Some(thread) = self.thread.take() else { return true };
        let deadline = Instant::now() + WAIT;
        while !thread.is_finished() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let finished = thread.is_finished();
        if finished {
            thread.join().unwrap();
        }
        finished
    }
}

pub async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "condition not reached: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
