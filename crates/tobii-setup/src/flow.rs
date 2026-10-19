//! The guided setup of Tobii's configuration app, step by step in one window.
//!
//! The order and the rules follow Tobii.Configuration (docs/setup-flow.md):
//!
//! 1. Display setup, when the device holds no display area (Tobii: no monitor mapped yet)
//!    or when asked for: "Where is your eye tracker?" only for a first setup, then the two
//!    lines to line up with the tracker's marks.
//! 2. Eye positioning: the user's eyes in the track box from the user position guide, with
//!    Tobii's hints, until they sit well (or press a key).
//! 3. Calibration: 1 target, then 3, then 3, each stage's targets shown at once and taken
//!    in the order the user looks at them.
//! 4. The result: a new profile is named and saved; a recalibration is saved to the current
//!    profile; a guest calibration is saved as `Guest`. Calibrations are kept per monitor,
//!    under the monitor of the last display setup.
//!
//! Tobii's configuration arguments pick one part of this ([`FlowKind`]).
//!
//! Writes stay behind the user: a changed display area is sent (1440) only after the user
//! pressed Done and, unless the options allow it, confirmed saving it; the calibration is
//! changed only once positioning ended and the calibration started. Leaving after the
//! calibration started (Esc, Cancel, a failure the user does not retry, a closed window)
//! puts the previous calibration back (1110). The previous calibration and display area are
//! also saved to files first. With `dry_run` every write is reported instead of sent.
//!
//! All tracker work runs on the thread that calls [`SetupFlow::run`], never on the GTK main
//! loop; while gaze is subscribed it never pauses reading (docs/gaze-calibration.md).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value as Json, json};
use thiserror::Error;

use tobii_control::ControlError;
use tobii_device::DeviceError;
use tobii_gaze::GazeError;
use tobii_gaze::calibration::{self as cal, Calibrator, FocusCheck, UiEvent};
use tobii_gaze::device::LocalCalibration;
use tobii_gaze::display::{self as ds, DisplayArea, LineSetup, MountingGeometry};
use tobii_gaze::monitors::{self, Monitor};
use tobii_gaze::operation::{
    CalibrationUi, DriverError, LogRecord, LogValue, Operation, OperationError, run, run_calls,
};
use tobii_gaze::positioning::{PositioningGuide, guide_sample};
use tobii_gaze::profiles::{self, ProfileStore};
use tobii_protocol::commands::{STREAM_DATA_KIND, Stream, unsubscribe_stream};
use tobii_protocol::gaze::gaze_message;

use crate::channel::{ChannelError, UiChannel, Waited};
use crate::device::Session;
use crate::handover::Handover;
use crate::screen::{Action, CalibrationPhase, DisplayLines, EyesLive, ScreenKind};

pub const ACK_TIMEOUT: Duration = Duration::from_secs(15);
pub const MOVE_TIMEOUT: Duration = Duration::from_secs(30);
pub const WINDOW_FIRST_SEEN_TIMEOUT: Duration = Duration::from_secs(180);
pub const DISPLAY_RECORD: &str = "display-setup.json";
pub const READ_SLICE_MS: u32 = 100;
pub const PROFILE_NAME_LENGTH: usize = 15;
pub const DEFAULT_SCREEN_PX: [f64; 2] = [1920.0, 1080.0];

/// When the display setup runs: only for a tracker without one, always, or never.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayMode {
    #[default]
    Auto,
    Always,
    Never,
}

/// Which of Tobii's configuration flows runs (`Tobii.Configuration.exe` arguments,
/// docs/setup-flow.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlowKind {
    /// No argument: the display setup when needed, then calibrate the current profile or a
    /// new one.
    #[default]
    Setup,
    /// `-C`: improve the current profile's calibration.
    Recalibrate,
    /// `-N`: calibrate and name a new profile.
    NewProfile,
    /// `-S`: the display setup only.
    DisplaySetup,
    /// `-R`: the display setup when needed, then a calibration only without a calibrated
    /// current profile.
    ResolveConfiguration,
    /// `-G`: calibrate the `Guest` profile and make it current.
    Guest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetupOptions {
    pub flow: FlowKind,
    pub display: DisplayMode,
    pub calibrate: bool,
    pub save_display_without_asking: bool,
    pub dry_run: bool,
    pub profile: Option<String>,
    pub profiles_directory: PathBuf,
    pub backups_directory: PathBuf,
    pub size_hint_mm: Option<[f64; 2]>,
    pub display_record: PathBuf,
}

impl SetupOptions {
    /// Options with the profiles, backups and display record under `data` (normally
    /// $XDG_DATA_HOME/tobii-linux).
    pub fn in_directory(data: &Path) -> Self {
        Self {
            flow: FlowKind::Setup,
            display: DisplayMode::Auto,
            calibrate: true,
            save_display_without_asking: false,
            dry_run: false,
            profile: None,
            profiles_directory: data.join("profiles"),
            backups_directory: data.join("backups"),
            size_hint_mm: None,
            display_record: data.join(DISPLAY_RECORD),
        }
    }
}

/// $XDG_DATA_HOME/tobii-linux, where the profiles, backups and display record live.
pub fn default_data_directory() -> PathBuf {
    ProfileStore::default_location().directory.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// The monitors the window can be on, the one it is on, and whether it can move.
///
/// A desktop that cannot move, or has fewer than two monitors with known positions, keeps
/// the setup on the monitor it started on and shows no monitor switcher.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Desktop {
    pub monitors: Vec<Monitor>,
    pub current: Option<usize>,
    pub movable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Previous,
    Next,
}

impl Direction {
    pub fn name(self) -> &'static str {
        match self {
            Direction::Previous => "previous",
            Direction::Next => "next",
        }
    }
}

impl Desktop {
    pub fn current_monitor(&self) -> Option<&Monitor> {
        self.current.and_then(|index| self.monitors.get(index))
    }

    pub fn neighbour(&self, direction: Direction) -> Option<usize> {
        if !self.movable {
            return None;
        }
        match direction {
            Direction::Previous => monitors::previous_monitor(&self.monitors, self.current_monitor()),
            Direction::Next => monitors::next_monitor(&self.monitors, self.current_monitor()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Running,
    Finished,
    Cancelled,
    Failed,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Finished => "finished",
            Status::Cancelled => "cancelled",
            Status::Failed => "failed",
        }
    }
}

/// The process exit codes: tobii-linux's own, or Tobii's `ConfigurationResult` when the
/// setup was started with one of Tobii's configuration arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCodes {
    /// 0 finished, 1 failed, 2 cancelled or an argument error.
    Legacy,
    /// 0 finished, 1 argument error, 2 cancelled, 3 failed.
    Tobii,
}

impl ExitCodes {
    pub fn status(self, status: Status) -> u8 {
        match (status, self) {
            (Status::Finished, _) => 0,
            (Status::Cancelled, _) => 2,
            (Status::Running | Status::Failed, ExitCodes::Legacy) => 1,
            (Status::Running | Status::Failed, ExitCodes::Tobii) => 3,
        }
    }

    pub fn argument_error(self) -> u8 {
        match self {
            ExitCodes::Legacy => 2,
            ExitCodes::Tobii => 1,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetupOutcome {
    pub status: Status,
    pub display_written: bool,
    pub calibration_id: Option<u32>,
    pub profile: Option<String>,
    pub backups: Vec<PathBuf>,
    pub message: String,
    pub monitor: Option<Json>,
    /// The run ended without a last page (`-R` with nothing to resolve): close the window.
    pub closes_window: bool,
}

#[derive(Debug, Error)]
pub enum FlowError {
    #[error(transparent)]
    Channel(#[from] ChannelError),
    /// The user wants to change the screen resolution first (the aspect ratio warning's Yes).
    #[error("The screen resolution should be changed first")]
    ScreenSettingsWanted,
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Gaze(#[from] GazeError),
    /// A running `tobii-linux serve` did not release the tracker.
    #[error("The eye tracker service did not release the eye tracker: {0}")]
    Handover(ControlError),
    #[error("{0}")]
    Other(String),
}

impl From<OperationError<DriverError>> for FlowError {
    fn from(error: OperationError<DriverError>) -> Self {
        match error {
            OperationError::Gaze(error) => FlowError::Gaze(error),
            OperationError::External(DriverError::Device(error)) => FlowError::Device(error),
            OperationError::External(DriverError::Ui(error)) => match error.downcast::<ChannelError>() {
                Ok(error) => FlowError::Channel(*error),
                Err(error) => FlowError::Other(error.to_string()),
            },
            failed @ OperationError::Failed { .. } => FlowError::Other(failed.to_string()),
        }
    }
}

/// The window's own error behind a failed operation step, if any (a cancel, a closed window).
fn window_cause(error: &OperationError<DriverError>) -> Option<ChannelError> {
    match error {
        OperationError::External(DriverError::Ui(error)) => error.downcast_ref::<ChannelError>().cloned(),
        OperationError::Failed { cause, .. } => window_cause(cause),
        _ => None,
    }
}

/// NewProfileViewModel's checks, or `None` when the name can be saved.
pub fn profile_name_problem(name: &str, existing: &[String]) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Please type a name for the profile.");
    }
    if name.chars().count() > PROFILE_NAME_LENGTH {
        return Some("Use at most 15 characters.");
    }
    if profiles::check_name(name).is_err() {
        return Some("Use only letters, digits, spaces, brackets and dashes.");
    }
    if name.to_lowercase() == profiles::GUEST.to_lowercase() {
        return Some("Guest is a reserved name.");
    }
    if existing.iter().any(|other| other.to_lowercase() == name.to_lowercase()) {
        return Some("A profile with this name exists already.");
    }
    None
}

/// The login name, as `getpass.getuser` finds it.
pub fn login_name() -> Option<String> {
    ["LOGNAME", "USER", "LNAME", "USERNAME"].iter().find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
}

/// The login name as the first profile's name, when it is a valid one and no profile exists.
pub fn suggested_profile_name(existing: &[String], login: Option<&str>) -> String {
    let Some(login) = login else {
        return String::new();
    };
    let name: String = login.chars().take(PROFILE_NAME_LENGTH).collect();
    if !existing.is_empty() || profile_name_problem(&name, existing).is_some() {
        return String::new();
    }
    name
}

pub fn display_summary(area: &DisplayArea) -> String {
    let (width, height) = (area.width_mm(), area.height_mm());
    let inches = (width * width + height * height).sqrt() / 25.4;
    format!("Screen {width:.0} x {height:.0} mm ({inches:.1} in)")
}

/// ScreenPlaneSetupViewModel's limits: both lines in 0.02..0.98 and at least 0.05 apart.
pub fn valid_lines(left: f64, right: f64) -> bool {
    ds::MIN_LINE <= left && right <= ds::MAX_LINE && right - left >= ds::MIN_LINE_DISTANCE - 1e-12
}

/// The monitor and display area of the last display setup, or an empty object.
pub fn load_display_record(path: &Path) -> Json {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Json::is_object)
        .unwrap_or_else(|| json!({}))
}

fn recorded_monitor_key(record: &Json) -> Option<String> {
    record.get("monitor")?.get("key")?.as_str().map(str::to_string)
}

/// The monitor as the display record keeps it; `connector` and `edid` (hex) only when known.
pub fn monitor_record(monitor: &Monitor) -> Json {
    let mut record = json!({
        "key": monitor.key,
        "name": monitor.name,
        "bounds": monitor.bounds,
        "primary": monitor.primary,
        "rotation": monitor.rotation,
        "instance_id": monitor.instance_id,
        "edid_id": monitor.edid_id(),
        "size_mm": monitor.size_mm(),
        "display_id": monitor.display_id(),
    });
    if let Some(connector) = &monitor.connector {
        record["connector"] = json!(connector);
    }
    if let Some(edid) = &monitor.edid {
        record["edid"] = json!(edid.iter().map(|b| format!("{b:02x}")).collect::<String>());
    }
    record
}

/// UTC seconds since the epoch as (year, month, day, hour, minute, second).
fn civil_time(seconds: u64) -> [u64; 6] {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    [year as u64, month as u64, day as u64, rest / 3600, rest / 60 % 60, rest % 60]
}

fn now_utc() -> [u64; 6] {
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil_time(seconds)
}

/// Host-side note of where the display setup was made: the monitor (and its display id) and the area.
pub fn save_display_record(path: &Path, monitor: Option<&Monitor>, area: &DisplayArea) -> std::io::Result<Json> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let [year, month, day, hour, minute, second] = now_utc();
    let record = json!({
        "monitor": monitor.map(monitor_record),
        "display_args": area.floats().to_vec(),
        "display_id": monitor.and_then(Monitor::display_id),
        "saved": format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
    });
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    let temporary = path.with_file_name(name);
    let text = serde_json::to_string_pretty(&record).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, text + "\n")?;
    std::fs::rename(&temporary, path)?;
    Ok(record)
}

/// Progress records as JSON lines: a kind and its values.
pub type Report = Box<dyn FnMut(&str, Json) + Send>;
/// Opens the tracker session the flow runs on, with the service's handover when it lent the
/// tracker.
pub type OpenSession = Box<dyn FnOnce() -> Result<(Session, Option<Handover>), FlowError> + Send>;

fn log_value(value: &LogValue) -> Json {
    match value {
        LogValue::Int(n) => json!(n),
        LogValue::Float(x) => json!(x),
        LogValue::Bool(b) => json!(b),
        LogValue::Text(text) => json!(text),
        LogValue::StimulusPoints(points) => json!(points.len()),
    }
}

fn log_record(record: &LogRecord) -> Json {
    Json::Object(record.values.iter().map(|(name, value)| ((*name).to_string(), log_value(value))).collect())
}

/// Calibrator callbacks as calibration screens; every event waits until the window drew it.
#[derive(Debug)]
struct WindowCalibrationUi {
    channel: UiChannel,
    welcome: bool,
}

type UiResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

impl CalibrationUi for WindowCalibrationUi {
    fn event(&mut self, event: &UiEvent) -> UiResult {
        let phase = match event {
            UiEvent::CalibrationStarted { attempt } => {
                CalibrationPhase::Intro { welcome: self.welcome && *attempt == 0, again: *attempt > 0 }
            }
            UiEvent::ShowTargets { stage, targets } => {
                CalibrationPhase::Targets { stage: *stage, targets: targets.clone() }
            }
            UiEvent::ShowPoint { index, x, y } => {
                CalibrationPhase::Targets { stage: 0, targets: vec![(*index, *x, *y)] }
            }
            UiEvent::Focus { index, focused } => CalibrationPhase::Focus { index: *index, focused: *focused },
            UiEvent::PointDone { index, accepted: true } => CalibrationPhase::Pop { index: *index },
            UiEvent::PointDone { index, accepted: false } => CalibrationPhase::Missed { index: *index },
            UiEvent::Computing { stage } => CalibrationPhase::Computing { stage: *stage },
            UiEvent::StageDone { stage } => CalibrationPhase::Computed { stage: *stage },
            UiEvent::Restarting { attempt } => CalibrationPhase::Restart { attempt: *attempt },
        };
        let seq = self.channel.show(ScreenKind::Calibration(phase));
        self.channel.wait_ack::<ChannelError>(seq, ACK_TIMEOUT, None)?;
        Ok(())
    }

    fn check(&mut self) -> UiResult {
        Ok(self.channel.check_cancel()?)
    }
}

/// Runs the setup steps against one tracker session.
pub struct SetupFlow {
    channel: UiChannel,
    desktop: Desktop,
    options: SetupOptions,
    report: Report,
    outcome: SetupOutcome,
    store: ProfileStore,
    session: Option<Session>,
    handover: Option<Handover>,
    calibration_before: Option<LocalCalibration>,
    move_timeout: Duration,
}

impl std::fmt::Debug for SetupFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupFlow").field("options", &self.options).field("outcome", &self.outcome).finish()
    }
}

type FlowResult<T> = Result<T, FlowError>;

impl SetupFlow {
    pub fn new(channel: UiChannel, options: SetupOptions, report: Report, desktop: Desktop) -> Self {
        let store = ProfileStore::new(options.profiles_directory.clone());
        Self {
            channel,
            desktop,
            options,
            report,
            outcome: SetupOutcome::default(),
            store,
            session: None,
            handover: None,
            calibration_before: None,
            move_timeout: MOVE_TIMEOUT,
        }
    }

    /// How long a move to another monitor may take before the setup stays where it was.
    pub fn with_move_timeout(mut self, timeout: Duration) -> Self {
        self.move_timeout = timeout;
        self
    }

    pub fn run(mut self, open_session: OpenSession) -> SetupOutcome {
        let result = open_session().and_then(|(session, handover)| {
            self.session = Some(session);
            self.handover = handover;
            self.steps()
        });
        match result {
            Ok(()) => {}
            Err(FlowError::Channel(ChannelError::Cancelled)) => self.finish_cancelled("", None, None),
            Err(FlowError::ScreenSettingsWanted) => self.finish_cancelled(
                "",
                Some("Change the screen resolution first"),
                Some("Set this screen to its native resolution, then run the setup again."),
            ),
            Err(FlowError::Channel(gone @ ChannelError::WindowGone(_))) => {
                self.finish_cancelled(&gone.to_string(), None, None);
            }
            Err(error) => {
                log::error!("setup failed: {error}");
                self.fail(&error);
            }
        }
        drop(self.session.take());
        drop(self.handover.take());
        self.outcome
    }

    fn report(&mut self, kind: &str, values: Json) {
        (self.report)(kind, values);
    }

    fn session(&mut self) -> &mut Session {
        self.session.as_mut().expect("the flow runs on an open session")
    }

    fn calls<T>(&mut self, operation: Operation<T, DriverError>) -> FlowResult<T> {
        Ok(run_calls(operation, self.session())?)
    }

    fn steps(&mut self) -> FlowResult<()> {
        self.channel.show(ScreenKind::Connecting);
        self.wait_for_window()?;
        let area = self.calls(Operation::new(ds::read_display_area))?;
        let mounting = self.calls(Operation::new(ds::read_mounting))?;
        let display_mm = [round1(area.width_mm()), round1(area.height_mm())];
        self.report("device", json!({ "display_mm": display_mm, "mapped": ds::is_mapped(Some(&area)) }));
        let wanted = self.options.display;
        let display_runs =
            wanted == DisplayMode::Always || (wanted == DisplayMode::Auto && !ds::is_mapped(Some(&area)));
        let area = if display_runs { self.display_step(area, &mounting)? } else { area };
        if !self.options.calibrate {
            self.finish("Display setup done", &display_summary(&area));
            return Ok(());
        }
        if !ds::is_mapped(Some(&area)) {
            return Err(FlowError::Other("The eye tracker has no display setup; run the display setup first".into()));
        }
        if self.configuration_complete()? {
            if display_runs {
                self.finish("Display setup done", &display_summary(&area));
            } else {
                self.outcome.status = Status::Finished;
                self.outcome.closes_window = true;
            }
            return Ok(());
        }
        self.positioning_step(self.outcome.display_written)?;
        let (outcome, profile) = self.calibration_step(&area)?;
        self.result_step(outcome.calibration, profile)
    }

    /// `-R` after the display: the current profile has a calibration for the monitor of the
    /// display setup as it stands now, so nothing is left.
    fn configuration_complete(&self) -> FlowResult<bool> {
        if self.options.flow != FlowKind::ResolveConfiguration {
            return Ok(false);
        }
        let Some(current) = self.store.current()? else { return Ok(false) };
        let monitor_key = profiles::recorded_monitor_key(&self.options.display_record);
        Ok(self.store.has_calibration_for(&current, monitor_key.as_deref())?)
    }

    fn wait_for_window(&mut self) -> FlowResult<()> {
        let channel = self.channel.clone();
        let mut ready = || channel.view().map(drop);
        self.channel.wait_action::<(), FlowError>(
            |_| false,
            Some(WINDOW_FIRST_SEEN_TIMEOUT),
            None,
            Some(&mut ready),
        )?;
        Ok(())
    }

    fn screen_px(&self) -> [f64; 2] {
        self.channel.view().map_or(DEFAULT_SCREEN_PX, |view| view.screen_px)
    }

    fn wait_for(&self, accept: impl Fn(&Action) -> bool) -> FlowResult<Action> {
        match self.channel.wait_action::<(), FlowError>(accept, None, None, None)? {
            Waited::Action(action) => Ok(action),
            Waited::Ready(()) => unreachable!("no ready condition"),
        }
    }

    // Display ----------------------------------------------------------------------------

    /// Whether the device's area belongs to the current monitor (Tobii keeps one mapping per monitor).
    fn mapped_here(&self, area: &DisplayArea) -> bool {
        if !ds::is_mapped(Some(area)) {
            return false;
        }
        let Some(current) = self.desktop.current_monitor() else {
            return true;
        };
        recorded_monitor_key(&load_display_record(&self.options.display_record)).is_none_or(|key| key == current.key)
    }

    fn size_hint(&self) -> Option<[f64; 2]> {
        self.options.size_hint_mm.or_else(|| self.desktop.current_monitor().and_then(Monitor::size_mm))
    }

    /// CheckResolutionAndWarnIfNotNative, before the lines: Yes stops the setup, No goes on.
    fn check_aspect(&mut self) -> FlowResult<()> {
        let screen_px = self.screen_px();
        let Some(monitor) = self.desktop.current_monitor() else {
            return Ok(());
        };
        if monitors::has_recommended_aspect(monitor, Some(screen_px)) {
            return Ok(());
        }
        self.channel.show(ScreenKind::AspectWarning);
        if self.wait_for(|a| matches!(a, Action::Yes | Action::No))? == Action::Yes {
            return Err(FlowError::ScreenSettingsWanted);
        }
        Ok(())
    }

    /// Shows the setup on the previous or next monitor; false when the window did not get there.
    fn move_to(&mut self, direction: Direction) -> FlowResult<bool> {
        let Some(target) = self.desktop.neighbour(direction) else {
            return Ok(false);
        };
        let key = self.desktop.monitors[target].key.clone();
        let previous = self.channel.monitor();
        self.report("move_monitor", json!({ "direction": direction.name(), "to": key }));
        self.channel.follow_monitor(Some(key.clone()));
        self.channel.show(ScreenKind::Moving { to: key.clone() });
        let channel = self.channel.clone();
        let mut arrived = || (channel.window_monitor().as_deref() == Some(key.as_str())).then_some(());
        match self.channel.wait_action::<(), FlowError>(|_| false, Some(self.move_timeout), None, Some(&mut arrived)) {
            Ok(_) => {
                self.desktop.current = Some(target);
                Ok(true)
            }
            Err(FlowError::Channel(ChannelError::Timeout(error))) => {
                self.report("move_failed", json!({ "to": key, "error": error }));
                self.channel.follow_monitor(previous);
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn display_step(&mut self, mut area: DisplayArea, mounting: &MountingGeometry) -> FlowResult<DisplayArea> {
        loop {
            self.check_aspect()?;
            let mapped_here = self.mapped_here(&area);
            let lines =
                ds::initial_lines(mounting, mapped_here.then_some(&area), self.size_hint(), Some(self.screen_px()));
            let monitor = self.desktop.current_monitor().map(|m| m.key.clone());
            let values =
                json!({ "left": lines.left, "right": lines.right, "source": lines.source, "monitor": monitor });
            self.report("initial_lines", values);
            let (mut left, mut right, mut intro) = (lines.left, lines.right, !ds::is_mapped(Some(&area)));
            let moved = loop {
                self.channel.show(ScreenKind::Display(DisplayLines {
                    intro,
                    left,
                    right,
                    aspect: lines.aspect,
                    tracker_width_mm: mounting.width_mm,
                    picture_proportion: ds::picture_proportion(mounting),
                    instructions: ds::line_instructions(mounting),
                    previous: self.desktop.neighbour(Direction::Previous).is_some(),
                    next: self.desktop.neighbour(Direction::Next).is_some(),
                }));
                let action = self.wait_for(|a| matches!(a, Action::Done { .. } | Action::Previous | Action::Next))?;
                let (done_left, done_right) = match action {
                    Action::Done { left, right } => (left, right),
                    Action::Previous => {
                        if self.move_to(Direction::Previous)? {
                            break true;
                        }
                        continue;
                    }
                    _ => {
                        if self.move_to(Direction::Next)? {
                            break true;
                        }
                        continue;
                    }
                };
                (left, right) = ds::clamp_lines(done_left, done_right);
                if valid_lines(left, right) && self.channel.fullscreen() {
                    break false;
                }
                intro = false;
            };
            if !moved {
                area = self.confirm_display(area, mounting, &lines, left, right, mapped_here)?;
                return Ok(area);
            }
        }
    }

    fn confirm_display(
        &mut self,
        area: DisplayArea,
        mounting: &MountingGeometry,
        lines: &LineSetup,
        left: f64,
        right: f64,
        mapped_here: bool,
    ) -> FlowResult<DisplayArea> {
        let (size, offset, wanted) = ds::area_from_lines(left, right, lines.aspect, mounting)?;
        let values = json!({ "left": left, "right": right, "size_mm": size, "tracker_offset_mm": offset });
        self.report("display_confirmed", values);
        if mapped_here && wanted.almost_equals(&area, 0.1) {
            self.record_display(&area)?;
            return Ok(area);
        }
        if !self.options.save_display_without_asking {
            self.channel.show(ScreenKind::DisplaySave {
                summary: display_summary(&wanted),
                offset_mm: round1(offset),
                first: !ds::is_mapped(Some(&area)),
            });
            if self.wait_for(|a| matches!(a, Action::Save | Action::Skip))? == Action::Skip {
                return Ok(area);
            }
        }
        self.channel.show(ScreenKind::Busy { title: "Saving the display setup...".into() });
        self.backup_display_area(&area)?;
        let stored = self.calls(Operation::new(move |io| ds::apply_display_area(io, wanted)))?;
        self.outcome.display_written = !self.options.dry_run;
        let values = json!({ "dry_run": self.options.dry_run, "stored_matches": stored.almost_equals(&wanted, 0.1) });
        self.report("display_written", values);
        if self.options.dry_run {
            return Ok(wanted);
        }
        self.record_display(&stored)?;
        Ok(stored)
    }

    fn record_display(&mut self, area: &DisplayArea) -> FlowResult<()> {
        if self.options.dry_run {
            return Ok(());
        }
        let path = self.options.display_record.clone();
        let record = save_display_record(&path, self.desktop.current_monitor(), area).map_err(GazeError::Io)?;
        self.outcome.monitor = record.get("monitor").filter(|m| !m.is_null()).cloned();
        let values = json!({ "path": path.display().to_string(), "display_id": record["display_id"] });
        self.report("display_record", values);
        Ok(())
    }

    fn backup_path(&self, kind: &str) -> FlowResult<PathBuf> {
        std::fs::create_dir_all(&self.options.backups_directory).map_err(GazeError::Io)?;
        let [year, month, day, hour, minute, second] = now_utc();
        let name = format!("{kind}-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}.setpm");
        Ok(self.options.backups_directory.join(name))
    }

    fn backup_display_area(&mut self, area: &DisplayArea) -> FlowResult<()> {
        if !ds::is_mapped(Some(area)) {
            return Ok(());
        }
        let path = ds::save_display_area(&self.backup_path("display-area")?, area, false)?;
        self.outcome.backups.push(path);
        Ok(())
    }

    // Positioning -------------------------------------------------------------------------

    fn positioning_step(&mut self, sit_upright: bool) -> FlowResult<()> {
        self.session().subscribe(Stream::Gaze)?;
        let result = self.position_user(sit_upright);
        self.channel.set_eyes(None);
        let unsubscribed = self.session().try_call(|id| unsubscribe_stream(id, Stream::Gaze.index()));
        result?;
        unsubscribed?;
        Ok(())
    }

    fn position_user(&mut self, sit_upright: bool) -> FlowResult<()> {
        let guide = RefCell::new(PositioningGuide::new());
        let latest_us = Cell::new(None);
        let gaze_id = Stream::Gaze.wire_id();
        let channel = self.channel.clone();
        let session = self.session.as_mut().expect("the flow runs on an open session");
        let mut read = || -> FlowResult<()> {
            for message in session.receive(READ_SLICE_MS)? {
                if message.kind != STREAM_DATA_KIND || message.command != gaze_id {
                    continue;
                }
                if let Some(g) = gaze_message(&message).map_err(GazeError::from)? {
                    guide.borrow_mut().update(&guide_sample(&g));
                    latest_us.set(Some(g.timestamp));
                }
            }
            let guide = guide.borrow();
            channel.set_eyes(Some(EyesLive { eyes: guide.calculator.eyes(), hint: guide.hint, good: guide.good() }));
            Ok(())
        };
        let mut ready = || {
            if !channel.fullscreen() || !guide.borrow().good() {
                return None;
            }
            let pressed = channel.has_action(|a| *a == Action::Continue);
            let advance = latest_us.get().is_some_and(|now| guide.borrow().ready_to_advance(now));
            (pressed || advance).then_some(())
        };
        channel.show(ScreenKind::Positioning { sit_upright, leaving: false });
        channel.wait_action::<(), FlowError>(|_| false, None, Some(&mut read), Some(&mut ready))?;
        let seq = channel.show(ScreenKind::Positioning { sit_upright, leaving: true });
        channel.wait_ack::<FlowError>(seq, ACK_TIMEOUT, Some(&mut read))
    }

    // Calibration -------------------------------------------------------------------------

    fn calibration_step(&mut self, area: &DisplayArea) -> FlowResult<(cal::CalibrationOutcome, Option<String>)> {
        let profile = match (self.options.flow, &self.options.profile) {
            (FlowKind::Guest, _) => Some(profiles::GUEST.to_string()),
            (FlowKind::NewProfile, _) => None,
            (_, Some(name)) => Some(name.clone()),
            (_, None) => self.store.current()?,
        };
        let recalibration = self.options.flow != FlowKind::Guest
            && profile.as_ref().is_some_and(|name| self.store.names().is_ok_and(|n| n.contains(name)));
        let before = self.calls(Operation::new(|io| async move { cal::backup_calibration(&io).await }))?;
        let path = cal::save_backup(&self.backup_path("calibration")?, &before, false)?;
        self.outcome.backups.push(path);
        self.calibration_before = Some(before.clone());
        let targets = cal::calibration_targets(Some([area.width_mm(), area.height_mm()]), &cal::INTERACTIVE_POINTS);
        let mut welcome = !recalibration;
        loop {
            let calibrator = Calibrator {
                targets: targets.clone(),
                previous: recalibration.then(|| before.clone()),
                focus: Some(FocusCheck { screen_px: self.screen_px() }),
                choose_by_gaze: true,
                restarts: 0,
                ..Calibrator::default()
            };
            let (operation, _progress) = calibrator.operation();
            let mut ui = WindowCalibrationUi { channel: self.channel.clone(), welcome };
            let report = &mut self.report;
            let mut log = |record: &LogRecord| report(record.kind, log_record(record));
            let session = self.session.as_mut().expect("the flow runs on an open session");
            let result = run(operation, session, &mut ui, &mut log, &mut Vec::new());
            let error = match result {
                Ok(outcome) => {
                    let keep = recalibration || self.options.flow == FlowKind::Guest;
                    return Ok((outcome, profile.filter(|_| keep)));
                }
                Err(error) => error,
            };
            if let Some(cause) = window_cause(&error) {
                return Err(cause.into());
            }
            let OperationError::Failed { step, cause } = error else {
                return Err(error.into());
            };
            if self.session().is_dead() {
                return Err(FlowError::Other(format!("{step} failed: {cause}")));
            }
            self.report("calibration_failed", json!({ "step": step, "error": cause.to_string() }));
            self.channel.show(ScreenKind::CalibrationFailed);
            self.wait_for(|a| *a == Action::Retry)?;
            welcome = false;
        }
    }

    fn result_step(&mut self, calibration: LocalCalibration, profile: Option<String>) -> FlowResult<()> {
        let profile = match profile {
            Some(profile) => profile,
            None => self.name_profile()?,
        };
        let monitor_key = profiles::recorded_monitor_key(&self.options.display_record);
        if !self.options.dry_run {
            self.store.save_for(&profile, monitor_key.as_deref(), &calibration, true)?;
            if self.options.flow == FlowKind::Guest {
                self.store.set_tracked_eyes(&profile, 0)?;
            }
            self.store.set_current(&profile)?;
        }
        self.outcome.calibration_id = Some(calibration.calibration_id);
        self.outcome.profile = Some(profile.clone());
        self.calibration_before = None;
        let values = json!({
            "profile": profile,
            "calibration_id": format!("{:#x}", calibration.calibration_id),
            "monitor_key": monitor_key,
            "dry_run": self.options.dry_run,
        });
        self.report("saved", values);
        self.finish("Calibration successful!", &format!("Saved as profile \"{profile}\"."));
        Ok(())
    }

    fn name_profile(&mut self) -> FlowResult<String> {
        let existing = self.store.names()?;
        let suggested = suggested_profile_name(&existing, login_name().as_deref());
        self.channel.show(ScreenKind::Profile { suggested, problem: String::new() });
        loop {
            let Action::SaveProfile { name } = self.wait_for(|a| matches!(a, Action::SaveProfile { .. }))? else {
                unreachable!("only profile names are accepted");
            };
            let name = name.trim().to_string();
            match profile_name_problem(&name, &existing) {
                None => return Ok(name),
                Some(problem) => {
                    self.channel.show(ScreenKind::Profile { suggested: name, problem: problem.into() });
                }
            }
        }
    }

    // Endings -----------------------------------------------------------------------------

    fn restore_calibration(&mut self) -> FlowResult<bool> {
        let Some(before) = self.calibration_before.take() else {
            return Ok(false);
        };
        if self.session.as_ref().is_none_or(Session::is_dead) {
            return Ok(false);
        }
        self.channel.show(ScreenKind::Busy { title: "Putting your previous calibration back...".into() });
        let calibration_id = before.calibration_id;
        let restore = Operation::new(move |io| async move { cal::restore_calibration(&io, &before).await });
        self.calls(restore)?;
        if !self.options.dry_run {
            self.report("restored", json!({ "calibration_id": format!("{calibration_id:#x}") }));
        }
        Ok(true)
    }

    fn finish(&mut self, title: &str, text: &str) {
        self.outcome.status = Status::Finished;
        self.outcome.message = title.to_string();
        self.channel.show(ScreenKind::Finished { title: title.into(), text: text.into() });
    }

    fn finish_cancelled(&mut self, reason: &str, title: Option<&str>, text: Option<&str>) {
        self.channel.clear_cancel();
        let restored = self.safely_restore();
        let title = title.unwrap_or("Setup cancelled");
        self.outcome.status = Status::Cancelled;
        self.outcome.message = if reason.is_empty() { title.to_string() } else { reason.to_string() };
        let text = text.map_or_else(|| self.what_was_kept(restored).to_string(), str::to_string);
        self.channel.show(ScreenKind::Cancelled { title: title.into(), text });
    }

    fn what_was_kept(&self, restored: bool) -> &'static str {
        if restored {
            "Your previous calibration is back."
        } else if self.outcome.display_written {
            "The new display setup was kept."
        } else {
            "Nothing was changed."
        }
    }

    fn fail(&mut self, error: &FlowError) {
        let restored = self.safely_restore();
        self.outcome.status = Status::Failed;
        self.outcome.message = error.to_string();
        let text = format!("{error}{}", if restored { " Your previous calibration is back." } else { "" });
        self.channel.show(ScreenKind::Error { title: "Something went wrong".into(), text });
    }

    fn safely_restore(&mut self) -> bool {
        match self.restore_calibration() {
            Ok(restored) => restored,
            Err(error) => {
                log::warn!("could not restore the previous calibration: {error}");
                self.report("restore_failed", json!({ "error": error.to_string() }));
                false
            }
        }
    }
}

fn round1(value: f64) -> f64 {
    tobii_gaze::math::round_digits(value, 1)
}

#[cfg(test)]
mod tests;
