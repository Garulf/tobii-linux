//! A scripted Eye Tracker 5 behind the session, built from the recovered request and reply
//! layouts (NOT device traffic): startup, calibration, stream demultiplexing, the stall of
//! a tracker whose endpoint is left unread, and reconnection.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tobii_device::channel::{Channel, ChannelError};
use tobii_device::session::{HEAD_POSE_REALM_EXTRA, HEAD_POSE_REALM_KEY, TrackerSession, UNLOCK_REALM_KEY};
use tobii_device::stop::{StopFlag, StopSignal};
use tobii_device::streams::{ItemPump, StreamReader, StreamValue};
use tobii_device::supervisor::{Backoff, LinkHost, LinkState, TrackerLink};
use tobii_device::{DeviceError, threads};
use tobii_protocol::Message;
use tobii_protocol::camera_info::{CameraRecord, HardwareInfo, hardware_info_response};
use tobii_protocol::commands::{RESPONSE_KIND, STREAM_DATA_KIND, Stream, blob_field, command, realm_digest};
use tobii_protocol::fields::{encode_payload, int32_field, uint32_field};
use tobii_protocol::gaze::build::{columns_payload, int32_column, int64_column, uint32_column, vec2_column};
use tobii_protocol::ttp::{Decoder, Field};

const SALT: &[u8] = b"0123456789abcdef";

fn camera() -> CameraRecord {
    CameraRecord {
        int32_0x00: -2,
        float32_0x04: 1.5,
        float32_0x08: -0.25,
        triple_0x10: [1.0, -2.0, 0.5],
        doubles_0x28: (0..15).map(f64::from).collect(),
        int32_0xa0: 640,
        int32_0xa4: 480,
        int32_0xa8: 1,
        series_0xb0: vec![0.125],
        triple_0x2b0: [0.0, 0.0, 1.0],
        triple_0x2c8: [-1.0, 0.0, 0.0],
        double_0x2e0: 2.0,
    }
}

fn reply(request: &Message, items: &[Field]) -> Message {
    let payload = if items.is_empty() { Vec::new() } else { encode_payload(items).unwrap() };
    Message::new(RESPONSE_KIND, request.request_id, 0, request.command, 0, payload)
}

fn stream_message(stream: Stream, payload: Vec<u8>) -> Message {
    Message::new(STREAM_DATA_KIND, 0, 0, stream.wire_id(), 0, payload)
}

fn image_message(timestamp: i64) -> Message {
    let blob = blob_field(&[7; 12]);
    let columns = [
        (1, int64_column(timestamp)),
        (2, uint32_column(8)),
        (3, uint32_column(4)),
        (4, uint32_column(3)),
        (5, vec![blob]),
    ];
    stream_message(Stream::Image, columns_payload(&columns).unwrap())
}

fn gaze_message(timestamp: i64) -> Message {
    let columns = [(1, int64_column(timestamp)), (27, uint32_column(1)), (28, vec2_column([0.25, 0.75]).unwrap())];
    stream_message(Stream::Gaze, columns_payload(&columns).unwrap())
}

fn presence_message(timestamp: i64, value: i32) -> Message {
    let columns = [(1, int64_column(timestamp)), (2, int32_column(value))];
    stream_message(Stream::Presence, columns_payload(&columns).unwrap())
}

/// Device-side state shared between the test and the channel the session owns.
#[derive(Debug, Default)]
struct Device {
    requests: Vec<Message>,
    outgoing: Vec<u8>,
    subscribed: Vec<u32>,
    clock_us: i64,
    last_stream: Option<Instant>,
    last_read: Option<Instant>,
    stalled: bool,
    unplugged: bool,
    refuse: Vec<u32>,
}

#[derive(Debug, Clone)]
struct FakeTracker {
    device: Arc<Mutex<Device>>,
    decoder: Arc<Mutex<Decoder>>,
    /// Unread time after which the tracker stops sending for good.
    stall_after: Duration,
    period: Duration,
}

impl FakeTracker {
    fn new(stall_after: Duration) -> Self {
        Self {
            device: Arc::default(),
            decoder: Arc::new(Mutex::new(Decoder::default())),
            stall_after,
            period: Duration::from_millis(5),
        }
    }

    fn commands(&self) -> Vec<u32> {
        self.device.lock().unwrap().requests.iter().map(|m| m.command).collect()
    }

    fn answer(&self, device: &mut Device, request: Message) {
        let outgoing = match request.command {
            command::UNLOCK_PPU => vec![reply(&request, &[int32_field(1), int32_field(77), int32_field(0)])],
            command::OPEN_REALM_SALT => vec![reply(&request, &[uint32_field(5), uint32_field(6), blob_field(SALT)])],
            0x848 => {
                let info = HardwareInfo { cameras_0x8: vec![camera()], points_0xbb0: vec![], int32_0xf70: 5 };
                vec![hardware_info_response(&info, request.request_id, (0, 0)).unwrap()]
            }
            command::SUBSCRIBE_STREAM => {
                let stream = u32::from_be_bytes(request.payload[7..11].try_into().unwrap());
                if device.refuse.contains(&stream) {
                    vec![Message { word4: 0x2000_0503, ..reply(&request, &[]) }]
                } else {
                    device.subscribed.push(stream);
                    vec![reply(&request, &[])]
                }
            }
            command::UNSUBSCRIBE_STREAM => {
                let stream = u32::from_be_bytes(request.payload[7..11].try_into().unwrap());
                device.subscribed.retain(|s| *s != stream);
                vec![reply(&request, &[])]
            }
            _ => vec![reply(&request, &[])],
        };
        device.requests.push(request);
        for message in outgoing {
            device.outgoing.extend(message.encode().unwrap());
        }
    }

    fn stream(&self, device: &mut Device, now: Instant) {
        if device.subscribed.is_empty() {
            device.last_stream = None;
            return;
        }
        let last = *device.last_stream.get_or_insert(now);
        let due = (now - last).as_micros() / self.period.as_micros();
        for _ in 0..due.min(50) {
            device.clock_us += self.period.as_micros() as i64;
            let t = device.clock_us;
            for &stream in &device.subscribed.clone() {
                let message = match stream {
                    0x500 => gaze_message(t),
                    0x501 => image_message(t),
                    0x504 => presence_message(t, 2),
                    other => Message::new(STREAM_DATA_KIND, 0, 0, other, 0, Vec::new()),
                };
                device.outgoing.extend(message.encode().unwrap());
            }
        }
        if due > 0 {
            device.last_stream = Some(now);
        }
    }
}

impl Channel for FakeTracker {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        let mut device = self.device.lock().unwrap();
        if device.unplugged {
            return Err(ChannelError::new(true, "bulk transfer: libusb error -4"));
        }
        for request in self.decoder.lock().unwrap().feed(data).unwrap() {
            if !device.stalled {
                self.answer(&mut device, request);
            }
        }
        Ok(())
    }

    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        let now = Instant::now();
        let mut device = self.device.lock().unwrap();
        if device.unplugged {
            return Err(ChannelError::new(true, "bulk transfer: libusb error -4"));
        }
        let unread = device.last_read.map_or(Duration::ZERO, |last| now - last);
        device.last_read = Some(now);
        if !device.subscribed.is_empty() && unread > self.stall_after {
            device.stalled = true;
        }
        if device.stalled {
            device.outgoing.clear();
            drop(device);
            std::thread::sleep(Duration::from_millis(u64::from(timeout_ms.min(20))));
            return Ok(Vec::new());
        }
        self.stream(&mut device, now);
        if device.outgoing.is_empty() {
            drop(device);
            std::thread::sleep(Duration::from_millis(u64::from(timeout_ms.min(2))));
            return Ok(Vec::new());
        }
        let take = device.outgoing.len().min(16384);
        Ok(device.outgoing.drain(..take).collect())
    }
}

fn started(stall_after: Duration) -> (FakeTracker, TrackerSession<FakeTracker>) {
    let tracker = FakeTracker::new(stall_after);
    let mut session = TrackerSession::new(tracker.clone());
    session.initialize().unwrap();
    (tracker, session)
}

#[test]
fn startup_opens_both_realms_and_reads_the_camera_calibration() {
    let (tracker, mut session) = started(Duration::from_secs(10));
    let calibration = session.camera_calibration(0).unwrap();
    assert_eq!(tracker.commands(), [1000, 1600, 1900, 1910, 1900, 1911, 0x848, 1915]);
    let requests = tracker.device.lock().unwrap().requests.clone();
    let contains = |haystack: &[u8], needle: &[u8]| haystack.windows(needle.len()).any(|w| w == needle);
    assert!(contains(&requests[3].payload, &realm_digest(UNLOCK_REALM_KEY, SALT)));
    assert!(contains(&requests[5].payload, &realm_digest(&HEAD_POSE_REALM_KEY, SALT)));
    assert!(contains(&requests[5].payload, &HEAD_POSE_REALM_EXTRA));
    assert_eq!(calibration.translation, [1.0, -2.0, 0.5]);
    assert_eq!(calibration.angles, [0.0, 1.0, 2.0]);
    assert_eq!(session.startup_replies().iter().map(|(c, _)| *c).collect::<Vec<_>>(), [1000, 1600]);
    assert!(matches!(session.camera_calibration(3), Err(DeviceError::NoCamera { index: 3, count: 1 })));
}

#[test]
fn a_refused_command_fails_and_a_silent_device_kills_the_session() {
    let (tracker, mut session) = started(Duration::from_secs(10));
    tracker.device.lock().unwrap().refuse.push(Stream::Image.wire_id());
    assert!(matches!(session.subscribe_images(), Err(DeviceError::Failed { status: 12, .. })));
    assert!(!session.is_dead());
    tracker.device.lock().unwrap().stalled = true;
    let started = Instant::now();
    let timed_out = session.call_message(&tobii_protocol::ttp::protocol_upgrade(99), Duration::from_millis(200));
    assert!(matches!(timed_out, Err(DeviceError::Timeout { command: 1000 })));
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(session.is_dead());
}

#[test]
fn streams_are_demultiplexed_in_arrival_order() {
    let (tracker, mut session) = started(Duration::from_secs(10));
    tracker.device.lock().unwrap().refuse.push(Stream::Presence.wire_id());
    let streams = [Stream::Gaze, Stream::Image, Stream::Presence, Stream::Sync];
    let mut reader = StreamReader::new(&streams, &[Stream::Presence]);
    reader.subscribe(&mut session).unwrap();
    assert_eq!(reader.subscribed, [Stream::Gaze, Stream::Image, Stream::Sync]);
    assert_eq!(reader.refused[0].0, Stream::Presence);
    let stop = StopFlag::new();
    let items: Vec<_> = reader.items(&mut session, Duration::from_secs(1), None, &stop).take(30).collect();
    let items: Vec<_> = items.into_iter().map(Result::unwrap).collect();
    assert_eq!(items[..3].iter().map(|i| i.stream).collect::<Vec<_>>(), [Stream::Gaze, Stream::Image, Stream::Sync]);
    let StreamValue::Gaze(frame) = &items[0].value else { panic!("gaze first") };
    assert_eq!((frame.gaze_point.valid, frame.gaze_point.point), (1, [0.25, 0.75]));
    let StreamValue::Image(image) = &items[1].value else { panic!("image second") };
    assert_eq!((image.width, image.height, image.timestamp), (4, 3, frame.timestamp_us()));
    assert!(matches!(items[2].value, StreamValue::Raw));
    assert!(reader.stats.received[&Stream::Gaze] >= 10);
    assert_eq!(reader.unsubscribe(&mut session).unwrap(), [Stream::Sync, Stream::Image, Stream::Gaze]);
    assert!(tracker.device.lock().unwrap().subscribed.is_empty());
}

#[test]
fn a_dropped_gaze_message_and_a_malformed_image_are_counted_not_yielded() {
    let (_, mut session) = started(Duration::from_secs(10));
    let mut reader = StreamReader::new(&[Stream::Gaze, Stream::Image], &[]);
    let mistyped = columns_payload(&[(1, int64_column(5)), (7, int32_column(0))]).unwrap();
    let mut broken = image_message(1);
    broken.payload.truncate(broken.payload.len() - 3);
    let unexpected = presence_message(1, 2);
    let good = gaze_message(9);
    session.pending_streams = vec![stream_message(Stream::Gaze, mistyped), broken, unexpected, good];
    let mut pump = ItemPump::new(session.take_pending_streams(), Duration::from_secs(1));
    let stop = StopFlag::new();
    let item = pump.next(&mut reader, &mut session, None, &stop).unwrap().unwrap();
    assert_eq!(item.stream, Stream::Gaze);
    assert_eq!(reader.stats.dropped[&Stream::Gaze], 1);
    assert_eq!(reader.stats.decode_errors[&Stream::Image], 1);
    assert_eq!(reader.stats.unexpected, 1);
    assert!(reader.stats.last_error.as_deref().unwrap().starts_with("image message: "));
}

#[test]
fn a_subscribed_stream_left_unread_stalls_the_tracker_and_the_session_dies() {
    let (_, mut session) = started(Duration::from_millis(400));
    let mut reader = StreamReader::new(&[Stream::Gaze], &[]);
    reader.subscribe(&mut session).unwrap();
    let stop = StopFlag::new();
    let mut items = reader.items(&mut session, Duration::from_millis(300), None, &stop);
    assert!(items.next().unwrap().is_ok());
    std::thread::sleep(Duration::from_millis(900));
    let error = items.find_map(Result::err).unwrap();
    assert!(matches!(error, DeviceError::Silent { .. }), "{error}");
    assert_eq!(error.to_string(), "The tracker sent no stream data for 0.3 s");
    assert!(session.is_dead());
    assert!(reader.unsubscribe(&mut session).unwrap().is_empty());
}

#[test]
fn reading_on_its_own_thread_keeps_a_slow_consumer_from_stalling_the_tracker() {
    let (_, mut session) = started(Duration::from_millis(400));
    let mut reader = StreamReader::new(&[Stream::Gaze], &[]);
    reader.subscribe(&mut session).unwrap();
    let stop = StopFlag::new();
    let reading_stop = stop.clone();
    let source = std::iter::from_fn(move || {
        let mut pump = ItemPump::new(session.take_pending_streams(), Duration::from_millis(300));
        let mut items = Vec::new();
        while let Some(item) = pump.next(&mut reader, &mut session, None, &reading_stop).transpose() {
            items.push(item);
            if items.len() == 400 {
                break;
            }
        }
        Some(items)
    })
    .take(1)
    .flatten();
    let mut relay = threads::drained(source, 1000);
    let first = relay.next().unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let rest: Vec<_> = relay.collect();
    assert!(matches!(first.value, StreamValue::Gaze(_)));
    assert!(rest.iter().all(Result::is_ok), "the tracker never stalled");
    assert!(rest.len() >= 300);
    stop.set();
}

/// How a scripted tracker ends its session after 20 image frames.
#[derive(Clone, Copy)]
enum Ending {
    Silent,
    Unplugged,
    Never,
}

struct Reconnecting {
    trackers: Vec<(FakeTracker, Ending)>,
    states: Arc<Mutex<Vec<(LinkState, String)>>>,
    started: Instant,
    frames: usize,
}

impl LinkHost for Reconnecting {
    type Error = DeviceError;

    fn on_state(&mut self, state: LinkState, detail: &str) -> Result<(), DeviceError> {
        self.states.lock().unwrap().push((state, detail.to_string()));
        Ok(())
    }

    fn attempt(&mut self, stop: &dyn StopSignal, opened: &mut dyn FnMut()) -> Result<(), DeviceError> {
        if self.trackers.is_empty() {
            return Err(DeviceError::NoTracker("No Eye Tracker 5 is visible to libusb".into()));
        }
        let (tracker, ending) = self.trackers.remove(0);
        let mut session = TrackerSession::new(tracker.clone());
        session.initialize()?;
        opened();
        let mut reader = StreamReader::new(&[Stream::Image], &[]);
        reader.subscribe(&mut session)?;
        let mut pump = ItemPump::new(session.take_pending_streams(), Duration::from_millis(300));
        let mut frames = 0;
        while let Some(item) = pump.next(&mut reader, &mut session, None, stop)? {
            frames += 1;
            self.frames += 1;
            if frames == 20 {
                let mut device = tracker.device.lock().unwrap();
                match ending {
                    Ending::Silent => device.stalled = true,
                    Ending::Unplugged => device.unplugged = true,
                    Ending::Never => {}
                }
            }
            assert!(matches!(item.value, StreamValue::Image(_)));
        }
        reader.unsubscribe(&mut session)?;
        Ok(())
    }

    fn now_s(&mut self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

#[test]
fn the_link_reconnects_after_a_silent_and_an_unplugged_tracker() {
    let stop = StopFlag::new();
    let states = Arc::new(Mutex::new(Vec::new()));
    let tracker = || FakeTracker::new(Duration::from_secs(10));
    let mut host = Reconnecting {
        trackers: vec![(tracker(), Ending::Silent), (tracker(), Ending::Unplugged), (tracker(), Ending::Never)],
        states: Arc::clone(&states),
        started: Instant::now(),
        frames: 0,
    };
    let watcher = {
        let (stop, states) = (stop.clone(), Arc::clone(&states));
        std::thread::spawn(move || {
            while states.lock().unwrap().iter().filter(|(s, _)| *s == LinkState::Starting).count() < 3 {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(100));
            stop.set();
        })
    };
    let mut link = TrackerLink::new(Backoff::new(0.01, 0.02, 10.0));
    link.run(&mut host, &stop);
    watcher.join().unwrap();
    let states = states.lock().unwrap();
    let names: Vec<&str> = states.iter().map(|(s, _)| s.name()).collect();
    assert_eq!(names[..5], ["starting", "lost", "starting", "lost", "starting"]);
    assert_eq!(states[1].1, "DeviceError: The tracker sent no stream data for 0.3 s");
    assert!(states[3].1.starts_with("USBError: bulk transfer"), "{}", states[3].1);
    assert_eq!(*names.last().unwrap(), "stopped");
    assert_eq!((link.sessions(), link.failures()), (3, 2));
    assert!(host.frames > 40);
}
