//! The browser demo: one process owns the tracker and serves the page, a status snapshot
//! and a Server-Sent Events stream of camera images, head pose, gaze and status.
//!
//! The page is the Python package's `demo/static`, compiled in. Tracker reading, head-pose
//! processing and replay run on their own threads; their events reach the HTTP server's
//! single-threaded loop through a channel.

pub mod events;
pub mod pipeline;
pub mod server;
pub mod sources;
pub mod wireframe;

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tobii_device::supervisor::Backoff;
use tokio::sync::{mpsc, watch};

use crate::args::{OptionSpec, Parser, Values, help_for};
use crate::error::{Error, Result};
use crate::pyjson::Json;
use pipeline::{DemoPipeline, GAZE_SOURCES, ModelDemoProcessor, ProcessorFactory, Publish};
use server::{DemoServer, EventHub};
use sources::{ReplaySource, SourceThread};

pub const DEFAULT_PORT: u16 = 8765;
pub const ALBEDO_DISPLAY: [f64; 9] = [-296.62, 326.0, 111.82, 300.38, 326.0, 111.82, -296.62, 10.27, -3.1];
pub const STATUS_INTERVAL: Duration = Duration::from_secs(1);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const HELP: &str = include_str!("../help/demo.txt");

pub fn parser(prog: &str) -> Parser {
    let base = prog.strip_suffix(" demo").unwrap_or(prog);
    Parser::new(
        prog,
        help_for(base, HELP),
        vec![
            OptionSpec::text("--host"),
            OptionSpec::int("--port"),
            OptionSpec::floats("--display", 9),
            OptionSpec::text("--models"),
            OptionSpec::int("--channel"),
            OptionSpec::int("--index"),
            OptionSpec::float("--image-rate"),
            OptionSpec::choice("--gaze-source", &GAZE_SOURCES),
            OptionSpec::flag("--no-headpose"),
            OptionSpec::text("--replay-frames"),
            OptionSpec::text("--replay-frames-dir"),
            OptionSpec::text("--replay-gaze"),
        ],
    )
}

/// The demo's options.
#[derive(Debug, Clone)]
pub struct Arguments {
    pub host: String,
    pub port: i64,
    pub display: Vec<f64>,
    pub models: PathBuf,
    pub channel: i64,
    pub index: i64,
    pub image_rate: f64,
    pub head_pose: bool,
    pub replay_frames: Option<PathBuf>,
    pub replay_frames_dir: Option<PathBuf>,
    pub replay_gaze: Option<PathBuf>,
}

impl Arguments {
    pub fn from(values: &Values) -> Self {
        let path = |flag: &str| values.text(flag).map(PathBuf::from);
        Self {
            host: values.text("--host").unwrap_or("127.0.0.1").into(),
            port: values.int("--port").unwrap_or(i64::from(DEFAULT_PORT)),
            display: values.floats("--display").unwrap_or_else(|| ALBEDO_DISPLAY.to_vec()),
            models: path("--models").unwrap_or_else(crate::paths::default_models),
            channel: values.int("--channel").unwrap_or(0),
            index: values.int("--index").unwrap_or(0),
            image_rate: values.float("--image-rate").unwrap_or(15.0),
            head_pose: !values.flag("--no-headpose"),
            replay_frames: path("--replay-frames"),
            replay_frames_dir: path("--replay-frames-dir"),
            replay_gaze: path("--replay-gaze"),
        }
    }

    pub fn replaying(&self) -> bool {
        self.replay_frames.is_some() || self.replay_gaze.is_some()
    }
}

/// The page address for a listening host and port.
pub fn page_url(host: &str, port: u16) -> String {
    let host = if host == "127.0.0.1" || host == "::1" { "localhost" } else { host };
    format!("http://{host}:{port}/")
}

/// Where the demo's data comes from.
pub enum Source {
    Live { index: i64, channel: i64 },
    Replay(Box<ReplaySource>),
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Live { index, channel } => write!(f, "Live({index}, {channel})"),
            Source::Replay(_) => f.write_str("Replay"),
        }
    }
}

impl Source {
    pub fn from(arguments: &Arguments) -> Result<Self> {
        if arguments.replaying() {
            return Ok(Source::Replay(Box::new(ReplaySource::load(
                arguments.replay_frames.as_deref(),
                arguments.replay_frames_dir.as_deref(),
                arguments.replay_gaze.as_deref(),
            )?)));
        }
        Ok(Source::Live { index: arguments.index, channel: arguments.channel })
    }

    fn start(self, thread: &SourceThread, pipeline: &Arc<DemoPipeline>) {
        let pipeline = Arc::clone(pipeline);
        match self {
            Source::Live { index, channel } => thread.start("live", move |stop| {
                let mut open = sources::usb_opener(index, channel);
                sources::run_live(&pipeline, &mut open, channel, Backoff::default(), &stop);
            }),
            Source::Replay(replay) => thread.start("replay", move |stop| replay.run(&pipeline, &stop)),
        }
    }
}

/// Serves until `stop` turns true or SIGINT/SIGTERM arrives; `ready(url)` runs once the port
/// is open (without it the page address is printed).
pub async fn run(
    arguments: &Arguments,
    factory: ProcessorFactory,
    ready: Option<Box<dyn FnOnce(String)>>,
    mut stop: Option<watch::Receiver<bool>>,
) -> Result<()> {
    let hub = Rc::new(EventHub::default());
    let (events, mut received) = mpsc::unbounded_channel::<(String, Vec<u8>, Option<Json>)>();
    let publish: Publish = Arc::new(move |topic: &str, payload: Json, retain: bool| {
        if let Some(block) = server::encode(topic, &payload) {
            let _ = events.send((topic.to_string(), block, retain.then_some(payload)));
        }
    });
    let corners = crate::cli::corners(&arguments.display);
    let mode = if arguments.replaying() { "replay" } else { "live" };
    let pipeline = DemoPipeline::new(
        Arc::clone(&publish),
        corners,
        arguments.models.clone(),
        arguments.image_rate,
        mode,
        arguments.head_pose,
        factory,
    );
    let source = Source::from(arguments)?;
    let port = u16::try_from(arguments.port).map_err(|_| Error::Value("bind(): port must be 0-65535.".into()))?;
    let server = DemoServer::start(Rc::clone(&hub), &arguments.host, port).await?;
    let url = page_url(&arguments.host, server.port);
    let delivered = {
        let hub = Rc::clone(&hub);
        tokio::task::spawn_local(async move {
            while let Some((topic, block, payload)) = received.recv().await {
                hub.publish_block(&topic, block, payload);
            }
        })
    };
    use tokio::signal::unix::{SignalKind, signal};
    let (mut interrupt, mut terminate) = (signal(SignalKind::interrupt())?, signal(SignalKind::terminate())?);
    match ready {
        Some(ready) => ready(url),
        None => println!("Demo running at {url} (Ctrl+C to stop)"),
    }
    use std::io::Write;
    let _ = std::io::stdout().flush();
    pipeline.start();
    let thread = SourceThread::default();
    source.start(&thread, &pipeline);
    let status = {
        let (hub, pipeline) = (Rc::clone(&hub), Arc::clone(&pipeline));
        tokio::task::spawn_local(async move {
            loop {
                hub.publish("status", pipeline.status(), true);
                tokio::time::sleep(STATUS_INTERVAL).await;
            }
        })
    };
    let stopped = async {
        match stop.as_mut() {
            Some(stop) => {
                let _ = stop.wait_for(|stopped| *stopped).await;
            }
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        () = stopped => {}
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    status.abort();
    thread.stop.set();
    let clean = thread.join(SHUTDOWN_TIMEOUT);
    pipeline.stop(SHUTDOWN_TIMEOUT);
    server.close().await;
    delivered.abort();
    if !clean {
        eprintln!("The tracker thread did not stop in time; streams may still be subscribed");
    }
    Ok(())
}

/// `tobii-linux demo [options]`.
pub fn main(args: &[String], prog: &str) -> ExitCode {
    let values = match parser(prog).parse(args) {
        Ok(values) => values,
        Err(exit) => {
            exit.report();
            return ExitCode::from(exit.code());
        }
    };
    crate::logger::install(log::Level::Info, crate::logger::TimeFormat::Clock);
    let arguments = Arguments::from(&values);
    let outcome =
        tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(Error::from).and_then(|runtime| {
            tokio::task::LocalSet::new().block_on(&runtime, run(&arguments, ModelDemoProcessor::factory(), None, None))
        });
    match outcome {
        Ok(()) => {
            println!("Stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            crate::cli::report_error(&error);
            ExitCode::FAILURE
        }
    }
}
