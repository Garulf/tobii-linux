//! The game service across an unplugged tracker: ReplugTests and TetTcpReplugTests of
//! tests/test_supervisor.py, driving cli::ServiceLink and serve_tracker with scripted
//! trackers and a fake frame processor. BackoffTests and TrackerLinkTests are
//! crates/tobii-device/src/supervisor.rs's own tests.

mod common;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use common::fake_tracker::FakeTracker;
use common::*;
use tobii_device::streams::SILENCE_TIMEOUT;
use tobii_device::supervisor::{Backoff, LinkCounters, TrackerLink};
use tobii_device::{CameraCalibration, DeviceError, StopFlag, TrackerSession};
use tobii_gaze::operation::{Operation, OperationError};
use tobii_headpose::pipeline::HeadPoseSample;
use tobii_linux::cli::{ServiceLink, service_feed};
use tobii_linux::error::Result;
use tobii_linux::service::clock::{Clocks, ServiceTime, wine_clock};
use tobii_linux::service::{DeviceCallError, Options, PrpService};
use tobii_linux::tracker::{FrameProcessor, HostTime};
use tobii_protocol::commands::{Stream, command, header_only_request, unsubscribe_stream};
use tobii_protocol::fields::{Value, values};
use tobii_protocol::gaze::PRESENCE_PRESENT;
use tobii_protocol::ttp::Decoder;
use tobii_protocol::{Message, ProtocolError};
use tobii_service::prp::{DEFAULT_MAX_PAYLOAD, HEAD_POSE, decode_envelope};
use tobii_service::prp_service::{PROPERTY_DEVICE_PAUSED, paused_notification, word_value};
use tobii_service::prp_session::{
    COMMAND_CHANNEL, COMMAND_PROPERTY_SUBSCRIBE, COMMAND_STREAM_START, ConnectRequest, PropertyResponse,
    SUBSCRIPTION_CHANNEL, decode_command_response, decode_property_subscribe_response, encode_connect_request,
    encode_property_subscribe_request, encode_stream_request,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const WAIT: Duration = Duration::from_secs(5);

/// Counts calibrations; every sample is invalid.
struct FakeProcessor {
    starts: Arc<AtomicUsize>,
}

impl FrameProcessor for FakeProcessor {
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
type Plugged = Arc<Mutex<Option<FakeTracker>>>;

fn plug(slot: &Plugged, device: Option<FakeTracker>) {
    *slot.lock().unwrap() = device;
}

fn open(slot: &Plugged) -> Result<TrackerSession<FakeTracker>> {
    let device = slot.lock().unwrap().clone();
    let device = device.ok_or_else(|| DeviceError::NoTracker("No Eye Tracker 5 is visible to libusb".into()))?;
    let mut session = TrackerSession::new(device);
    session.initialize()?;
    Ok(session)
}

/// `cli.service_link` of the Python tests: a TrackerLink thread feeding `service`.
struct Link {
    stop: StopFlag,
    counters: Arc<LinkCounters>,
    starts: Arc<AtomicUsize>,
    streams: Arc<Mutex<Option<BTreeSet<u32>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Link {
    fn start(service: &PrpService, tet_tcp: bool, slot: &Plugged) -> Self {
        let (events, received) = tokio::sync::mpsc::unbounded_channel();
        {
            let service = service.clone();
            tokio::task::spawn_local(async move { service.follow(received).await });
        }
        let mut feed = service_feed(&events, false, tet_tcp);
        let streams: Arc<Mutex<Option<BTreeSet<u32>>>> = Arc::default();
        if let Some(device) = feed.device.take() {
            let streams = Arc::clone(&streams);
            feed.device = Some(Box::new(move |replies| {
                *streams.lock().unwrap() = Some(replies.streams.clone());
                device(replies);
            }));
        }
        let clock = HostTime(Arc::new(ServiceTime(Arc::clone(service.clock()))));
        let paused = service.pause_flag();
        let mut link = TrackerLink::new(Backoff::new(0.02, 0.05, 10.0));
        let (stop, counters, starts) = (StopFlag::new(), Arc::clone(&link.counters), Arc::new(AtomicUsize::new(0)));
        let thread = {
            let (stop, starts, slot) = (stop.clone(), Arc::clone(&starts), Arc::clone(slot));
            std::thread::spawn(move || {
                let mut processor = FakeProcessor { starts };
                let mut open = || open(&slot);
                let mut host =
                    ServiceLink { open: &mut open, processor: &mut processor, clock, feed: &feed, events, paused };
                link.run(&mut host, &stop);
            })
        };
        Self { stop, counters, starts, streams, thread: Some(thread) }
    }

    fn sessions_and_failures(&self) -> (u64, u64) {
        (self.counters.sessions(), self.counters.failures())
    }

    /// Stops the link and waits for its thread; true when it finished in time.
    async fn stop(&mut self) -> bool {
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

async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "condition not reached: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn service(options: Options) -> (PrpService, u16, tokio::task::JoinHandle<()>) {
    common::service(options, wine_clock("auto", Clocks::Host).unwrap()).await
}

/// Waits for a session to attach and for the device's presence record, which reaches the
/// service as a separate event that clients subscribed in between are notified of.
async fn streaming(service: &PrpService) {
    until("attached", || service.tracker_attached()).await;
    until("present", || service.presence().value == Some(PRESENCE_PRESENT)).await;
}

/// A connected client that subscribed presence and started the head-pose stream.
async fn streaming_client(port: u16) -> (RelayedChannel, RelayedChannel, PropertyResponse) {
    let mut command = RelayedChannel::open(port, "ET5-TEST", CLIENT_PID).await;
    let info = command.handshake(COMMAND_CHANNEL, "client_TOBIIPRP-ET5-TEST_00001234_1", CLIENT_PID).await;
    let pipe = info.subscription_url.strip_prefix("prp-internal://").unwrap();
    let mut subscription = RelayedChannel::open(port, pipe, CLIENT_PID).await;
    let sub_info = subscription.handshake(SUBSCRIPTION_CHANNEL, "client_sub", CLIENT_PID).await;
    let request = ConnectRequest {
        subscription_client_id: sub_info.client_id,
        process_id: CLIENT_PID,
        image_directory: "\\Games\\".encode_utf16().flat_map(u16::to_le_bytes).collect(),
        image_name: "StarCitizen".encode_utf16().flat_map(u16::to_le_bytes).collect(),
        field_of_use: 1,
        ..ConnectRequest::default()
    };
    command.send_packet(&encode_connect_request(1, &request, true).unwrap()).await;
    assert_eq!(decode_command_response(&command.packet().await, true).unwrap().status, 0);
    command.send_packet(&encode_property_subscribe_request(2, 0xB, COMMAND_PROPERTY_SUBSCRIBE).unwrap()).await;
    let presence = decode_property_subscribe_response(&command.packet().await).unwrap();
    command.send_packet(&encode_stream_request(3, COMMAND_STREAM_START, HEAD_POSE as i32).unwrap()).await;
    assert_eq!(decode_command_response(&command.packet().await, true).unwrap().status, 0);
    (command, subscription, presence)
}

fn presence_value(response: &PropertyResponse) -> u32 {
    u32::from_le_bytes(response.block[12..16].try_into().unwrap())
}

#[test]
fn service_survives_unplug_and_replug() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let slot = Plugged::default();
        let mut link = Link::start(&service, false, &slot);
        let mut refused = RelayedChannel::open(port, "ET5-TEST", CLIENT_PID).await;
        refused.send(&[0; 8]).await;
        assert!(refused.closed().await);

        let device = FakeTracker::streaming();
        plug(&slot, Some(device.clone()));
        streaming(&service).await;
        let (mut command, mut subscription, presence) = streaming_client(port).await;
        assert_eq!(presence.status, 0);
        for _ in 0..3 {
            let package = subscription.packet().await;
            assert_eq!(decode_envelope(&package, DEFAULT_MAX_PAYLOAD).unwrap()[8], 1);
        }

        device.unplug();
        plug(&slot, None);
        assert!(command.closed().await);
        assert!(subscription.closed().await);
        until("detached", || !service.tracker_attached()).await;
        assert_eq!(service.hub().subscribers(), 0);

        plug(&slot, Some(FakeTracker::streaming()));
        streaming(&service).await;
        let (_command, mut subscription, presence) = streaming_client(port).await;
        assert_eq!(presence_value(&presence), PRESENCE_PRESENT);
        assert!(!subscription.packet().await.is_empty());
        assert_eq!(link.starts.load(Ordering::SeqCst), 2);
        assert_eq!(link.sessions_and_failures(), (2, 1));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn stopping_unsubscribes_and_disconnects_clients() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        until("attached", || service.tracker_attached()).await;
        let (mut command, mut subscription, _) = streaming_client(port).await;
        assert!(link.stop().await);
        let payloads = |messages: Vec<Message>| {
            let mut payloads: Vec<Vec<u8>> = messages.into_iter().map(|m| m.payload).collect();
            payloads.sort();
            payloads
        };
        let unsubscribed = device.log().into_iter().filter(|m| m.command == command::UNSUBSCRIBE_STREAM).collect();
        let expected =
            [Stream::Gaze, Stream::Image, Stream::Presence].map(|s| unsubscribe_stream(0, s.index()).unwrap());
        assert_eq!(payloads(unsubscribed), payloads(expected.to_vec()));
        assert!(command.closed().await);
        assert!(subscription.closed().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn presence_starts_unknown_then_follows_the_device() {
    run(async {
        let (service, _, accept) = service(options("ET5-TEST")).await;
        let slot = Plugged::new(Mutex::new(Some(FakeTracker::streaming())));
        let mut link = Link::start(&service, false, &slot);
        until("present", || service.presence().value == Some(PRESENCE_PRESENT)).await;
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

/// The first reply of the tet-tcp route to `request`.
async fn ask(port: u16, request: Message) -> Message {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(&request.encode().unwrap()).await.unwrap();
    let (mut decoder, mut buffer) = (Decoder::default(), vec![0; 65536]);
    loop {
        let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer)).await.unwrap().unwrap();
        assert!(read > 0, "the route closed before replying");
        if let Some(reply) = decoder.feed(&buffer[..read]).unwrap().into_iter().next() {
            return reply;
        }
    }
}

#[test]
fn tet_tcp_replies_come_from_the_session_and_the_port_follows_the_tracker() {
    run(async {
        let options = Options { tet_tcp: true, tet_tcp_port: 0, ..options("ET5-TEST") };
        let (service, _, accept) = service(options).await;
        let tet_tcp = service.tet_tcp().unwrap().clone();
        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, true, &slot);
        until("route bound", || tet_tcp.port_bound().is_some()).await;
        let port = tet_tcp.port_bound().unwrap();
        let before = device.log().len();
        let unlock = ask(port, header_only_request(41, command::UNLOCK_PPU).unwrap()).await;
        assert_eq!((unlock.request_id, unlock.command, unlock.word4), (41, 1600, 0));
        assert!(!unlock.payload.is_empty());
        let status = ask(port, header_only_request(42, command::GET_DEVICE_STATUS).unwrap()).await;
        assert_eq!((status.request_id, status.word4), (42, 0));
        assert_eq!(device.log().len(), before, "clients never reach the tracker");
        let captured: BTreeSet<u32> = device.commands().into_iter().collect();
        assert!(captured.contains(&command::GET_DEVICE_INFO) && captured.contains(&command::GET_DISPLAY_AREA));
        assert_eq!(*link.streams.lock().unwrap(), Some(BTreeSet::from([0x500, 0x501, 0x504])));

        device.unplug();
        plug(&slot, None);
        until("route closed", || tet_tcp.port_bound().is_none()).await;
        plug(&slot, Some(FakeTracker::streaming()));
        until("route bound again", || tet_tcp.port_bound().is_some()).await;
        assert_eq!(link.sessions_and_failures(), (2, 1));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

fn device_status_requests(device: &FakeTracker) -> Vec<Message> {
    device.log().into_iter().filter(|m| m.command == command::GET_DEVICE_STATUS).collect()
}

#[test]
fn device_call_reaches_the_tracker_while_streaming() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        streaming(&service).await;
        let (_command, mut subscription, _) = streaming_client(port).await;
        subscription.packet().await;
        let before = device_status_requests(&device).len();
        let reply = service
            .device_call(|id| header_only_request(id, command::GET_DEVICE_STATUS), Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!((reply.command, reply.word4), (command::GET_DEVICE_STATUS, 0));
        let sent = device_status_requests(&device);
        assert_eq!(sent.len(), before + 1);
        assert_eq!(sent.last().unwrap().request_id, reply.request_id);
        let unbuilt = service.device_call(|_| Err(ProtocolError::new("unbuilt")), Duration::from_secs(1)).await;
        assert!(matches!(unbuilt, Err(DeviceCallError::Device(DeviceError::Protocol(_)))), "{unbuilt:?}");
        for _ in 0..3 {
            let package = subscription.packet().await;
            assert_eq!(decode_envelope(&package, DEFAULT_MAX_PAYLOAD).unwrap()[8], 1);
        }
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn device_call_is_answered_while_the_tracker_sends_nothing() {
    run(async {
        let (service, _, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::scripted(Vec::new(), 1);
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        until("attached", || service.tracker_attached()).await;
        let reply = service
            .device_call(|id| header_only_request(id, command::GET_DEVICE_STATUS), Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(reply.word4, 0);
        assert_eq!(device_status_requests(&device).last().unwrap().request_id, reply.request_id);
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn device_call_without_tracker_is_not_attached() {
    run(async {
        let (service, _, accept) = service(options("ET5-TEST")).await;
        let outcome =
            service.device_call(|id| header_only_request(id, command::GET_DEVICE_STATUS), Duration::from_secs(1)).await;
        assert!(matches!(outcome, Err(DeviceCallError::NotAttached)), "{outcome:?}");

        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        streaming(&service).await;
        device.unplug();
        plug(&slot, None);
        until("detached", || !service.tracker_attached()).await;
        let outcome =
            service.device_call(|id| header_only_request(id, command::GET_DEVICE_STATUS), Duration::from_secs(1)).await;
        assert!(matches!(outcome, Err(DeviceCallError::NotAttached)), "{outcome:?}");
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn operations_run_their_calls_through_the_session() {
    run(async {
        let (service, _, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        streaming(&service).await;
        let before = device_status_requests(&device).len();
        let operation = Operation::new(|io| async move {
            io.check().await.map_err(OperationError::External)?;
            io.log("status", Vec::new()).await;
            let request = header_only_request(0, command::GET_DEVICE_STATUS)?;
            let values = io.call(request, Some(1.0)).await.map_err(OperationError::External)?;
            Ok(values.len())
        });
        assert_eq!(service.run_operation(operation).await.unwrap(), 0);
        assert_eq!(device_status_requests(&device).len(), before + 1);

        let reading = Operation::new(|io| async move { io.read().await.map_err(OperationError::External) });
        assert!(matches!(service.run_operation(reading).await, Err(OperationError::Gaze(_))));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

/// The value of every pause/resume request the tracker got: 1 pause, 0 resume.
fn pause_values(device: &FakeTracker) -> Vec<i64> {
    let values = |message: Message| values(&message.payload).unwrap().first().and_then(Value::as_int).unwrap();
    device.log().into_iter().filter(|m| m.command == command::PAUSE_DEVICE).map(values).collect()
}

/// Requests a pausable tracker gets from session startup, before the service sends any.
fn startup_writes() -> usize {
    let device = FakeTracker::pausable();
    TrackerSession::new(device.clone()).initialize().unwrap();
    device.log().len()
}

/// Skips stream packages until `expected` arrives.
async fn notified(subscription: &mut RelayedChannel, expected: &[u8]) {
    let deadline = Instant::now() + WAIT;
    while subscription.packet().await != expected {
        assert!(Instant::now() < deadline, "no notification {expected:02x?}");
    }
}

/// Stream packages that arrive within `window`; fails if the connection closes.
async fn packages_within(subscription: &mut RelayedChannel, window: Duration) -> Vec<Instant> {
    let deadline = Instant::now() + window;
    let mut arrivals = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match subscription.receive_within(remaining).await {
            Received::Timeout => return arrivals,
            Received::Closed => panic!("the service closed the client"),
            Received::Message(_) => arrivals.push(Instant::now()),
        }
    }
}

#[test]
fn disabling_pauses_and_keeps_clients() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::pausable();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        streaming(&service).await;
        let (mut command, mut subscription, _) = streaming_client(port).await;
        command
            .send_packet(
                &encode_property_subscribe_request(4, PROPERTY_DEVICE_PAUSED, COMMAND_PROPERTY_SUBSCRIBE).unwrap(),
            )
            .await;
        let watched = decode_property_subscribe_response(&command.packet().await).unwrap();
        assert_eq!((watched.status, watched.block), (0, word_value(0).0));
        subscription.packet().await;

        service.set_enabled(false).await.unwrap();
        assert!(!service.enabled());
        assert_eq!(pause_values(&device), [1]);
        notified(&mut subscription, &paused_notification(true)).await;
        let paused_at = Instant::now();
        let late = packages_within(&mut subscription, SILENCE_TIMEOUT + Duration::from_secs(1)).await;
        assert!(late.iter().all(|at| *at - paused_at < Duration::from_millis(500)), "data kept coming while paused");
        assert_eq!(command.receive_within(Duration::from_millis(50)).await, Received::Timeout);
        assert!(service.tracker_attached());
        assert_eq!(link.sessions_and_failures(), (1, 0));

        service.set_enabled(true).await.unwrap();
        assert!(service.enabled());
        assert_eq!(pause_values(&device), [1, 0]);
        notified(&mut subscription, &paused_notification(false)).await;
        let package = subscription.packet().await;
        assert_eq!(decode_envelope(&package, DEFAULT_MAX_PAYLOAD).unwrap()[8], 1);
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn replug_while_disabled_pauses_again() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::pausable();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        streaming(&service).await;
        service.set_enabled(false).await.unwrap();

        device.unplug();
        plug(&slot, None);
        until("detached", || !service.tracker_attached()).await;
        let replugged = FakeTracker::pausable();
        plug(&slot, Some(replugged.clone()));
        until("attached again", || service.tracker_attached()).await;
        let first = replugged.log()[startup_writes()].clone();
        assert_eq!(first.command, command::PAUSE_DEVICE);
        assert_eq!(pause_values(&replugged), [1]);
        let (_command, mut subscription, _) = streaming_client(port).await;
        assert!(packages_within(&mut subscription, Duration::from_millis(500)).await.is_empty());
        assert!(!service.enabled());

        service.set_enabled(true).await.unwrap();
        assert_eq!(pause_values(&replugged), [1, 0]);
        let package = subscription.packet().await;
        assert_eq!(decode_envelope(&package, DEFAULT_MAX_PAYLOAD).unwrap()[8], 1);
        assert_eq!(link.sessions_and_failures(), (2, 1));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}
