//! The browser demo: the checks of tests/test_demo.py (event encoding the unit tests of
//! src/demo/events.rs leave out, the pipeline with a fake processor, the HTTP server and
//! its event stream over loopback, replay, and the live source across a lost tracker).

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use common::fake_tracker::{FakeTracker, payload};
use serde_json::{Value, json};
use tobii_device::supervisor::Backoff;
use tobii_device::{CameraCalibration, DeviceError, TrackerSession};
use tobii_headpose::pipeline::{DisplayArea, HeadPoseSample};
use tobii_linux::demo::events::{
    PNG_SIGNATURE, Tracked, frame_event, gaze_columns_event, gaze_event, mesh_topology_event, png_grayscale, pose_event,
};
use tobii_linux::demo::pipeline::{
    DemoPipeline, DemoProcessor, LatestSlot, Processed, ProcessorFactory, Publish, device_combined_gaze,
};
use tobii_linux::demo::server::{DemoServer, EventHub, Subscriber};
use tobii_linux::demo::sources::{OpenSession, SourceThread, run_live};
use tobii_linux::error::{Error, Result};
use tobii_linux::pyjson::{self, Json, object};
use tobii_protocol::commands::command;
use tobii_protocol::frames::ImageFrame;
use tobii_protocol::gaze::{GazeFrame, Presence, decode_gaze_message};

const TRACKING: i64 = 2;
const AREA: [[f64; 3]; 3] = [[-296.62, 326.0, 111.82], [300.38, 326.0, 111.82], [-296.62, 10.27, -3.1]];
const WAIT: Duration = Duration::from_secs(5);

fn recorded(name: &str) -> GazeFrame {
    GazeFrame::new(decode_gaze_message(&payload(name)).unwrap().unwrap())
}

/// The payload as the browser receives it.
fn browser(payload: &Json) -> Value {
    serde_json::from_str(&pyjson::compact(payload)).unwrap()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Kind and body of every chunk, checking the signature and each checksum.
fn png_chunks(data: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    assert_eq!(&data[..8], PNG_SIGNATURE);
    let (mut offset, mut chunks) = (8, Vec::new());
    while offset < data.len() {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[offset + 4..offset + 8].try_into().unwrap();
        let body = data[offset + 8..offset + 8 + length].to_vec();
        let crc = u32::from_be_bytes(data[offset + 8 + length..offset + 12 + length].try_into().unwrap());
        assert_eq!(crc, crc32(&data[offset + 4..offset + 8 + length]));
        chunks.push((kind, body));
        offset += 12 + length;
    }
    chunks
}

fn base64_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let digits: Vec<u32> =
        text.bytes().filter(|&b| b != b'=').map(|b| ALPHABET.iter().position(|&a| a == b).unwrap() as u32).collect();
    let mut bytes = Vec::new();
    for chunk in digits.chunks(4) {
        let n = chunk.iter().enumerate().fold(0, |n, (index, digit)| n | digit << (18 - 6 * index));
        bytes.extend(&n.to_be_bytes()[1..chunk.len()]);
    }
    bytes
}

fn int16_values(text: &Value) -> Vec<i16> {
    base64_decode(text.as_str().unwrap()).chunks(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]])).collect()
}

fn tracked(accepted: bool) -> Tracked {
    Tracked {
        face: Some([10, 20, 30, 40]),
        frame_points: Some(vec![[0.25, 0.5], [0.75, 0.125]]),
        quality_score: 0.8125,
        accepted,
    }
}

fn image(timestamp: i64, pixels: Vec<u8>) -> ImageFrame {
    ImageFrame { timestamp, pixel_format: 8, width: 4, height: 3, pixels }
}

#[test]
fn grayscale_png_round_trip() {
    let pixels: Vec<u8> = (0..12).collect();
    let chunks = png_chunks(&png_grayscale(4, 3, &pixels).unwrap());
    assert_eq!(chunks.iter().map(|(kind, _)| kind).collect::<Vec<_>>(), [b"IHDR", b"IDAT", b"IEND"]);
    assert_eq!(chunks[0].1, [0, 0, 0, 4, 0, 0, 0, 3, 8, 0, 0, 0, 0]);
    let rows = miniz_oxide::inflate::decompress_to_vec_zlib(&chunks[1].1).unwrap();
    assert_eq!(rows, [&[0][..], &pixels[0..4], &[0], &pixels[4..8], &[0], &pixels[8..12]].concat());
}

#[test]
fn frame_event_carries_image_and_overlay() {
    let frame = image(5_000_000, (0..12).collect());
    let event = browser(&frame_event(7, &frame, Some(&tracked(true)), Some(TRACKING), None).unwrap());
    assert_eq!(event["state"], "tracking");
    assert_eq!(event["face"], json!([10, 20, 30, 40]));
    assert_eq!(event["landmarks"], json!([0.25, 0.5, 0.75, 0.125]));
    assert_eq!(event["quality"], json!(0.812));
    assert!(!png_chunks(&base64_decode(event["png"].as_str().unwrap())).is_empty());
    let bare = browser(&frame_event(8, &frame, None, None, None).unwrap());
    assert!(bare["state"].is_null());
    assert!(bare["landmarks"].is_null());
}

fn sample(timestamp_us: i64, valid: bool, position_mm: [f32; 3], rotation_rad: [f32; 3]) -> HeadPoseSample {
    HeadPoseSample { timestamp_us, valid, position_mm, rotation_rad }
}

#[test]
fn pose_event_in_degrees() {
    let pose = sample(1_000, true, [1.234, -5.0, 620.04], [0.1, -0.2, 0.0]);
    let event = browser(&pose_event(&pose, &tracked(true), Some(TRACKING), 9.87, None));
    assert_eq!(event["position"], json!([1.2, -5.0, 620.0]));
    assert_eq!(event["rotation"], json!([5.73, -11.46, 0.0]));
    assert_eq!(event["ms"], json!(9.9));
    let invalid =
        browser(&pose_event(&sample(1, false, [0.0; 3], [0.0; 3]), &tracked(false), Some(TRACKING), 1.0, None));
    assert!(invalid["position"].is_null());
    assert_eq!(invalid["accepted"], false);
}

#[test]
fn gaze_event_from_a_device_record() {
    let record = recorded("left");
    let event = gaze_event(&record, device_combined_gaze(&record), "device");
    let text = pyjson::compact(&event);
    let event = browser(&event);
    assert_eq!(event["point"], json!([0.6322, 0.0999]));
    assert_eq!(event["left"]["valid"], true);
    assert_eq!(event["left"]["pupil"], json!(4.52));
    assert_eq!(event["right"]["valid"], false);
    assert!(event["right"]["point"].is_null());
    assert!(!text.contains(' '));
    let missing = recorded("none");
    assert_eq!(device_combined_gaze(&missing), None);
    assert!(browser(&gaze_event(&missing, None, "device"))["point"].is_null());
}

#[test]
fn gaze_columns_event_lists_every_column() {
    let event = browser(&gaze_columns_event(&recorded("both")));
    let columns = event["columns"].as_array().unwrap();
    assert_eq!(columns.len(), 35);
    assert_eq!(columns[0], json!([1, "timestamp", 898_743_954_739_i64]));
    assert_eq!(columns[34].as_array().unwrap()[..2], [json!(44), json!("right_openness_validity")]);
}

/// Every published event, for tests to wait on.
#[derive(Default)]
struct Collector {
    events: Mutex<Vec<(String, Value)>>,
    changed: Condvar,
}

impl Collector {
    fn publish(self: &Arc<Self>) -> Publish {
        let collector = Arc::clone(self);
        Arc::new(move |topic: &str, payload: Json, _retain: bool| {
            collector.events.lock().unwrap().push((topic.to_string(), browser(&payload)));
            collector.changed.notify_all();
        })
    }

    fn wait_for(&self, topic: &str) -> Vec<Value> {
        let events = self.events.lock().unwrap();
        let (events, _) = self.changed.wait_timeout_while(events, WAIT, |e| e.iter().all(|(t, _)| t != topic)).unwrap();
        events.iter().filter(|(t, _)| t == topic).map(|(_, payload)| payload.clone()).collect()
    }
}

/// Tracks every frame; the wireframe has three points.
#[derive(Default)]
struct FakeProcessor {
    calibrations: Vec<CameraCalibration>,
}

impl DemoProcessor for FakeProcessor {
    fn calibrate(&mut self, calibration: &CameraCalibration) {
        self.calibrations.push(*calibration);
    }

    fn calibrated(&self) -> bool {
        !self.calibrations.is_empty()
    }

    fn topology(&self) -> Json {
        mesh_topology_event(&[(0, 1), (1, 2)], 3, &[[0.0; 3]; 4], [0.0, -200.0, 30.0])
    }

    fn process(&mut self, frame: &ImageFrame) -> Result<Processed> {
        Ok(Processed {
            tracked: tracked(true),
            sample: sample(frame.timestamp, true, [1.0, 2.0, 600.0], [0.0; 3]),
            mesh_pixels: Some(vec![10.0, 20.0, 30.5, 40.0, 50.0, 60.0]),
            mesh_millimetres: Some([1.0, 2.0, 600.0].repeat(3)),
        })
    }

    fn tracker_state(&self) -> i64 {
        TRACKING
    }
}

fn fake_processors() -> ProcessorFactory {
    Box::new(|_: &Path, _: DisplayArea| Ok(Box::new(FakeProcessor::default()) as Box<dyn DemoProcessor>))
}

fn pipeline(
    published: &Arc<Collector>,
    image_rate: f64,
    head_pose: bool,
    factory: ProcessorFactory,
) -> Arc<DemoPipeline> {
    DemoPipeline::new(published.publish(), AREA, PathBuf::new(), image_rate, "live", head_pose, factory)
}

fn without_head_pose(published: &Arc<Collector>) -> Arc<DemoPipeline> {
    pipeline(published, 15.0, false, fake_processors())
}

fn status(pipeline: &DemoPipeline) -> Value {
    browser(&pipeline.status())
}

#[test]
fn frames_are_processed_and_published_with_their_overlay() {
    let published = Arc::new(Collector::default());
    let pipeline = pipeline(&published, 1000.0, true, fake_processors());
    let calibration = CameraCalibration { translation: [0.0, 0.0, 4.14], angles: [0.0; 3] };
    pipeline.on_session(Some(calibration), vec![("model".into(), "test".into())], Vec::new());
    pipeline.start();
    pipeline.on_frame(image(2_000_000, vec![0; 12]));
    let pose = published.wait_for("pose")[0].clone();
    let frame = published.wait_for("frame")[0].clone();
    pipeline.stop(WAIT);
    assert_eq!(pose["position"], json!([1.0, 2.0, 600.0]));
    assert_eq!(int16_values(&pose["mesh3d"]), [10, 20, 6000].repeat(3));
    assert_eq!(int16_values(&frame["mesh"]), [100, 200, 305, 400, 500, 600]);
    assert_eq!(published.wait_for("mesh_topology")[0]["edges"], json!([0, 1, 1, 2]));
    assert_eq!(pose["state"], "tracking");
    assert_eq!(frame["face"], json!([10, 20, 30, 40]));
    let status = status(&pipeline);
    assert_eq!(status["head_pose"]["state"], "running");
    assert_eq!(status["counters"]["processed"], 1);
    assert_eq!(status["device"], json!({"model": "test"}));
    assert_eq!(status["display_area"]["width_mm"], json!(597.0));
}

#[test]
fn gaze_is_published_through_the_selected_source() {
    let published = Arc::new(Collector::default());
    let pipeline = without_head_pose(&published);
    pipeline.on_gaze(&recorded("both"));
    pipeline.on_gaze(&recorded("none"));
    pipeline.on_presence(Presence { timestamp_us: 1, value: 2 });
    let gaze = published.wait_for("gaze");
    assert_eq!(gaze[0]["point"], json!([0.4717, 1.1246]));
    assert!(gaze[1]["point"].is_null());
    assert_eq!(published.wait_for("gaze_columns").len(), 1);
    let status = status(&pipeline);
    assert_eq!(status["counters"]["gaze_valid"], 1);
    assert_eq!(
        status["presence"],
        json!({"value": 2, "state": "present", "property_value": 3, "since_us": 1, "notifications": 0})
    );
}

#[test]
fn presence_is_unknown_until_the_device_reports() {
    let pipeline = without_head_pose(&Arc::default());
    pipeline.on_presence_subscribed();
    assert_eq!(status(&pipeline)["presence"]["state"], "unknown");
    pipeline.on_presence(Presence { timestamp_us: 7, value: 1 });
    let presence = &status(&pipeline)["presence"];
    assert_eq!(
        (&presence["state"], &presence["property_value"], &presence["notifications"]),
        (&json!("away"), &json!(2), &json!(1))
    );
}

/// Warnings of every thread; the pipeline warns from its processing thread.
static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct WarningLog;

impl log::Log for WarningLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            WARNINGS.lock().unwrap().push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

fn record_warnings() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        log::set_logger(&WarningLog).unwrap();
        log::set_max_level(log::LevelFilter::Warn);
    });
}

#[test]
fn missing_models_leave_camera_images_running() {
    record_warnings();
    let broken: ProcessorFactory = Box::new(|_: &Path, _: DisplayArea| {
        Err(Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "no models here")))
    });
    let published = Arc::new(Collector::default());
    let pipeline = pipeline(&published, 1000.0, true, broken);
    pipeline.start();
    pipeline.on_frame(image(1, vec![0; 12]));
    let frame = published.wait_for("frame")[0].clone();
    pipeline.stop(WAIT);
    assert!(frame["state"].is_null());
    let status = status(&pipeline);
    assert_eq!(status["head_pose"]["state"], "unavailable");
    assert!(status["errors"][0]["message"].as_str().unwrap().contains("no models here"));
    assert!(WARNINGS.lock().unwrap().iter().any(|warning| warning.contains("no models here")));
}

#[test]
fn latest_slot_drops_frames_that_were_never_taken() {
    let slot = LatestSlot::default();
    for item in [1, 2, 3] {
        slot.put(item);
    }
    assert_eq!(slot.take(Duration::from_millis(10)), Some(3));
    assert_eq!(slot.dropped(), 2);
    assert_eq!(slot.take(Duration::from_millis(10)), None);
}

struct Response {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

fn fetch(port: u16, path: &str) -> Response {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(response[..end].to_vec()).unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let content_type =
        head.lines().find_map(|line| line.strip_prefix("Content-Type: ")).unwrap_or_default().to_string();
    Response { status, content_type, body: response[end + 4..].to_vec() }
}

async fn fetch_async(port: u16, path: &'static str) -> Response {
    tokio::task::spawn_blocking(move || fetch(port, path)).await.unwrap()
}

type Received = BTreeMap<String, Vec<Value>>;

/// Topics and payloads from the event stream until every wanted topic has arrived; each
/// event also goes to `arrived` as it comes.
fn read_events(port: u16, wanted: &[&str], arrived: tokio::sync::mpsc::UnboundedSender<String>) -> Received {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(stream, "GET /events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
    let mut lines = BufReader::new(stream).lines().map(Result::unwrap);
    assert_eq!(lines.next().unwrap(), "HTTP/1.1 200 OK");
    let headers: Vec<String> = lines.by_ref().take_while(|line| !line.is_empty()).collect();
    assert!(headers.iter().any(|header| header == "Content-Type: text/event-stream"));
    let (mut received, mut topic) = (Received::new(), None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !wanted.iter().all(|topic| received.contains_key(*topic)) && Instant::now() < deadline {
        let line = lines.next().unwrap();
        if let Some(name) = line.strip_prefix("event: ") {
            topic = Some(name.to_string());
        } else if let (Some(data), Some(topic)) = (line.strip_prefix("data: "), &topic) {
            received.entry(topic.clone()).or_default().push(serde_json::from_str(data).unwrap());
            let _ = arrived.send(topic.clone());
        }
    }
    received
}

#[test]
fn page_status_and_event_stream() {
    common::run(async {
        let hub = Rc::new(EventHub::default());
        let server = DemoServer::start(Rc::clone(&hub), "127.0.0.1", 0).await.unwrap();
        let port = server.port;
        let page = fetch_async(port, "/").await;
        let script = fetch_async(port, "/static/demo.js").await;
        let empty_status = fetch_async(port, "/api/status").await;
        hub.publish("status", object([("connection", "streaming".into())]), true);
        let status = fetch_async(port, "/api/status").await;
        let missing = fetch_async(port, "/static/../app.py").await;
        let (arrived, mut arrivals) = tokio::sync::mpsc::unbounded_channel();
        let reading = tokio::task::spawn_blocking(move || read_events(port, &["status", "gaze"], arrived));
        assert_eq!(arrivals.recv().await.as_deref(), Some("status"));
        hub.publish("gaze", object([("point", Json::List(vec![Json::Float(0.5), Json::Float(0.5)]))]), false);
        let received = reading.await.unwrap();
        server.close().await;

        assert_eq!(page.status, 200);
        assert!(page.content_type.starts_with("text/html"));
        assert!(page.body.windows(15).any(|w| w == b"/static/demo.js"));
        assert!(script.content_type.starts_with("text/javascript"));
        assert!(script.body.windows(11).any(|w| w == b"EventSource"));
        assert_eq!(serde_json::from_slice::<Value>(&empty_status.body).unwrap(), json!({"connection": "starting"}));
        assert_eq!(serde_json::from_slice::<Value>(&status.body).unwrap(), json!({"connection": "streaming"}));
        assert_eq!(missing.status, 404);
        assert_eq!(received["status"], [json!({"connection": "streaming"})]);
        assert_eq!(received["gaze"], [json!({"point": [0.5, 0.5]})]);
    });
}

#[test]
fn slow_browsers_get_the_newest_image_and_a_bounded_backlog() {
    let subscriber = Subscriber::new(3);
    for index in 0..5 {
        subscriber.push("gaze", Rc::new(format!("g{index}").into_bytes()));
        subscriber.push("frame", Rc::new(format!("f{index}").into_bytes()));
    }
    assert_eq!(subscriber.take(), b"g2g3g4f4");
    assert_eq!(subscriber.dropped.get(), 2);
}

/// A fresh directory under the target's scratch space.
fn scratch_directory(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn json_lines(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn replay_serves_recorded_frames_and_gaze() {
    let root = scratch_directory("demo-replay");
    std::fs::create_dir(root.join("frames")).unwrap();
    let mut log_rows = vec![
        json!({"kind": "step", "name": "calibration", "ok": true, "translation": [0, 0, 4.14], "angles": [0, 0, 0]}),
    ];
    for index in 0..3 {
        let name = format!("frame-{index:04}-4x3.gray");
        std::fs::write(root.join("frames").join(&name), [index as u8 * 40; 12]).unwrap();
        log_rows.push(json!({"kind": "frame", "timestamp": 1_000_000 + index * 30_000, "width": 4, "height": 3,
                             "path": format!("/mnt/c/elsewhere/frames/{name}")}));
    }
    std::fs::write(root.join("frames.jsonl"), json_lines(&log_rows)).unwrap();
    let gaze_rows: Vec<Value> = ["both", "left", "none"]
        .map(|name| json!({"kind": "message", "command": 0x500, "stream": 1, "payload": hex(&payload(name))}))
        .to_vec();
    std::fs::write(root.join("gaze.jsonl"), json_lines(&gaze_rows)).unwrap();
    let args: Vec<String> = ["--port", "0", "--no-headpose", "--image-rate", "100", "--replay-frames"]
        .into_iter()
        .map(String::from)
        .chain([root.join("frames.jsonl").display().to_string(), "--replay-gaze".into()])
        .chain([root.join("gaze.jsonl").display().to_string()])
        .collect();
    let arguments =
        tobii_linux::demo::Arguments::from(&tobii_linux::demo::parser("tobii-linux demo").parse(&args).unwrap());

    let received = common::run(async move {
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let (ready, url) = tokio::sync::oneshot::channel();
        let ready: Box<dyn FnOnce(String)> = Box::new(move |url| ready.send(url).unwrap());
        let demo = tokio::task::spawn_local(async move {
            tobii_linux::demo::run(&arguments, fake_processors(), Some(ready), Some(stopped)).await
        });
        let url = url.await.unwrap();
        let port: u16 = url.trim_end_matches('/').rsplit(':').next().unwrap().parse().unwrap();
        let (arrived, _arrivals) = tokio::sync::mpsc::unbounded_channel();
        let received = tokio::task::spawn_blocking(move || read_events(port, &["frame", "gaze", "status"], arrived))
            .await
            .unwrap();
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(10), demo).await.unwrap().unwrap().unwrap();
        received
    });
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(received["status"][0]["mode"], "replay");
    assert_eq!(received["frame"][0]["width"], 4);
    let point = &received["gaze"][0]["point"];
    assert!([json!([0.4717, 1.1246]), json!([0.6322, 0.0999]), Value::Null].contains(point), "{point}");
}

/// Polls `condition` for up to five seconds.
fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "condition not reached: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn sessions_restart_after_the_tracker_is_lost() {
    let (first, second) = (FakeTracker::streaming(), FakeTracker::streaming());
    let devices = [None, Some(first.clone()), Some(second.clone())];
    let mut opened = 0;
    let mut open: OpenSession<FakeTracker> = Box::new(move || {
        let device = devices[opened.min(devices.len() - 1)].clone();
        opened += 1;
        let device = device.ok_or_else(|| DeviceError::NoTracker("No Eye Tracker 5 is visible to libusb".into()))?;
        let mut session = TrackerSession::new(device);
        session.initialize()?;
        Ok((Vec::new(), session))
    });
    let pipeline = without_head_pose(&Arc::default());
    let source = SourceThread::default();
    {
        let pipeline = Arc::clone(&pipeline);
        source.start("live", move |stop| run_live(&pipeline, &mut open, 0, Backoff::new(0.01, 0.02, 10.0), &stop));
    }
    let streaming = |sessions: u64| {
        let now = status(&pipeline);
        now["sessions"] == sessions && now["connection"] == "streaming" && now["presence"]["state"] == "present"
    };
    until("first session streams", || streaming(1) && status(&pipeline)["counters"]["frames"].as_u64() > Some(0));
    first.unplug();
    until("second session streams", || streaming(2));
    source.stop.set();
    assert!(source.join(WAIT));
    let status = status(&pipeline);
    assert_eq!((&status["sessions"], &status["lost_sessions"]), (&json!(2), &json!(1)));
    assert_eq!(status["presence"]["state"], "present");
    assert!(status["counters"]["frames"].as_u64().unwrap() > 0);
    let unsubscribed = second.commands().into_iter().filter(|c| *c == command::UNSUBSCRIBE_STREAM).count();
    assert_eq!(unsubscribed, 3);
}
