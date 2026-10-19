//! Scripted Eye Tracker 5 channels, built from the recovered request and reply layouts (NOT
//! device traffic), as the Python tests script them: `ScriptedTracker` and `StallingTracker`
//! of tests/test_streams.py and `StreamingDevice` of tests/test_supervisor.py. Like those,
//! every read returns at most one message.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tobii_device::channel::{Channel, ChannelError};
use tobii_device::clock::HostClock;
use tobii_linux::tracker::HostTime;
use tobii_protocol::Message;
use tobii_protocol::camera_info::{CameraRecord, HardwareInfo, hardware_info_response};
use tobii_protocol::commands::{RESPONSE_KIND, STREAM_DATA_KIND, Stream, blob_field, command, pause_device};
use tobii_protocol::fields::{encode_payload, int32_field, uint32_field};
use tobii_protocol::gaze::PLATMOD_PRESENT;
use tobii_protocol::gaze::build::{columns_payload, int32_column, int64_column, uint32_column};
use tobii_protocol::ttp::{Decoder, Field};

const SALT: &[u8] = b"0123456789abcdef";
const REFUSED_STATUS: u32 = 3;
pub const STREAMING_START_US: i64 = 900_000_000_000;
pub const STREAMING_PERIOD_US: i64 = 30_207;
const UNPLUGGED: &str = "LIBUSB_ERROR_NO_DEVICE";

/// The gaze stream payloads tools/linux_stream_capture.py recorded from a live tracker.
pub fn payload(name: &str) -> Vec<u8> {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("../../../../tests/data/gaze-payloads.json")).unwrap();
    let text = recorded["payloads"][name].as_str().unwrap();
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

pub fn stream_message(stream: Stream, payload: Vec<u8>) -> Message {
    Message::new(STREAM_DATA_KIND, 0, 0, stream.wire_id(), 0, payload)
}

pub fn gaze_message(payload: Vec<u8>) -> Message {
    stream_message(Stream::Gaze, payload)
}

/// A gaze message with only the given columns.
pub fn gaze_columns_message(columns: &[(u32, Vec<Field>)]) -> Message {
    gaze_message(columns_payload(columns).unwrap())
}

/// A 4x3 image of pixels 0..12; `image_columns()` of tests/test_frames.py at `timestamp`.
pub fn image_message_at(timestamp: i64) -> Message {
    let columns = [
        (1, int64_column(timestamp)),
        (2, uint32_column(8)),
        (3, uint32_column(4)),
        (4, uint32_column(3)),
        (5, vec![blob_field(&(0..12).collect::<Vec<u8>>())]),
    ];
    stream_message(Stream::Image, columns_payload(&columns).unwrap())
}

pub const IMAGE_TIMESTAMP: i64 = 123_456_789;

pub fn image_message() -> Message {
    image_message_at(IMAGE_TIMESTAMP)
}

pub fn presence_message(timestamp: i64, value: i32) -> Message {
    stream_message(
        Stream::Presence,
        columns_payload(&[(1, int64_column(timestamp)), (2, int32_column(value))]).unwrap(),
    )
}

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

fn reply(request: &Message, status: u32, items: &[Field]) -> Message {
    let payload = if items.is_empty() { Vec::new() } else { encode_payload(items).unwrap() };
    Message::new(RESPONSE_KIND, request.request_id, 0, request.command, status, payload)
}

/// The wire id a subscribe or unsubscribe request names.
pub fn requested_stream(request: &Message) -> u32 {
    u32::from_be_bytes(request.payload[7..11].try_into().unwrap())
}

/// What the tracker does once its streams are subscribed.
#[derive(Debug, Clone)]
pub enum Behaviour {
    /// Sends `script` once `after` subscriptions have been answered.
    Scripted { script: Vec<Message>, after: usize },
    /// Streams an image every 10 ms, and one present record when presence is subscribed.
    Streaming,
    /// Streams one message per read, images and gaze in turn, once `after` subscriptions
    /// have been answered; goes silent for good after an unread gap longer than `stall`.
    Stalling { stall: Duration, after: usize },
}

#[derive(Debug)]
struct Device {
    behaviour: Behaviour,
    decoder: Decoder,
    outgoing: VecDeque<Vec<u8>>,
    log: Vec<Message>,
    refuse: Vec<u32>,
    failing_request: Option<usize>,
    subscriptions: usize,
    streaming: bool,
    timestamp: i64,
    unplugged: bool,
    stalled: bool,
    last_read: Option<Instant>,
    sent: u64,
    trailing: Option<(u32, Vec<Message>)>,
    slow: Option<(u32, Duration)>,
    delayed: Option<(Instant, Vec<u8>)>,
    pausable: bool,
    paused: bool,
    pause_status: u32,
}

/// A scripted tracker behind a session; clones share the device.
#[derive(Debug, Clone)]
pub struct FakeTracker(Arc<Mutex<Device>>);

impl FakeTracker {
    pub fn new(behaviour: Behaviour) -> Self {
        Self(Arc::new(Mutex::new(Device {
            behaviour,
            decoder: Decoder::default(),
            outgoing: VecDeque::new(),
            log: Vec::new(),
            refuse: Vec::new(),
            failing_request: None,
            subscriptions: 0,
            streaming: false,
            timestamp: STREAMING_START_US,
            unplugged: false,
            stalled: false,
            last_read: None,
            sent: 0,
            trailing: None,
            slow: None,
            delayed: None,
            pausable: false,
            paused: false,
            pause_status: 0,
        })))
    }

    pub fn scripted(script: Vec<Message>, after: usize) -> Self {
        Self::new(Behaviour::Scripted { script, after })
    }

    pub fn streaming() -> Self {
        Self::new(Behaviour::Streaming)
    }

    /// Streams like [`Self::streaming`], but sends no stream data while 3100 has paused it.
    pub fn pausable() -> Self {
        let tracker = Self::streaming();
        tracker.device().pausable = true;
        tracker
    }

    pub fn stalling(after: usize) -> Self {
        Self::new(Behaviour::Stalling { stall: Duration::from_millis(300), after })
    }

    fn device(&self) -> MutexGuard<'_, Device> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Answers subscriptions of these streams with a failure status.
    pub fn refusing(self, streams: &[Stream]) -> Self {
        self.device().refuse = streams.iter().map(|stream| stream.wire_id()).collect();
        self
    }

    /// Answers the request at this position of the log with a failure status, if it is an
    /// unsubscription.
    pub fn failing_unsubscribe(self, position: usize) -> Self {
        self.device().failing_request = Some(position);
        self
    }

    /// Sends `stream` in the same read as the reply to the next `command` request.
    pub fn trailing(self, command: u32, stream: Vec<Message>) -> Self {
        self.device().trailing = Some((command, stream));
        self
    }

    /// Answers every pause or resume request with this raw status and ignores it.
    pub fn refusing_pause(self, status: u32) -> Self {
        self.device().pause_status = status;
        self
    }

    /// Holds every reply to `command` back for `delay`.
    pub fn slow_reply(self, command: u32, delay: Duration) -> Self {
        self.device().slow = Some((command, delay));
        self
    }

    pub fn log(&self) -> Vec<Message> {
        self.device().log.clone()
    }

    pub fn commands(&self) -> Vec<u32> {
        self.device().log.iter().map(|m| m.command).collect()
    }

    pub fn unplug(&self) {
        self.device().unplugged = true;
    }

    pub fn stalled(&self) -> bool {
        self.device().stalled
    }
}

impl Device {
    fn answer(&mut self, request: Message) {
        self.log.push(request.clone());
        let status = |refused: bool| if refused { REFUSED_STATUS } else { 0 };
        let answer = match request.command {
            command::UNLOCK_PPU => reply(&request, 0, &[int32_field(1), int32_field(77), int32_field(0)]),
            command::OPEN_REALM_SALT => reply(&request, 0, &[uint32_field(5), uint32_field(6), blob_field(SALT)]),
            0x848 => {
                let info = HardwareInfo { cameras_0x8: vec![camera()], points_0xbb0: vec![], int32_0xf70: 5 };
                hardware_info_response(&info, request.request_id, (0, 0)).unwrap()
            }
            command::SUBSCRIBE_STREAM => {
                reply(&request, status(self.refuse.contains(&requested_stream(&request))), &[])
            }
            command::UNSUBSCRIBE_STREAM => reply(&request, status(self.failing_request == Some(self.log.len())), &[]),
            command::PAUSE_DEVICE if self.pause_status != 0 => reply(&request, self.pause_status, &[]),
            command::PAUSE_DEVICE if self.pausable => {
                self.paused = request.payload == pause_device(0).payload;
                reply(&request, 0, &[])
            }
            _ => reply(&request, 0, &[]),
        };
        let mut bytes = answer.encode().unwrap();
        if let Some((_, stream)) = self.trailing.take_if(|(command, _)| *command == request.command) {
            stream.iter().for_each(|message| bytes.extend(message.encode().unwrap()));
        }
        if let Some((_, delay)) = self.slow.filter(|(command, _)| *command == request.command) {
            self.delayed = Some((Instant::now() + delay, bytes));
            return;
        }
        self.outgoing.push_back(bytes);
        if request.command == command::SUBSCRIBE_STREAM {
            self.subscribed(&request);
        }
    }

    fn subscribed(&mut self, request: &Message) {
        self.subscriptions += 1;
        match &self.behaviour {
            Behaviour::Scripted { script, after } if self.subscriptions == *after => {
                let script: Vec<Vec<u8>> = script.iter().map(|message| message.encode().unwrap()).collect();
                self.outgoing.extend(script);
            }
            Behaviour::Streaming => {
                self.streaming = true;
                if requested_stream(request) == Stream::Presence.wire_id() && !self.paused {
                    let present = presence_message(self.timestamp, PLATMOD_PRESENT);
                    self.outgoing.push_back(present.encode().unwrap());
                }
            }
            Behaviour::Stalling { after, .. } if self.subscriptions >= *after => self.streaming = true,
            _ => {}
        }
    }

    fn next_stream_message(&mut self) -> Option<Message> {
        match self.behaviour {
            Behaviour::Streaming => {
                self.timestamp += STREAMING_PERIOD_US;
                Some(image_message_at(self.timestamp))
            }
            Behaviour::Stalling { .. } => {
                self.sent += 1;
                Some(if self.sent % 2 == 1 { image_message() } else { gaze_message(payload("both")) })
            }
            Behaviour::Scripted { .. } => None,
        }
    }

    fn stream_pause(&self) -> Duration {
        match self.behaviour {
            Behaviour::Streaming => Duration::from_millis(10),
            _ => Duration::from_millis(5),
        }
    }
}

fn idle(timeout_ms: u32) {
    std::thread::sleep(Duration::from_millis(u64::from(timeout_ms.min(20))));
}

impl Channel for FakeTracker {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        let mut device = self.device();
        if device.unplugged {
            return Err(ChannelError::new(true, UNPLUGGED));
        }
        if device.stalled {
            return Ok(());
        }
        for request in device.decoder.feed(data).unwrap() {
            device.answer(request);
        }
        Ok(())
    }

    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        let now = Instant::now();
        let mut device = self.device();
        if device.unplugged {
            return Err(ChannelError::new(true, UNPLUGGED));
        }
        if let Behaviour::Stalling { stall, .. } = device.behaviour
            && device.streaming
            && device.last_read.is_some_and(|last| now - last > stall)
        {
            device.stalled = true;
        }
        device.last_read = Some(now);
        if device.stalled {
            drop(device);
            idle(timeout_ms);
            return Ok(Vec::new());
        }
        if let Some((_, bytes)) = device.delayed.take_if(|(at, _)| *at <= now) {
            return Ok(bytes);
        }
        if let Some(message) = device.outgoing.pop_front() {
            return Ok(message);
        }
        if device.streaming
            && !device.paused
            && let Some(message) = device.next_stream_message()
        {
            let pause = device.stream_pause();
            drop(device);
            std::thread::sleep(pause);
            return Ok(message.encode().unwrap());
        }
        drop(device);
        idle(timeout_ms.min(1));
        Ok(Vec::new())
    }
}

/// Host time that starts 5 s after the recorded device time and advances 1 ms per reading.
#[derive(Debug, Default)]
pub struct TickingClock(AtomicI64);

pub const RECORDED_DEVICE_US: i64 = 898_743_954_739;

impl HostClock for TickingClock {
    fn now_us(&self) -> i64 {
        RECORDED_DEVICE_US + 5_000_000 + self.0.fetch_add(1, Ordering::SeqCst) * 1000
    }
}

pub fn ticking_clock() -> HostTime {
    HostTime(Arc::new(TickingClock::default()))
}
