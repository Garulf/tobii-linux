//! Live stream sessions against scripted trackers: the checks of tests/test_streams.py
//! (GazeAndImageStreamsTests, SlowConsumerTests, SuspendTests and the StreamReaderTests
//! tobii-device does not already cover), on LiveStreams, head_pose_samples and drained_items.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::fake_tracker::*;
use tobii_device::streams::{
    DeviceClockWatch, HostSuspendWatch, SUSPEND_GAP_S, StreamReader, StreamValue, SuspendWatch,
};
use tobii_device::{CameraCalibration, DeviceError, StopFlag, TrackerSession};
use tobii_headpose::pipeline::HeadPoseSample;
use tobii_linux::error::{Error, Result};
use tobii_linux::tracker::{DeviceRequest, FrameProcessor, LiveStreams, SharedStats, drained_items, head_pose_samples};
use tobii_protocol::Message;
use tobii_protocol::commands::{Stream, command, header_only_request, subscribe_stream, unsubscribe_stream};
use tobii_protocol::frames::ImageFrame;
use tobii_protocol::gaze::build::int64_column;
use tobii_protocol::gaze::{GazeFrame, PLATMOD_PRESENT, PLATMOD_UNKNOWN, Presence};

const SUBSCRIBE: u32 = command::SUBSCRIBE_STREAM;
const UNSUBSCRIBE: u32 = command::UNSUBSCRIBE_STREAM;

/// The recorded payloads and the scripted images do not share a clock.
fn any_order() -> DeviceClockWatch {
    DeviceClockWatch { rewind_us: 1 << 62, latest: None }
}

fn unstarted(device: &FakeTracker) -> TrackerSession<FakeTracker> {
    TrackerSession::new(device.clone())
}

fn ignore_gaze(_: GazeFrame) {}

fn ignore_presence(_: i64, _: i64) {}

#[test]
fn every_gaze_frame_is_published_on_the_host_clock() {
    let script = vec![
        gaze_message(payload("both")),
        image_message(),
        gaze_message(payload("left")),
        presence_message(RECORDED_DEVICE_US, 2),
        gaze_message(payload("none")),
        image_message(),
    ];
    let device = FakeTracker::scripted(script, 3);
    let (published, presences) = (Mutex::new(Vec::new()), Mutex::new(Vec::new()));
    let gaze = |frame: GazeFrame| published.lock().unwrap().push(frame);
    let presence = |time_us: i64, value: i64| presences.lock().unwrap().push((time_us, value));
    let mut live = LiveStreams::new(ticking_clock(), &gaze);
    live.presence = Some(&presence);
    live.silence = Duration::from_millis(200);
    live.device_clock = any_order();
    let (mut frames, mut mapped) = (Vec::new(), None);
    let outcome = live.run(
        &mut unstarted(&device),
        &StopFlag::new(),
        |_| {},
        |frame, to_host| {
            mapped.get_or_insert_with(|| to_host(frame.timestamp));
            frames.push(frame);
            Ok(())
        },
    );
    assert!(matches!(outcome, Err(Error::Device(DeviceError::Silent { .. }))), "{outcome:?}");
    let published = published.into_inner().unwrap();
    assert_eq!(published.iter().map(|f| f.gaze_point.valid).collect::<Vec<_>>(), [1, 1, 0]);
    for (frame, device_us) in published.iter().zip([898_743_954_739, 898_734_316_869, 898_734_226_487]) {
        assert!(frame.timestamp_us() > device_us + 4_000_000);
        assert_eq!(frame.gaze_point.timestamp, frame.timestamp_us());
    }
    let values: Vec<i64> = presences.into_inner().unwrap().into_iter().map(|(_, value)| value).collect();
    assert_eq!(values, [i64::from(PLATMOD_UNKNOWN), i64::from(PLATMOD_PRESENT)]);
    assert!(mapped.unwrap() > frames[0].timestamp);
    assert!(frames.len() <= 2);
    assert_eq!(device.commands(), [SUBSCRIBE; 3], "a silent session is not asked to unsubscribe");
}

#[test]
fn device_stream_messages_are_forwarded_with_their_host_time() {
    let presence = presence_message(RECORDED_DEVICE_US, 2);
    let device = FakeTracker::scripted(vec![gaze_message(payload("both")), presence.clone(), image_message()], 3);
    let (published, forwarded) = (Mutex::new(Vec::new()), Mutex::new(Vec::new()));
    let gaze = |frame: GazeFrame| published.lock().unwrap().push(frame);
    let device_stream = |message: &Message, time_us: i64| forwarded.lock().unwrap().push((message.clone(), time_us));
    let mut live = LiveStreams::new(ticking_clock(), &gaze);
    live.presence = Some(&ignore_presence);
    live.device_stream = Some(&device_stream);
    live.silence = Duration::from_millis(200);
    live.device_clock = any_order();
    let stop = StopFlag::new();
    let mut wire_streams = Vec::new();
    live.run(
        &mut unstarted(&device),
        &stop,
        |reader| wire_streams = reader.subscribed.iter().map(|stream| stream.wire_id()).collect(),
        |_, _| {
            stop.set();
            Ok(())
        },
    )
    .unwrap();
    let forwarded = forwarded.into_inner().unwrap();
    let payloads: Vec<&[u8]> = forwarded.iter().map(|(message, _)| message.payload.as_slice()).collect();
    assert_eq!(payloads, [payload("both").as_slice(), presence.payload.as_slice()]);
    assert_eq!(forwarded[0].1, published.into_inner().unwrap()[0].timestamp_us());
    wire_streams.sort_unstable();
    assert_eq!(wire_streams, [0x500, 0x501, 0x504]);
}

#[test]
fn stopping_unsubscribes_every_stream() {
    let device = FakeTracker::scripted(vec![image_message(); 3], 3);
    let mut live = LiveStreams::new(ticking_clock(), &ignore_gaze);
    live.presence = Some(&ignore_presence);
    let stop = StopFlag::new();
    live.run(
        &mut unstarted(&device),
        &stop,
        |_| {},
        |_, _| {
            stop.set();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(device.commands(), [SUBSCRIBE, SUBSCRIBE, SUBSCRIBE, UNSUBSCRIBE, UNSUBSCRIBE, UNSUBSCRIBE]);
}

#[test]
fn device_clock_going_back_ends_the_session() {
    let gaze_at =
        |timestamp| tobii_protocol::gaze::build::stream_message(Stream::Gaze, &[(1, int64_column(timestamp))]);
    let device = FakeTracker::scripted(vec![gaze_at(900_000_000_000).unwrap(), gaze_at(800_000_000_000).unwrap()], 2);
    let mut session = unstarted(&device);
    let outcome =
        LiveStreams::new(ticking_clock(), &ignore_gaze).run(&mut session, &StopFlag::new(), |_| {}, |_, _| Ok(()));
    let error = outcome.unwrap_err();
    assert!(error.to_string().contains("restarted"), "{error}");
    assert!(session.is_dead());
    assert_eq!(device.commands(), [SUBSCRIBE, SUBSCRIBE]);
}

#[test]
fn head_pose_and_gaze_yields_samples_on_the_host_clock() {
    let device = FakeTracker::scripted(vec![image_message(), gaze_message(payload("both"))], 2);
    let published = Mutex::new(Vec::new());
    let gaze = |frame: GazeFrame| published.lock().unwrap().push(frame);
    let stop = StopFlag::new();
    let mut samples = Vec::new();
    LiveStreams::new(ticking_clock(), &gaze)
        .run(
            &mut unstarted(&device),
            &stop,
            |_| {},
            |frame, to_host| {
                let sample = HeadPoseSample {
                    timestamp_us: frame.timestamp,
                    valid: true,
                    position_mm: [1.0, 2.0, 3.0],
                    rotation_rad: [0.0; 3],
                };
                samples.push(HeadPoseSample { timestamp_us: to_host(sample.timestamp_us), ..sample });
                stop.set();
                Ok(())
            },
        )
        .unwrap();
    assert!(samples[0].timestamp_us > IMAGE_TIMESTAMP);
    assert_eq!(device.commands()[device.commands().len() - 2..], [UNSUBSCRIBE, UNSUBSCRIBE]);
}

#[test]
fn live_streams_keep_reading_under_slow_processing() {
    let device = FakeTracker::stalling(2);
    let gaze_frames = Mutex::new(0usize);
    let gaze = |_: GazeFrame| *gaze_frames.lock().unwrap() += 1;
    let mut live = LiveStreams::new(ticking_clock(), &gaze);
    live.device_clock = any_order();
    let stop = StopFlag::new();
    let mut frames = 0;
    live.run(
        &mut unstarted(&device),
        &stop,
        |_| {},
        |_, _| {
            frames += 1;
            if frames < 3 {
                std::thread::sleep(Duration::from_millis(500));
            } else {
                stop.set();
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(!device.stalled());
    assert!(*gaze_frames.lock().unwrap() > 20);
}

/// Takes half a second per frame.
struct SlowProcessor;

impl FrameProcessor for SlowProcessor {
    fn start(&mut self, _: &CameraCalibration) {}

    fn process(&mut self, frame: &ImageFrame) -> Result<HeadPoseSample> {
        std::thread::sleep(Duration::from_millis(500));
        Ok(HeadPoseSample { timestamp_us: frame.timestamp, valid: true, position_mm: [0.0; 3], rotation_rad: [0.0; 3] })
    }
}

#[test]
fn head_pose_samples_keep_reading_under_slow_processing() {
    let device = FakeTracker::stalling(1);
    let mut samples = 0;
    let outcome = head_pose_samples(&mut unstarted(&device), &mut SlowProcessor, |_| {
        samples += 1;
        if samples == 3 { Err(Error::Runtime("three samples are enough".into())) } else { Ok(()) }
    });
    assert!(matches!(outcome, Err(Error::Runtime(_))), "{outcome:?}");
    assert_eq!(samples, 3);
    assert!(!device.stalled());
}

#[test]
fn drained_items_survive_a_slow_consumer() {
    let device = FakeTracker::stalling(2);
    let mut session = unstarted(&device);
    let mut reader = StreamReader::new(&[Stream::Gaze, Stream::Image], &[]);
    reader.subscribe(&mut session).unwrap();
    let stop = StopFlag::new();
    let (mut taken, mut remaining) = (0, 0);
    drained_items(&mut reader, &mut session, None, &stop, &SharedStats::default(), 256, |_| {
        taken += 1;
        if taken > 3 {
            remaining += 1;
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
        if taken == 3 {
            stop.set();
        }
    })
    .unwrap();
    assert!(!device.stalled());
    assert!(remaining > 20, "{remaining}");
    let _ = reader.unsubscribe(&mut session);
}

/// Seconds suspended since boot, as a sequence of readings.
fn suspended_times(readings: impl IntoIterator<Item = f64, IntoIter: Send + 'static>) -> impl FnMut() -> f64 + Send {
    let mut readings = readings.into_iter();
    move || readings.next().unwrap()
}

#[test]
fn suspend_watch_notices_boottime_running_ahead() {
    let mut watch = HostSuspendWatch::new(suspended_times([10.0, 10.1, 10.2, 75.0, 75.1]), SUSPEND_GAP_S);
    let resumed: Vec<bool> = (0..4).map(|_| watch.resumed()).collect();
    assert_eq!(resumed, [false, false, true, false]);
}

#[test]
fn resume_ends_the_session() {
    let device = FakeTracker::scripted(vec![image_message(); 50], 2);
    let mut session = unstarted(&device);
    let mut reader = StreamReader::new(&[Stream::Gaze, Stream::Image], &[]);
    reader.subscribe(&mut session).unwrap();
    let readings = std::iter::repeat_n(0.0, 4).chain(std::iter::repeat(60.0));
    let watch = Box::new(HostSuspendWatch::new(suspended_times(readings), SUSPEND_GAP_S));
    let outcome = drained_items(&mut reader, &mut session, Some(watch), &StopFlag::new(), &Arc::default(), 256, drop);
    let error = outcome.unwrap_err();
    assert!(error.to_string().contains("suspended"), "{error}");
    assert!(session.is_dead());
}

#[test]
fn presence_is_decoded_and_optional() {
    let streams = [Stream::Gaze, Stream::Image, Stream::Presence];
    let device = FakeTracker::scripted(vec![presence_message(77, 2)], 3);
    let mut session = unstarted(&device);
    let mut reader = StreamReader::new(&streams, &[Stream::Presence]);
    reader.subscribe(&mut session).unwrap();
    let stop = StopFlag::new();
    let item = reader.items(&mut session, Duration::from_secs(1), None, &stop).next().unwrap().unwrap();
    assert_eq!(item.value, StreamValue::Presence(Presence { timestamp_us: 77, value: 2 }));

    let refusing = FakeTracker::scripted(Vec::new(), 3).refusing(&[Stream::Presence]);
    let mut reader = StreamReader::new(&streams, &[Stream::Presence]);
    reader.subscribe(&mut unstarted(&refusing)).unwrap();
    assert_eq!(reader.subscribed, [Stream::Gaze, Stream::Image]);
    assert!(reader.refused.iter().any(|(stream, _)| *stream == Stream::Presence));
}

#[test]
fn required_streams_must_subscribe() {
    let device = FakeTracker::scripted(Vec::new(), 2).refusing(&[Stream::Image]);
    let mut reader = StreamReader::new(&[Stream::Gaze, Stream::Image], &[]);
    assert!(matches!(reader.subscribe(&mut unstarted(&device)), Err(DeviceError::Failed { .. })));
}

#[test]
fn unsubscribe_reaches_every_stream_after_a_failure() {
    let device = FakeTracker::scripted(Vec::new(), 2).failing_unsubscribe(3);
    let mut session = unstarted(&device);
    let mut reader = StreamReader::new(&[Stream::Gaze, Stream::Image], &[]);
    reader.subscribe(&mut session).unwrap();
    assert!(matches!(reader.unsubscribe(&mut session), Err(DeviceError::Failed { .. })));
    assert_eq!(device.commands(), [SUBSCRIBE, SUBSCRIBE, UNSUBSCRIBE, UNSUBSCRIBE]);
    assert!(reader.subscribed.is_empty());
    let requested: Vec<Vec<u8>> = device.log().into_iter().map(|m| m.payload).collect();
    let expected = |build: fn(u32, u32) -> Vec<u8>| [Stream::Gaze, Stream::Image].map(|s| build(0, s.index()));
    assert_eq!(requested[..2], expected(|id, s| subscribe_stream(id, s, &[]).unwrap().payload));
    let mut unsubscribed = requested[2..].to_vec();
    unsubscribed.sort();
    let mut wanted = expected(|id, s| unsubscribe_stream(id, s).unwrap().payload).to_vec();
    wanted.sort();
    assert_eq!(unsubscribed, wanted);
}

type Replied = tokio::sync::oneshot::Receiver<std::result::Result<Message, DeviceError>>;

/// An inbox holding one device status request (1490), and where its reply arrives.
fn status_request() -> (std::sync::mpsc::Receiver<DeviceRequest>, Replied) {
    let (requests, inbox) = std::sync::mpsc::channel();
    let (reply, replied) = tokio::sync::oneshot::channel();
    let build = Box::new(|id| header_only_request(id, command::GET_DEVICE_STATUS));
    requests.send(DeviceRequest { build, timeout: Duration::from_secs(2), reply }).unwrap();
    (inbox, replied)
}

#[test]
fn stream_data_read_with_a_reply_reaches_the_reader_in_order() {
    let stream = vec![gaze_message(payload("both")), gaze_message(payload("left")), gaze_message(payload("none"))];
    let device = FakeTracker::scripted(Vec::new(), 2).trailing(command::GET_DEVICE_STATUS, stream);
    let published = Mutex::new(Vec::new());
    let gaze = |frame: GazeFrame| published.lock().unwrap().push(frame);
    let (inbox, mut replied) = status_request();
    let mut live = LiveStreams::new(ticking_clock(), &gaze);
    live.silence = Duration::from_millis(300);
    live.device_clock = any_order();
    live.requests = Some(inbox);
    let outcome = live.run(&mut unstarted(&device), &StopFlag::new(), |_| {}, |_, _| Ok(()));
    assert!(matches!(outcome, Err(Error::Device(DeviceError::Silent { .. }))), "{outcome:?}");
    assert_eq!(replied.try_recv().unwrap().unwrap().word4, 0);
    let valid: Vec<u32> = published.into_inner().unwrap().iter().map(|f| f.gaze_point.valid).collect();
    assert_eq!(valid, [1, 1, 0]);
}

#[test]
fn a_reply_slower_than_the_silence_timeout_keeps_the_session_alive() {
    let silence = Duration::from_millis(200);
    let device = FakeTracker::scripted(Vec::new(), 2).slow_reply(command::GET_DEVICE_STATUS, silence * 2);
    let (inbox, replied) = status_request();
    let mut live = LiveStreams::new(ticking_clock(), &ignore_gaze);
    live.silence = silence;
    live.requests = Some(inbox);
    let stop = StopFlag::new();
    let stopper = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let reply = replied.blocking_recv().unwrap();
            std::thread::sleep(Duration::from_millis(50));
            stop.set();
            reply
        })
    };
    let outcome = live.run(&mut unstarted(&device), &stop, |_| {}, |_, _| Ok(()));
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(stopper.join().unwrap().unwrap().word4, 0);
}
