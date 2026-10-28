//! The command line of `tobii-linux-setup`, and what it chooses before the window opens.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde_json::Value as Json;
use tobii_device::DeviceError;
use tobii_gaze::display as ds;
use tobii_gaze::monitors::{self, Monitor};

use crate::device::{self, Session, WouldSend};
use crate::flow::{
    self, Desktop, DisplayMode, ExitCodes, FlowError, FlowKind, OpenSession, SetupOptions, load_display_record,
};
use crate::handover::{self, ServiceChoice};
use crate::simulated::{SimulatedTracker, Simulation};

pub const USAGE: &str = "\
usage: tobii-linux-setup [options]

Set up the Eye Tracker 5 on this display the way Tobii's setup does: line up the
display (when needed), check where you sit, calibrate, and save the result as a profile.
When tobii-linux serve runs, it lends the tracker to the setup until the setup ends.

Tobii's configuration arguments (at most one):
  -C                           Improve the calibration of the current profile
  -N                           Create a new profile: calibrate, then name it
  -S                           Set up the display only
  -R                           Resolve the configuration: the display setup when the tracker
                               has none, then a calibration when the current profile has none
  -G, --guest                  Calibrate the temporary Guest profile and make it current

options:
  --display auto|always|never  Run the display setup: auto when the tracker has none yet
                               (default), always, never
  --no-calibration             Stop after the display setup
  --save-display               Send a changed display setup to the tracker without asking
  --dry-run                    Read from the tracker but only report what would be written
  --profile NAME               Calibration profile to recalibrate (default: the current one;
                               without one, the window asks for a name)
  --profiles-directory DIR     Profile store (default $XDG_DATA_HOME/tobii-linux/profiles)
  --backups-directory DIR      Where the previous calibration and display setup are kept
                               (default $XDG_DATA_HOME/tobii-linux/backups)
  --edid FILE                  EDID file whose size is used as the screen size hint
  --connector NAME             DRM connector (e.g. card0-DP-1) whose EDID gives the size hint
  --monitor NAME               Start on this monitor (output name such as DP-1); default: the
                               one of the last display setup, else the primary
  --index N                    Device index from probe (default 0)
  --channel N                  USB channel carrying TTP (default 0)
  --service ADDR               Control address of tobii-linux serve, asked to release the
                               tracker first (default 127.0.0.1:7760)
  --no-service                 Open the tracker without asking tobii-linux serve
  --simulate mapped|unmapped   Use a scripted tracker and user instead of the device
  --simulate-monitors          With --simulate, pretend to run on a desktop with a portrait
                               monitor left of a 27 inch primary one
  --simulate-compute-failure N With --simulate, refuse the Nth compute (1070) as all points
                               failed, to show the failure screen
  --windowed                   Do not ask for fullscreen (the window must still fill the screen)
  --log FILE                   Write the steps as JSON lines to this file
  -h, --help                   Show this help

exit codes: 0 done, 1 failed, 2 cancelled or an argument error; with one of Tobii's
arguments Tobii's: 0 done, 1 argument error, 2 cancelled, 3 failed";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub flow: FlowKind,
    pub display: DisplayMode,
    pub calibrate: bool,
    pub save_display: bool,
    pub dry_run: bool,
    pub profile: Option<String>,
    pub profiles_directory: Option<PathBuf>,
    pub backups_directory: Option<PathBuf>,
    pub edid: Option<PathBuf>,
    pub connector: Option<String>,
    pub monitor: Option<String>,
    pub index: usize,
    pub channel: u32,
    pub service: Option<SocketAddr>,
    pub no_service: bool,
    pub simulate: Option<bool>,
    pub simulate_monitors: bool,
    pub simulate_compute_failure: Option<usize>,
    pub windowed: bool,
    pub log: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Run(Box<Args>),
    Help,
}

fn number<T: std::str::FromStr>(option: &str, value: &str) -> Result<T, String> {
    value.parse().map_err(|_| format!("{option}: invalid number '{value}'"))
}

/// Tobii's `-C`, `-N`, `-S`, `-R` and `-G` (`--guest`), as the flow they start.
fn tobii_flow(argument: &str) -> Option<FlowKind> {
    Some(match argument {
        "-C" => FlowKind::Recalibrate,
        "-N" => FlowKind::NewProfile,
        "-S" => FlowKind::DisplaySetup,
        "-R" => FlowKind::ResolveConfiguration,
        "-G" | "--guest" => FlowKind::Guest,
        _ => return None,
    })
}

/// The display and calibration steps of a Tobii flow.
fn apply_flow(args: &mut Args, flow: FlowKind) {
    args.flow = flow;
    (args.display, args.calibrate) = match flow {
        FlowKind::Setup | FlowKind::ResolveConfiguration => (DisplayMode::Auto, true),
        FlowKind::Recalibrate | FlowKind::NewProfile | FlowKind::Guest => (DisplayMode::Never, true),
        FlowKind::DisplaySetup => (DisplayMode::Always, false),
    };
}

/// Tobii's exit codes when one of Tobii's configuration arguments appears among the
/// arguments (valid or not), else tobii-linux's own.
pub fn exit_codes<I: IntoIterator<Item = String>>(arguments: I) -> ExitCodes {
    if arguments.into_iter().any(|argument| tobii_flow(&argument).is_some()) {
        ExitCodes::Tobii
    } else {
        ExitCodes::Legacy
    }
}

/// Parses the arguments after the program name.
pub fn parse<I: IntoIterator<Item = String>>(arguments: I) -> Result<Command, String> {
    let mut args = Args { calibrate: true, ..Args::default() };
    let mut tobii: Option<(String, FlowKind)> = None;
    let mut steps_chosen = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let (option, inline) = match argument.split_once('=') {
            Some((option, value)) if option.starts_with("--") => (option.to_string(), Some(value.to_string())),
            _ => (argument.clone(), None),
        };
        let mut value =
            || inline.clone().or_else(|| arguments.next()).ok_or_else(|| format!("{option}: expected a value"));
        if let Some(flow) = tobii_flow(&option) {
            if let Some((first, _)) = &tobii {
                return Err(format!("{option}: only one of -C, -N, -S, -R, -G (--guest), not also {first}"));
            }
            tobii = Some((option.clone(), flow));
            continue;
        }
        if matches!(option.as_str(), "--display" | "--no-calibration" | "--profile") {
            steps_chosen.get_or_insert_with(|| option.clone());
        }
        match option.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--display" => {
                args.display = match value()?.as_str() {
                    "auto" => DisplayMode::Auto,
                    "always" => DisplayMode::Always,
                    "never" => DisplayMode::Never,
                    other => return Err(format!("--display: invalid choice '{other}' (auto, always, never)")),
                }
            }
            "--no-calibration" => args.calibrate = false,
            "--save-display" => args.save_display = true,
            "--dry-run" => args.dry_run = true,
            "--profile" => args.profile = Some(value()?),
            "--profiles-directory" => args.profiles_directory = Some(value()?.into()),
            "--backups-directory" => args.backups_directory = Some(value()?.into()),
            "--edid" => args.edid = Some(value()?.into()),
            "--connector" => args.connector = Some(value()?),
            "--monitor" => args.monitor = Some(value()?),
            "--index" => args.index = number(&option, &value()?)?,
            "--channel" => args.channel = number(&option, &value()?)?,
            "--service" => {
                let text = value()?;
                args.service = Some(text.parse().map_err(|_| format!("--service: invalid address '{text}'"))?);
            }
            "--no-service" => args.no_service = true,
            "--simulate" => {
                args.simulate = match value()?.as_str() {
                    "mapped" => Some(true),
                    "unmapped" => Some(false),
                    other => return Err(format!("--simulate: invalid choice '{other}' (mapped, unmapped)")),
                }
            }
            "--simulate-monitors" => args.simulate_monitors = true,
            "--simulate-compute-failure" => args.simulate_compute_failure = Some(number(&option, &value()?)?),
            "--windowed" => args.windowed = true,
            "--log" => args.log = Some(value()?.into()),
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }
    if let Some((flow_argument, flow)) = tobii {
        if let Some(option) = steps_chosen {
            return Err(format!("{flow_argument} cannot be combined with {option}"));
        }
        apply_flow(&mut args, flow);
    }
    if args.service.is_some() && args.no_service {
        return Err("--service and --no-service cannot be combined".into());
    }
    if args.edid.is_some() && args.connector.is_some() {
        return Err("--edid and --connector cannot be combined".into());
    }
    if (args.simulate_monitors || args.simulate_compute_failure.is_some()) && args.simulate.is_none() {
        return Err("--simulate-monitors and --simulate-compute-failure need --simulate".into());
    }
    Ok(Command::Run(Box::new(args)))
}

/// The screen size hint of --edid or --connector.
pub fn size_hint(args: &Args) -> Result<Option<[f64; 2]>, String> {
    if let Some(path) = &args.edid {
        let edid = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return ds::edid_size_mm(&edid).map(Some).map_err(|e| e.to_string());
    }
    if let Some(connector) = &args.connector {
        let found = ds::drm_monitors(std::path::Path::new("/sys/class/drm")).map_err(|e| e.to_string())?;
        let monitor = found.into_iter().find(|m| &m.connector == connector);
        return monitor.map(|m| Some(m.size_mm)).ok_or_else(|| format!("No EDID for connector {connector}"));
    }
    Ok(None)
}

/// A fresh directory for the profiles and backups of a simulation.
pub fn scratch_directory() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!("tobii-linux-setup-simulation-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

/// SetupOptions; a simulation keeps its profiles, backups and display record in `scratch`
/// unless told otherwise.
pub fn options_from(args: &Args, scratch: Option<PathBuf>) -> Result<SetupOptions, String> {
    let data = scratch.clone().unwrap_or_else(flow::default_data_directory);
    let mut options = SetupOptions::in_directory(&data);
    if let Some(directory) = &args.profiles_directory {
        options.profiles_directory = directory.clone();
    }
    if let Some(directory) = &args.backups_directory {
        options.backups_directory = directory.clone();
    }
    if scratch.is_some()
        && let Some(parent) = options.profiles_directory.parent()
    {
        options.display_record = parent.join(flow::DISPLAY_RECORD);
    }
    options.flow = args.flow;
    options.display = args.display;
    options.calibrate = args.calibrate;
    options.save_display_without_asking = args.save_display;
    options.dry_run = args.dry_run;
    options.profile = args.profile.clone();
    options.size_hint_mm = size_hint(args)?;
    Ok(options)
}

/// Whether the run asks a running `tobii-linux serve` for the tracker: never in a simulation
/// or with --no-service.
pub fn service_choice(args: &Args) -> ServiceChoice {
    if args.simulate.is_some() || args.no_service {
        return ServiceChoice::Skip;
    }
    ServiceChoice::Ask(args.service.unwrap_or(tobii_control::DEFAULT_ADDR))
}

/// Makes the dry-run report of a session, when the run is a dry run.
pub type DryRunReport = Box<dyn Fn() -> Option<WouldSend> + Send>;

/// Opens the run's tracker: the simulated one, or USB through `open_usb` (normally
/// [`device::open_usb`]) after a running service released it.
pub fn session_opener<U>(args: &Args, dry_run_report: DryRunReport, open_usb: U) -> OpenSession
where
    U: FnOnce(usize, u32, Option<WouldSend>) -> Result<Session, DeviceError> + Send + 'static,
{
    let fail_compute_at: Vec<usize> = args.simulate_compute_failure.into_iter().collect();
    let service = service_choice(args);
    match args.simulate {
        Some(mapped) => Box::new(move || {
            let tracker = SimulatedTracker::new(Simulation { mapped, fail_compute_at, ..Simulation::default() });
            Ok((device::session_on(tracker, dry_run_report()), None))
        }),
        None => {
            let (index, channel) = (args.index, args.channel);
            Box::new(move || {
                handover::open_borrowed(service, || open_usb(index, channel, dry_run_report()).map_err(FlowError::from))
            })
        }
    }
}

/// albedo's EDIDs (LG 27GL850, base blocks), for the simulated two-monitor desktop.
pub const PRIMARY_EDID: &str = "00ffffffffffff001e6d7f5b0b220100061e0104b53c22789f8cb5af4f43ab260e5054254b007140818081c0a9c0b300d1c08100d1cf28de0050a0a038500830080455502100001a000000fd003090e6e63c010a202020202020000000fc003237474c3835300a2020202020000000ff003030364e54435a32363235310a01fa";
pub const PORTRAIT_EDID: &str = "00ffffffffffff001e6d7f5b5fc20300061e0104b53c22789f8cb5af4f43ab260e5054254b007140818081c0a9c0b300d1c08100d1cf28de0050a0a038500830080455502100001a000000fd003090e6e63c010a202020202020000000fc003237474c3835300a2020202020000000ff003030364e544e4837383336370a01fc";

/// A portrait monitor (HDMI-1) left of a 2560 x 1440 primary one (DP-1), as `xrandr --prop`
/// would list albedo's desktop under X11 (synthetic, like the monitors tests).
pub fn simulated_monitors() -> Vec<Monitor> {
    let text = format!(
        "Screen 0: minimum 320 x 200, current 4000 x 2560, maximum 16384 x 16384\n\
         DP-1 connected primary 2560x1440+1440+588 (normal left inverted right x axis y axis) 597mm x 336mm\n\
         \tEDID:\n\t\t{PRIMARY_EDID}\n   2560x1440    143.97*+  59.95\n\
         HDMI-1 connected 1440x2560+0+0 left (normal left inverted right x axis y axis) 597mm x 336mm\n\
         \tEDID:\n\t\t{PORTRAIT_EDID}\n   2560x1440     59.95*+\n"
    );
    monitors::parse_xrandr(&text).expect("the simulated xrandr output parses")
}

/// The monitors, the one to start on (Tobii's rule, or --monitor), and whether the window can move.
pub fn choose_desktop(args: &Args, monitors: Vec<Monitor>, record: &Json) -> Desktop {
    let recorded = record.get("monitor").and_then(|m| m.get("key")).and_then(Json::as_str);
    let wanted = args.monitor.as_deref().or(recorded);
    let named = args.monitor.as_ref().and_then(|name| monitors.iter().position(|m| &m.key == name || &m.name == name));
    let current = named.or_else(|| monitors::initial_monitor(&monitors, wanted));
    let movable = monitors.iter().filter(|m| m.bounds.is_some()).count() > 1;
    Desktop { monitors, current, movable }
}

/// The desktop of the run: simulated, or the Linux desktop's monitors.
pub fn desktop(args: &Args, options: &SetupOptions) -> Desktop {
    let monitors = match args.simulate {
        Some(_) if args.simulate_monitors => simulated_monitors(),
        Some(_) => Vec::new(),
        None => monitors::linux_monitors().unwrap_or_default(),
    };
    choose_desktop(args, monitors, &load_display_record(&options.display_record))
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::simulated::{SimulatedTracker, Simulation};

    fn run(arguments: &[&str]) -> Args {
        match parse(arguments.iter().map(|a| a.to_string())).unwrap() {
            Command::Run(args) => *args,
            Command::Help => panic!("help"),
        }
    }

    #[test]
    fn defaults_and_options() {
        let args = run(&[]);
        assert_eq!((args.display, args.calibrate, args.simulate), (DisplayMode::Auto, true, None));
        let args = run(&["--simulate", "unmapped", "--display=always", "--no-calibration", "--profile", "Anna"]);
        assert_eq!(run(&["--simulate=mapped", "--simulate-compute-failure", "2"]).simulate_compute_failure, Some(2));
        assert_eq!(args.simulate, Some(false));
        assert_eq!((args.display, args.calibrate, args.profile.as_deref()), (DisplayMode::Always, false, Some("Anna")));
        assert_eq!(parse(["--help".to_string()]).unwrap(), Command::Help);
        for bad in [&["--display", "sometimes"][..], &["--index"], &["--bogus"], &["--simulate-monitors"]] {
            assert!(parse(bad.iter().map(|a| a.to_string())).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn tobii_arguments_choose_the_flow() {
        let flow = |arguments: &[&str]| {
            let args = run(arguments);
            (args.flow, args.display, args.calibrate)
        };
        assert_eq!(flow(&[]), (FlowKind::Setup, DisplayMode::Auto, true));
        assert_eq!(flow(&["-C"]), (FlowKind::Recalibrate, DisplayMode::Never, true));
        assert_eq!(flow(&["-N"]), (FlowKind::NewProfile, DisplayMode::Never, true));
        assert_eq!(flow(&["-S"]), (FlowKind::DisplaySetup, DisplayMode::Always, false));
        assert_eq!(flow(&["-R"]), (FlowKind::ResolveConfiguration, DisplayMode::Auto, true));
        assert_eq!(flow(&["-G"]), (FlowKind::Guest, DisplayMode::Never, true));
        assert_eq!(flow(&["--guest"]), flow(&["-G"]));
        assert_eq!(run(&["-S", "--simulate", "unmapped", "--windowed"]).simulate, Some(false));
        let options = options_from(&run(&["--guest"]), Some(PathBuf::from("/scratch"))).unwrap();
        assert_eq!(options.flow, FlowKind::Guest);
        for bad in [&["-C", "-N"][..], &["-G", "--guest"], &["-S", "--display", "never"], &["-N", "--profile", "Anna"]]
        {
            assert!(parse(bad.iter().map(|a| a.to_string())).is_err(), "{bad:?}");
        }
        for unsupported in ["-P", "-Q", "-X"] {
            assert!(parse([unsupported.to_string()]).is_err(), "{unsupported}");
        }
    }

    #[test]
    fn a_tobii_argument_picks_tobii_exit_codes() {
        let codes = |arguments: &[&str]| exit_codes(arguments.iter().map(|a| a.to_string()));
        for legacy in [&[][..], &["--display", "always"], &["--bogus"], &["--simulate", "mapped", "--dry-run"]] {
            assert_eq!(codes(legacy), ExitCodes::Legacy, "{legacy:?}");
        }
        for tobii in [&["-C"][..], &["--guest"], &["-G"], &["-R", "--bogus"], &["-C", "-N"], &["--windowed", "-S"]] {
            assert_eq!(codes(tobii), ExitCodes::Tobii, "{tobii:?}");
        }
    }

    #[test]
    fn service_options() {
        assert_eq!(service_choice(&run(&[])), ServiceChoice::Ask(tobii_control::DEFAULT_ADDR));
        assert_eq!(tobii_control::DEFAULT_ADDR.to_string(), "127.0.0.1:7760");
        let other: SocketAddr = "127.0.0.1:7761".parse().unwrap();
        assert_eq!(service_choice(&run(&["--service", "127.0.0.1:7761"])), ServiceChoice::Ask(other));
        assert_eq!(service_choice(&run(&["--no-service"])), ServiceChoice::Skip);
        assert_eq!(service_choice(&run(&["--simulate", "mapped"])), ServiceChoice::Skip);
        for bad in [&["--service", "nowhere"][..], &["--service", "127.0.0.1:1", "--no-service"]] {
            assert!(parse(bad.iter().map(|a| a.to_string())).is_err(), "{bad:?}");
        }
    }

    fn simulated_usb(
        opened: Arc<AtomicBool>,
    ) -> impl FnOnce(usize, u32, Option<WouldSend>) -> Result<Session, DeviceError> + Send + 'static {
        move |_, _, would_send| {
            opened.store(true, Ordering::SeqCst);
            Ok(crate::device::session_on(SimulatedTracker::new(Simulation::default()), would_send))
        }
    }

    fn listener() -> (TcpListener, SocketAddr) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    fn contacted(listener: &TcpListener) -> bool {
        match listener.accept() {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("{error}"),
        }
    }

    #[test]
    fn simulate_never_contacts_service() {
        let (listener, addr) = listener();
        let args = run(&["--simulate", "mapped", "--service", &addr.to_string()]);
        let usb = |_, _, _| -> Result<Session, DeviceError> { panic!("a simulation opens no USB") };
        let (_session, handover) = session_opener(&args, Box::new(|| None), usb)().unwrap();
        assert!(handover.is_none());
        assert!(!contacted(&listener));
    }

    #[test]
    fn no_service_opens_usb_directly() {
        let opened = Arc::new(AtomicBool::new(false));
        let open = session_opener(&run(&["--no-service"]), Box::new(|| None), simulated_usb(Arc::clone(&opened)));
        let (_session, handover) = open().unwrap();
        assert!(handover.is_none() && opened.load(Ordering::SeqCst));

        let (listener, addr) = listener();
        drop(listener);
        let opened = Arc::new(AtomicBool::new(false));
        let args = run(&["--service", &addr.to_string()]);
        let started = std::time::Instant::now();
        let (_session, handover) =
            session_opener(&args, Box::new(|| None), simulated_usb(Arc::clone(&opened)))().unwrap();
        assert!(handover.is_none() && opened.load(Ordering::SeqCst), "nothing listens: USB as before");
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "{:?}", started.elapsed());
    }

    #[test]
    fn the_simulated_desktop_starts_on_the_primary_monitor() {
        let args = run(&["--simulate", "mapped", "--simulate-monitors"]);
        let desktop = choose_desktop(&args, simulated_monitors(), &serde_json::json!({}));
        let current = desktop.current_monitor().unwrap();
        assert_eq!((current.name.as_str(), current.size_mm()), ("DP-1", Some([597.0, 336.0])));
        assert!(desktop.movable);
        assert_eq!(desktop.neighbour(flow::Direction::Previous), Some(1));
        assert_eq!(desktop.neighbour(flow::Direction::Next), None);
        let portrait = &desktop.monitors[1];
        assert_eq!(portrait.size_mm(), Some([336.0, 597.0]));
    }

    #[test]
    fn the_recorded_or_named_monitor_comes_first() {
        let record = serde_json::json!({"monitor": {"key": "HDMI-1"}});
        let desktop = choose_desktop(&run(&[]), simulated_monitors(), &record);
        assert_eq!(desktop.current, Some(1));
        let desktop = choose_desktop(&run(&["--monitor", "DP-1"]), simulated_monitors(), &record);
        assert_eq!(desktop.current, Some(0));
        let single = choose_desktop(&run(&[]), simulated_monitors()[..1].to_vec(), &record);
        assert!(!single.movable);
    }

    #[test]
    fn a_simulation_keeps_its_files_in_the_scratch_directory() {
        let args = run(&["--simulate", "unmapped"]);
        let options = options_from(&args, Some(PathBuf::from("/scratch"))).unwrap();
        assert_eq!(options.profiles_directory, PathBuf::from("/scratch/profiles"));
        assert_eq!(options.display_record, PathBuf::from("/scratch/display-setup.json"));
    }
}
