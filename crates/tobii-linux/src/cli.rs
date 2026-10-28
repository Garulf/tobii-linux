//! Command-line entry point: descriptor probe, the head-pose session and the game service.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tobii_device::streams::HostSuspendWatch;
use tobii_device::supervisor::{Backoff, LinkHost, LinkState, TrackerLink};
use tobii_device::{Channel, StopFlag, StopSignal, TrackerSession};
use tobii_headpose::pipeline::DisplayArea;
use tobii_service::prp_service::{DEFAULT_NAME, DEFAULT_PORT};
use tokio::sync::mpsc;

use crate::args::{Exit, OptionSpec, Parser, help_for};
use crate::error::{Error, Result};
use crate::pyjson::{self, Json, object};
use crate::service::clock::{Clocks, ServiceTime, WINE_CLOCKS, wine_clock};
use crate::service::enabled::EnabledState;
use crate::service::{Corners, Event, Options, PrpService, list_repr, listen};
use crate::tracker::{FrameProcessor, HostTime, ModelProcessor, ServiceFeed, serve_tracker};

const MAIN_HELP: &str = include_str!("help/main.txt");
const PROBE_HELP: &str = include_str!("help/probe.txt");
const HEADPOSE_HELP: &str = include_str!("help/headpose.txt");
const SERVE_HELP: &str = include_str!("help/serve.txt");
pub const COMMANDS: [&str; 5] = ["probe", "demo", "setup", "headpose", "serve"];
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);
const DISPLAY_VALUES: usize = 9;

fn display_option() -> OptionSpec {
    OptionSpec::floats("--display", DISPLAY_VALUES)
}

fn session_options(display: OptionSpec) -> Vec<OptionSpec> {
    vec![display, OptionSpec::text("--models"), OptionSpec::int("--channel"), OptionSpec::int("--index")]
}

fn serve_options() -> Vec<OptionSpec> {
    let mut options = session_options(display_option());
    options.extend([
        OptionSpec::text("--host"),
        OptionSpec::int("--port"),
        OptionSpec::text("--name"),
        OptionSpec::text("--display-id"),
        OptionSpec::choice("--wine-clock", &WINE_CLOCKS),
        OptionSpec::flag("--no-gaze"),
        OptionSpec::flag("--tet-tcp"),
        OptionSpec::choice("--sesp-positions", &["prp", "windows"]),
    ]);
    options
}

/// The display corners of `--display`: top-left, top-right, bottom-left.
pub fn corners(values: &[f64]) -> Corners {
    [[values[0], values[1], values[2]], [values[3], values[4], values[5]], [values[6], values[7], values[8]]]
}

pub fn display_area(corners: &Corners) -> DisplayArea {
    let corner = |values: [f64; 3]| values.map(|v| v as f32);
    DisplayArea { top_left: corner(corners[0]), top_right: corner(corners[1]), bottom_left: corner(corners[2]) }
}

/// Options shared by the head-pose session and the service; only `serve` may omit `--display`.
#[derive(Debug, Clone)]
pub struct Session {
    pub corners: Option<Corners>,
    pub models: PathBuf,
    pub channel: i64,
    pub index: i64,
}

impl Session {
    fn from(values: &crate::args::Values) -> Self {
        Self {
            corners: values.floats("--display").map(|values| corners(&values)),
            models: values.text("--models").map_or_else(crate::paths::default_models, PathBuf::from),
            channel: values.int("--channel").unwrap_or(0),
            index: values.int("--index").unwrap_or(0),
        }
    }

    pub fn open(&self) -> Result<(tobii_usb::ProbedDevice, tobii_device::TrackerSession<tobii_usb::UsbChannel>)> {
        let channel = u32::try_from(self.channel)
            .map_err(|_| Error::Value(format!("USB channel {} is out of range", self.channel)))?;
        let index = usize::try_from(self.index).map_err(|_| Error::Runtime("list index out of range".into()))?;
        Ok(tobii_device::usb_session::usb_session(index, channel)?)
    }
}

/// The JSON error line the command prints on stderr for a failure.
pub fn report_error(error: &Error) {
    eprintln!("{}", pyjson::dumps(&object([("error", Json::Str(error.to_string()))])));
}

fn probe_json(devices: &[tobii_usb::ProbedDevice]) -> Json {
    let device = |probed: &tobii_usb::ProbedDevice| {
        let descriptor = Json::Object(
            probed.device.fields().iter().map(|(name, value)| (name.to_string(), Json::from(*value))).collect(),
        );
        let configurations = probed
            .configurations
            .iter()
            .map(|configuration| {
                let interfaces = configuration
                    .interfaces
                    .iter()
                    .map(|alt| {
                        let endpoints = alt
                            .endpoints
                            .iter()
                            .map(|ep| {
                                object([
                                    ("address", ep.address.into()),
                                    ("attributes", ep.attributes.into()),
                                    ("max_packet_size", ep.max_packet_size.into()),
                                    ("interval", ep.interval.into()),
                                ])
                            })
                            .collect();
                        object([
                            ("interface", alt.interface.into()),
                            ("alternate", alt.alternate.into()),
                            ("interface_class", alt.interface_class.into()),
                            ("endpoints", Json::List(endpoints)),
                        ])
                    })
                    .collect();
                object([("value", configuration.value.into()), ("interfaces", Json::List(interfaces))])
            })
            .collect();
        object([
            ("bus", probed.bus.into()),
            ("address", probed.address.into()),
            ("device", descriptor),
            ("configurations", Json::List(configurations)),
        ])
    };
    object([("status", "descriptor probe only".into()), ("devices", Json::List(devices.iter().map(device).collect()))])
}

fn probe() -> Result<ExitCode> {
    let devices = tobii_usb::probe()?;
    println!("{}", pyjson::indented(&probe_json(&devices)));
    Ok(ExitCode::SUCCESS)
}

pub fn sample_json(sample: &tobii_headpose::pipeline::HeadPoseSample) -> Json {
    object([
        ("timestamp_us", sample.timestamp_us.into()),
        ("valid", sample.valid.into()),
        ("position_mm", sample.position_mm.into()),
        ("rotation_rad", sample.rotation_rad.into()),
    ])
}

fn headpose(session: &Session) -> Result<ExitCode> {
    use std::io::Write;
    let (_, mut tracker) = session.open()?;
    let corners = session.corners.expect("headpose requires --display");
    let mut processor = ModelProcessor::load(&session.models, display_area(&corners))?;
    let mut stdout = std::io::stdout();
    crate::tracker::head_pose_samples(&mut tracker, &mut processor, |sample| {
        writeln!(stdout, "{}", pyjson::dumps(&sample_json(&sample)))?;
        stdout.flush()?;
        Ok(())
    })?;
    Ok(ExitCode::SUCCESS)
}

/// The service arguments of `serve`.
#[derive(Debug, Clone)]
pub struct Serve {
    pub session: Session,
    pub host: String,
    pub port: i64,
    pub options: Options,
    pub wine_clock: String,
}

impl Serve {
    fn from(values: &crate::args::Values) -> Self {
        let session = Session::from(values);
        let options = Options {
            name: values.text("--name").unwrap_or(DEFAULT_NAME).into(),
            display_area: session.corners,
            display_id: values.text("--display-id").map(String::from),
            gaze: !values.flag("--no-gaze"),
            tet_tcp: values.flag("--tet-tcp"),
            compensate_sesp_position: values.text("--sesp-positions").unwrap_or("prp") == "prp",
            enabled_state: Some(EnabledState::default_path(
                std::env::var("XDG_STATE_HOME").ok().as_deref(),
                std::env::var("HOME").ok().as_deref(),
            )),
            ..Options::default()
        };
        Self {
            session,
            host: values.text("--host").unwrap_or("127.0.0.1").into(),
            port: values.int("--port").unwrap_or(i64::from(DEFAULT_PORT)),
            options,
            wine_clock: values.text("--wine-clock").unwrap_or("auto").into(),
        }
    }
}

/// How often a wait on the gate looks at the process-wide stop signal.
const GATE_STOP_POLL: Duration = Duration::from_millis(50);

/// Lends the tracker to another program (`tobii-linux-setup`) through the control channel.
///
/// While the gate is held the running session stops and no new one opens USB. Sessions take
/// a [`UsbClaim`] before opening the tracker and drop it after closing it, so a holder can
/// wait in [`ReleaseGate::closed`] until USB is free.
#[derive(Debug)]
pub struct ReleaseGate {
    held: Mutex<bool>,
    changed: Condvar,
    usb: tokio::sync::watch::Sender<bool>,
}

impl Default for ReleaseGate {
    fn default() -> Self {
        Self { held: Mutex::new(false), changed: Condvar::new(), usb: tokio::sync::watch::Sender::new(false) }
    }
}

impl ReleaseGate {
    fn lock(&self) -> MutexGuard<'_, bool> {
        self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set(&self, held: bool) {
        *self.lock() = held;
        self.changed.notify_all();
    }

    /// Stops the running session and keeps new ones from opening the tracker.
    pub fn hold(&self) {
        self.set(true);
    }

    pub fn open(&self) {
        self.set(false);
    }

    pub fn is_held(&self) -> bool {
        *self.lock()
    }

    /// Whether a session holds the tracker's USB device.
    pub fn usb_open(&self) -> bool {
        *self.usb.borrow()
    }

    /// Waits until the gate is open, then claims USB until the claim is dropped; `None` when
    /// `stop` is set first.
    pub fn wait_open(&self, stop: &dyn StopSignal) -> Option<UsbClaim<'_>> {
        let mut held = self.lock();
        loop {
            if stop.is_set() {
                return None;
            }
            if !*held {
                self.usb.send_replace(true);
                return Some(UsbClaim(self));
            }
            held = self.changed.wait_timeout(held, GATE_STOP_POLL).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    /// Waits up to `timeout` until the gate is held or `stop` is set; true when either is.
    fn wait_held(&self, stop: &dyn StopSignal, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut held = self.lock();
        loop {
            if *held || stop.is_set() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let slice = remaining.min(GATE_STOP_POLL);
            held = self.changed.wait_timeout(held, slice).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    /// Returns once no session holds USB.
    pub async fn closed(&self) {
        let mut usb = self.usb.subscribe();
        let _ = usb.wait_for(|open| !open).await;
    }
}

/// A session's hold on the tracker's USB device, taken from [`ReleaseGate::wait_open`].
#[derive(Debug)]
pub struct UsbClaim<'a>(&'a ReleaseGate);

impl Drop for UsbClaim<'_> {
    fn drop(&mut self) {
        self.0.usb.send_replace(false);
    }
}

/// A session's stop: the process-wide one, or the gate being held.
struct SessionStop<'a> {
    stop: &'a dyn StopSignal,
    gate: &'a ReleaseGate,
}

impl StopSignal for SessionStop<'_> {
    fn is_set(&self) -> bool {
        self.stop.is_set() || self.gate.is_held()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.gate.wait_held(self.stop, timeout)
    }
}

/// Opens a started tracker session: the USB tracker, or a scripted one in tests.
pub type OpenTracker<'a, C> = &'a mut dyn FnMut() -> Result<TrackerSession<C>>;

/// TrackerLink host feeding the service's events from the sessions `open` starts.
pub struct ServiceLink<'a, C> {
    pub open: OpenTracker<'a, C>,
    pub processor: &'a mut dyn FrameProcessor,
    pub clock: HostTime,
    pub feed: &'a ServiceFeed,
    pub events: mpsc::UnboundedSender<Event>,
    /// The service's pause flag, set while eye tracking is off.
    pub paused: Arc<AtomicBool>,
    /// The service's release gate; sessions open the tracker only while it is open.
    pub gate: Arc<ReleaseGate>,
    /// The configured display area; each session reads the tracker's when `None`.
    pub display_area: Option<Corners>,
}

impl<C> std::fmt::Debug for ServiceLink<'_, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceLink").field("clock", &self.clock).field("feed", self.feed).finish_non_exhaustive()
    }
}

impl<C: Channel> LinkHost for ServiceLink<'_, C> {
    type Error = Error;

    fn on_state(&mut self, state: LinkState, detail: &str) -> Result<()> {
        let _ = self.events.send(Event::Detach);
        let detail = if state == LinkState::Lost && self.gate.is_held() { "Released to a client" } else { detail };
        log::info!("tracker {}: {detail}", state.name());
        Ok(())
    }

    fn attempt(&mut self, stop: &dyn StopSignal, opened: &mut dyn FnMut()) -> Result<()> {
        let gate = Arc::clone(&self.gate);
        let Some(_usb) = gate.wait_open(stop) else { return Ok(()) };
        let mut session = (self.open)()?;
        opened();
        let (requests, inbox) = std::sync::mpsc::channel();
        let _ = self.events.send(Event::Requests(requests));
        let suspend = Box::new(HostSuspendWatch::default());
        let paused = Arc::clone(&self.paused);
        serve_tracker(
            &mut session,
            self.processor,
            self.clock.clone(),
            self.feed,
            &SessionStop { stop, gate: &gate },
            Some(suspend),
            Some(inbox),
            paused,
            self.display_area,
        )
    }

    fn now_s(&mut self) -> f64 {
        let now = rustix_monotonic();
        now as f64 / 1e6
    }
}

fn rustix_monotonic() -> i64 {
    use tobii_device::clock::{HostClock, MonotonicClock};
    MonotonicClock.now_us()
}

/// The feed of one service: every callback posts an event to the service's loop.
///
/// Gaze frames are always posted, for the control channel; whether games get gaze streams
/// is the service's `gaze` option.
pub fn service_feed(events: &mpsc::UnboundedSender<Event>, tet_tcp: bool) -> ServiceFeed {
    let post = |events: &mpsc::UnboundedSender<Event>| {
        let events = events.clone();
        move |event: Event| {
            let _ = events.send(event);
        }
    };
    let (sample, presence, started) = (post(events), post(events), post(events));
    let (gaze_post, device, device_stream) = (post(events), post(events), post(events));
    ServiceFeed {
        sample: Some(Box::new(move |s| sample(Event::Sample(s)))),
        gaze: Some(Box::new(move |frame| gaze_post(Event::Gaze(Box::new(frame))))),
        presence: Some(Box::new(move |time_us, value| presence(Event::Presence(time_us, value)))),
        started: Some(Box::new(move |calibration_id, display_area| {
            started(Event::Attach(calibration_id, display_area))
        })),
        device: tet_tcp.then(|| Box::new(move |replies| device(Event::Device(Box::new(replies)))) as Box<_>),
        device_stream: tet_tcp.then(|| {
            Box::new(move |message: &tobii_protocol::Message, time_us| {
                device_stream(Event::DeviceStream(Box::new(message.clone()), time_us))
            }) as Box<_>
        }),
    }
}

/// Serve games until SIGINT or SIGTERM, starting the tracker again whenever it is lost.
///
/// While no tracker session runs, game clients are disconnected and refused, as the
/// Windows runtime shuts its server down when the device goes away; the service process
/// and the bridge's control connection stay up.
fn serve(arguments: &Serve) -> Result<ExitCode> {
    crate::logger::install(log::Level::Info, crate::logger::TimeFormat::Milliseconds);
    let clock = wine_clock(&arguments.wine_clock, Clocks::Host).expect("checked by the parser");
    let replaced_per_session = arguments.session.corners.unwrap_or_default();
    let processor = ModelProcessor::load(&arguments.session.models, display_area(&replaced_per_session))?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, run_service(arguments, clock, Box::new(processor)))
}

async fn run_service(
    arguments: &Serve,
    clock: Arc<crate::service::clock::Clock>,
    mut processor: Box<dyn FrameProcessor>,
) -> Result<ExitCode> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let service = PrpService::new(arguments.options.clone(), Arc::clone(&clock));
    let port = u16::try_from(arguments.port).map_err(|_| Error::Value("bind(): port must be 0-65535.".into()))?;
    let listener = listen(&arguments.host, port).await?;
    log::info!(
        "{} service on {}:{}, pipes {}",
        if arguments.options.tet_tcp { "tet-tcp" } else { "PRP" },
        arguments.host,
        arguments.port,
        list_repr(&service.pipe_names())
    );
    let accept = service.serve(listener);
    let (events, received) = mpsc::unbounded_channel();
    let follow = {
        let service = service.clone();
        tokio::task::spawn_local(async move { service.follow(received).await })
    };
    let feed = service_feed(&events, arguments.options.tet_tcp);
    let (paused, gate) = (service.pause_flag(), service.release_gate());
    let stop = StopFlag::new();
    let (finished, done) = tokio::sync::oneshot::channel::<(u64, u64)>();
    let link_thread = {
        let session = arguments.session.clone();
        let host_clock = HostTime(Arc::new(ServiceTime(Arc::clone(&clock))));
        let stop = stop.clone();
        std::thread::Builder::new().name("tracker-link".into()).spawn(move || {
            let mut link = TrackerLink::new(Backoff::default());
            let processor = processor.as_mut();
            let mut open = || session.open().map(|(_, tracker)| tracker);
            let mut host = ServiceLink {
                open: &mut open,
                processor,
                clock: host_clock,
                feed: &feed,
                events,
                paused,
                gate,
                display_area: session.corners,
            };
            link.run(&mut host, &stop);
            let _ = finished.send((link.sessions(), link.failures()));
        })?
    };
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    log::info!("stopping");
    stop.set();
    let joined = tokio::time::timeout(SHUTDOWN_TIMEOUT, done).await;
    service.close();
    accept.abort();
    tokio::time::sleep(Duration::from_millis(10)).await;
    follow.abort();
    let Ok(Ok((sessions, failures))) = joined else {
        return Err(Error::Runtime("The tracker thread did not stop in time".into()));
    };
    let _ = link_thread.join();
    log::info!("stopped: {sessions} session(s), {failures} lost");
    Ok(ExitCode::SUCCESS)
}

/// `tobii-linux [-h] {probe,demo,setup,headpose,serve} ...`
pub fn main(argv: Vec<String>) -> ExitCode {
    let prog = argv.first().map_or("tobii-linux".to_string(), |arg0| {
        std::path::Path::new(arg0).file_name().map_or(arg0.clone(), |name| name.to_string_lossy().into_owned())
    });
    let args = &argv[argv.len().min(1)..];
    match args.first().map(String::as_str) {
        Some("demo") => return crate::demo::main(&args[1..], &format!("{prog} demo")),
        Some("setup") => return crate::setup::main(&args[1..]),
        _ => {}
    }
    match parse(&prog, args) {
        Ok(command) => run(command),
        Err(exit) => {
            exit.report();
            ExitCode::from(exit.code())
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    Probe,
    Headpose(Session),
    Serve(Box<Serve>),
    Demo(Vec<String>),
    Setup(Vec<String>),
}

fn subparser(prog: &str, command: &str, help: &str, options: Vec<OptionSpec>) -> Parser {
    Parser::new(format!("{prog} {command}"), help_for(prog, help), options)
}

pub fn parse(prog: &str, args: &[String]) -> std::result::Result<Command, Exit> {
    let main = Parser::new(prog, help_for(prog, MAIN_HELP), Vec::new());
    let mut extras = Vec::new();
    let mut index = 0;
    let command = loop {
        let Some(arg) = args.get(index) else {
            return Err(main.error("the following arguments are required: command"));
        };
        index += 1;
        if arg == "-h" || (arg.len() > 2 && "--help".starts_with(arg.as_str())) {
            return Err(Exit::Help(main.help.clone()));
        }
        if arg.starts_with('-') && arg != "-" && arg != "--" {
            extras.push(arg.clone());
            continue;
        }
        break arg.clone();
    };
    if !COMMANDS.contains(&command.as_str()) {
        let choices: Vec<String> = COMMANDS.iter().map(|choice| crate::args::python_repr(choice)).collect();
        return Err(main.error(&format!(
            "argument command: invalid choice: {} (choose from {})",
            crate::args::python_repr(&command),
            choices.join(", ")
        )));
    }
    let rest = &args[index..];
    let (parsed, unknown) = match command.as_str() {
        "probe" => {
            let (_, unknown) = subparser(prog, "probe", PROBE_HELP, Vec::new()).parse_known(rest)?;
            (Command::Probe, unknown)
        }
        "headpose" => {
            let (values, unknown) =
                subparser(prog, "headpose", HEADPOSE_HELP, session_options(display_option().required()))
                    .parse_known(rest)?;
            (Command::Headpose(Session::from(&values)), unknown)
        }
        "serve" => {
            let (values, unknown) = subparser(prog, "serve", SERVE_HELP, serve_options()).parse_known(rest)?;
            (Command::Serve(Box::new(Serve::from(&values))), unknown)
        }
        "demo" => (Command::Demo(rest.to_vec()), Vec::new()),
        _ => (Command::Setup(rest.to_vec()), Vec::new()),
    };
    extras.extend(unknown);
    if !extras.is_empty() {
        return Err(main.error(&format!("unrecognized arguments: {}", extras.join(" "))));
    }
    Ok(parsed)
}

fn run(command: Command) -> ExitCode {
    let outcome = match command {
        Command::Probe => probe(),
        Command::Headpose(session) => headpose(&session),
        Command::Serve(arguments) => serve(&arguments),
        Command::Demo(args) => return crate::demo::main(&args, "tobii-linux demo"),
        Command::Setup(args) => return crate::setup::main(&args),
    };
    outcome.unwrap_or_else(|error| {
        report_error(&error);
        ExitCode::FAILURE
    })
}
