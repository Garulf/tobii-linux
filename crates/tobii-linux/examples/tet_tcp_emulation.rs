//! `serve --tet-tcp` without a tracker, as tools/tet_tcp_emulation.py runs the Python service:
//! the device's recorded getter, protocol upgrade and unlock_ppu replies, its recorded gaze
//! messages looped at 30 Hz with a "present" record every second, and the synthetic head
//! sweep. Prints the same JSON summary after `--seconds`. Offline only.
//!
//! `tet_tcp_emulation [--host H] [--port P] [--device-query JSONL] [--replay RAW_LOG]
//! [--seconds S] [--wine-clock CLOCK] [--sesp-positions prp|windows] [--interrupt-at S] [--debug]`

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tobii_linux::pyjson::{Json, dumps, object};
use tobii_linux::service::clock::{Clocks, now_us, wine_clock};
use tobii_linux::service::synthetic::publish_synthetic;
use tobii_linux::service::{Options, PrpService, REAL_SERVICE_PERIOD, list_repr, listen};
use tobii_linux::tracker::DeviceReplies;
use tobii_protocol::Message;
use tobii_protocol::commands::{RESPONSE_KIND, STREAM_DATA_KIND, command};
use tobii_protocol::gaze::build::{columns_payload, int32_column, int64_column};
use tobii_service::tet_tcp::{GAZE_STREAM, PRESENCE_STREAM};
use tobii_usb::traffic::{Direction, replay_messages};

const PRIVATE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../evidence/private");
const GAZE_PERIOD: Duration = Duration::from_micros(33_333);
const PRESENCE_PERIOD: Duration = Duration::from_secs(1);
const INTERRUPTION: Duration = Duration::from_secs(3);
const PRESENT_DEVICE_VALUE: i32 = 2;
const RECORDED_COMMANDS: [(&str, u32); 15] = [
    ("ENUMERATE_STREAM_TYPES", command::ENUMERATE_STREAM_TYPES),
    ("GET_DEVICE_PROPERTIES", command::GET_DEVICE_PROPERTIES),
    ("GET_CAPABILITIES", command::GET_CAPABILITIES),
    ("GET_CAPABILITIES_ALL", command::GET_CAPABILITIES_ALL),
    ("GET_TRACK_BOX", command::GET_TRACK_BOX),
    ("GET_DEVICE_INFO", command::GET_DEVICE_INFO),
    ("GET_DISPLAY_AREA", command::GET_DISPLAY_AREA),
    ("GET_DEVICE_STATUS", command::GET_DEVICE_STATUS),
    ("GET_FREQUENCIES", command::GET_FREQUENCIES),
    ("ENUMERATE_FREQUENCIES", command::ENUMERATE_FREQUENCIES),
    ("GET_DEVICE_NAME", command::GET_DEVICE_NAME),
    ("ENUMERATE_EXTENSIONS", command::ENUMERATE_EXTENSIONS),
    ("ENUMERATE_ENABLED_EXTENSIONS", command::ENUMERATE_ENABLED_EXTENSIONS),
    ("GET_GEOMETRY_MOUNTING", command::GET_GEOMETRY_MOUNTING),
    ("GET_ENABLED_EYE", command::GET_ENABLED_EYE),
];

type Failure = Box<dyn std::error::Error>;

struct Arguments {
    host: String,
    port: u16,
    device_query: PathBuf,
    replay: PathBuf,
    seconds: f64,
    wine_clock: String,
    compensate_sesp_position: bool,
    interrupt_at: Option<f64>,
    debug: bool,
}

fn arguments() -> Result<Arguments, Failure> {
    let mut parsed = Arguments {
        host: "127.0.0.1".into(),
        port: 7760,
        device_query: Path::new(PRIVATE).join("gaze-capture/device-query.jsonl"),
        replay: Path::new(PRIVATE).join("calibration/probe/host-pattern-fixed-raw.jsonl"),
        seconds: 60.0,
        wine_clock: "auto".into(),
        compensate_sesp_position: true,
        interrupt_at: None,
        debug: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--host" => parsed.host = value()?,
            "--port" => parsed.port = value()?.parse()?,
            "--device-query" => parsed.device_query = value()?.into(),
            "--replay" => parsed.replay = value()?.into(),
            "--seconds" => parsed.seconds = value()?.parse()?,
            "--wine-clock" => parsed.wine_clock = value()?,
            "--sesp-positions" => parsed.compensate_sesp_position = value()? == "prp",
            "--interrupt-at" => parsed.interrupt_at = Some(value()?.parse()?),
            "--debug" => parsed.debug = true,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    Ok(parsed)
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

fn recorded(device_query: &Path, replay: &Path) -> Result<(DeviceReplies, Vec<Message>), Failure> {
    let mut replies = HashMap::new();
    for line in std::fs::read_to_string(device_query)?.lines() {
        let row: serde_json::Value = serde_json::from_str(line)?;
        if row["kind"] != "reply" {
            continue;
        }
        let name = row["command"].as_str().unwrap_or_default();
        let (_, command) = RECORDED_COMMANDS.iter().find(|(known, _)| *known == name).ok_or(format!("{name}?"))?;
        let payload = unhex(row["payload"].as_str().unwrap_or_default()).ok_or("payload is not hex")?;
        replies.insert(*command, Message::new(RESPONSE_KIND, 0, 1, *command, 0, payload));
    }
    let mut gaze = Vec::new();
    for replayed in replay_messages(replay, 0)? {
        let Ok(message) = replayed.message else { continue };
        if replayed.direction != Direction::In {
            continue;
        }
        if message.kind == RESPONSE_KIND && [command::PROTOCOL_UPGRADE, command::UNLOCK_PPU].contains(&message.command)
        {
            replies.entry(message.command).or_insert(message);
        } else if message.kind == STREAM_DATA_KIND && message.command == GAZE_STREAM {
            gaze.push(message);
        }
    }
    let streams = BTreeSet::from([GAZE_STREAM, PRESENCE_STREAM]);
    Ok((DeviceReplies { replies, streams }, gaze))
}

fn presence_message(timestamp_us: i64) -> Message {
    let payload = columns_payload(&[(1, int64_column(timestamp_us)), (2, int32_column(PRESENT_DEVICE_VALUE))])
        .expect("presence columns encode");
    Message::new(STREAM_DATA_KIND, 0, 1, PRESENCE_STREAM, 0, payload)
}

async fn feed_streams(service: PrpService, gaze: Vec<Message>) {
    let clock = Arc::clone(service.clock());
    let mut next_presence = tokio::time::Instant::now();
    for message in gaze.iter().cycle() {
        let now = now_us(&clock);
        service.publish_device_stream(message, now);
        if tokio::time::Instant::now() >= next_presence {
            service.publish_device_stream(&presence_message(now), now);
            next_presence = tokio::time::Instant::now() + PRESENCE_PERIOD;
        }
        tokio::time::sleep(GAZE_PERIOD).await;
    }
}

async fn run(arguments: Arguments) -> Result<(), Failure> {
    let clock = wine_clock(&arguments.wine_clock, Clocks::Host).ok_or("unknown --wine-clock")?;
    let options =
        Options { tet_tcp: true, compensate_sesp_position: arguments.compensate_sesp_position, ..Options::default() };
    let service = PrpService::new(options, Arc::clone(&clock));
    let (device, gaze) = recorded(&arguments.device_query, &arguments.replay)?;
    let accept = service.serve(listen(&arguments.host, arguments.port).await?);
    service.attach_tracker(Some(0), None);
    service.attach_device(device.clone());
    let tasks = [
        tokio::task::spawn_local(publish_synthetic(service.clone(), REAL_SERVICE_PERIOD)),
        tokio::task::spawn_local(feed_streams(service.clone(), gaze.clone())),
    ];
    log::info!(
        target: "tobii_linux",
        "tet-tcp emulation: relay {}:{}, pipes {}, {} device replies, {} gaze messages",
        arguments.host,
        arguments.port,
        list_repr(&service.pipe_names()),
        device.replies.len(),
        gaze.len()
    );
    let seconds = Duration::from_secs_f64(arguments.seconds);
    match arguments.interrupt_at.map(Duration::from_secs_f64) {
        Some(interrupt_at) => {
            tokio::time::sleep(interrupt_at).await;
            log::info!(target: "tobii_linux", "interrupting the tracker session for {:.1} s", INTERRUPTION.as_secs_f64());
            service.detach_tracker();
            tokio::time::sleep(INTERRUPTION).await;
            service.attach_tracker(Some(0), None);
            service.attach_device(device);
            tokio::time::sleep(seconds.saturating_sub(interrupt_at + INTERRUPTION)).await;
        }
        None => tokio::time::sleep(seconds).await,
    }
    tasks.iter().for_each(|task| task.abort());
    let sesp = service.sesp().expect("the tet-tcp route serves SESP");
    let tet_tcp = service.tet_tcp().expect("the tet-tcp route");
    let summary = object([
        ("sesp_head_poses_sent", Json::from(sesp.sent.get())),
        ("sesp_dropped", Json::from(sesp.dropped.get())),
        ("tet_tcp_stream_messages_sent", Json::from(tet_tcp.sent())),
        ("tet_tcp_dropped", Json::from(tet_tcp.dropped())),
        ("clock", Json::from(clock.state().source.name())),
        ("game_time_offset_us", Json::from(-service.game_time().map(0))),
    ]);
    service.close();
    accept.abort();
    tokio::time::sleep(Duration::from_millis(10)).await;
    println!("{}", dumps(&summary));
    Ok(())
}

fn main() -> Result<(), Failure> {
    let arguments = arguments()?;
    let level = if arguments.debug { log::Level::Debug } else { log::Level::Info };
    tobii_linux::logger::install(level, tobii_linux::logger::TimeFormat::Milliseconds);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    tokio::task::LocalSet::new().block_on(&runtime, run(arguments))
}
