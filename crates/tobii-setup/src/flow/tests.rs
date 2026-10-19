//! The flow against [`SimulatedTracker`], a scripted device following the command semantics
//! recovered from the runtime and the Tobii client (NOT device traffic), with a scripted
//! window that plays the user.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tobii_gaze::device::gaze_command as gc;
use tobii_protocol::commands::command;

use super::*;
use crate::channel::WindowEvent;
use crate::cli::{PORTRAIT_EDID, PRIMARY_EDID};
use crate::device::session_on;
use crate::screen::{Screen, View};
use crate::simulated::{ALBEDO_AREA, SimulatedTracker, SimulatedUser, Simulation, TrackerState};

const TEST_MOVE_TIMEOUT: Duration = Duration::from_millis(500);
const FULLSCREEN_VIEW: View = View {
    width: 1280.0,
    height: 720.0,
    screen_width: 1280.0,
    screen_height: 720.0,
    screen_px: [1280.0, 720.0],
    fullscreen: false,
};

/// The targets on screen that have not exploded yet: (index, point).
type ShownTargets = Arc<Mutex<Vec<(usize, [f64; 2])>>>;

/// An action for a screen, with the view the window reports it with.
type Answer = Box<dyn FnMut(&Screen) -> Option<(Action, Option<View>)> + Send>;

fn always(action: Action) -> Answer {
    Box::new(move |_| Some((action.clone(), None)))
}

fn done_with(left_delta: f64) -> Answer {
    Box::new(move |screen| match &screen.kind {
        ScreenKind::Display(lines) => Some((Action::Done { left: lines.left + left_delta, right: lines.right }, None)),
        _ => None,
    })
}

fn save_profile(name: &str) -> Answer {
    always(Action::SaveProfile { name: name.into() })
}

/// The window and the user in front of it: acknowledges every screen, answers with
/// `answers[name]`, looks at the first calibration target on screen that has not exploded
/// yet, and goes along when the setup moves to another monitor (when `follows_moves`).
struct ScriptedWindow {
    channel: UiChannel,
    answers: HashMap<&'static str, Answer>,
    view: View,
    moved_view: Option<View>,
    monitor: Option<String>,
    follows_moves: bool,
    seen: Arc<Mutex<Vec<Screen>>>,
    targets: ShownTargets,
    stopped: Arc<AtomicBool>,
}

impl ScriptedWindow {
    fn follow_targets(&self, phase: &CalibrationPhase) {
        let mut targets = self.targets.lock().unwrap();
        match phase {
            CalibrationPhase::Targets { targets: shown, .. } => {
                *targets = shown.iter().map(|&(index, x, y)| (index, [x, y])).collect();
            }
            CalibrationPhase::Pop { index } => targets.retain(|(i, _)| i != index),
            CalibrationPhase::Intro { .. } | CalibrationPhase::Restart { .. } => targets.clear(),
            _ => {}
        }
    }

    fn event(&self, seq: Option<u64>) -> WindowEvent {
        WindowEvent { seq, view: Some(self.view), monitor: self.monitor.clone(), ..WindowEvent::default() }
    }

    fn run(mut self) {
        let mut last = None;
        self.channel.window_event(self.event(None));
        while !self.stopped.load(Ordering::Relaxed) {
            let screen = self.channel.snapshot().screen;
            if Some(screen.seq) != last {
                last = Some(screen.seq);
                self.seen.lock().unwrap().push(screen.clone());
                if let ScreenKind::Calibration(phase) = &screen.kind {
                    self.follow_targets(phase);
                }
                if let ScreenKind::Moving { to } = &screen.kind
                    && self.follows_moves
                {
                    self.monitor = Some(to.clone());
                    self.view = self.moved_view.unwrap_or(self.view);
                }
                self.channel.window_event(WindowEvent { ack: true, ..self.event(Some(screen.seq)) });
                let answer = self.answers.get_mut(screen.name()).and_then(|answer| answer(&screen));
                if let Some((action, view)) = answer {
                    let mut event = self.event(Some(screen.seq));
                    event.action = Some(action);
                    if view.is_some() {
                        event.view = view;
                    }
                    self.channel.window_event(event);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

struct Harness {
    channel: UiChannel,
    tracker: Option<SimulatedTracker>,
    state: Arc<Mutex<TrackerState>>,
    options: SetupOptions,
    directory: PathBuf,
    desktop: Desktop,
    reports: Arc<Mutex<Vec<(String, Json)>>>,
    window: Option<ScriptedWindow>,
    seen: Arc<Mutex<Vec<Screen>>>,
    dry_run_held: Arc<Mutex<Vec<u32>>>,
}

fn fast(simulation: Simulation) -> Simulation {
    Simulation { collect_s: 0.05, compute_s: 0.05, user: SimulatedUser::seated_after(0.2), ..simulation }
}

fn temporary_directory() -> PathBuf {
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("tobii-setup-flow-test-{}-{count}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

impl Harness {
    fn new(simulation: Simulation, answers: Vec<(&'static str, Answer)>) -> Self {
        let channel = UiChannel::new();
        let mut tracker = SimulatedTracker::new(fast(simulation));
        let directory = temporary_directory();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let targets: ShownTargets = Arc::new(Mutex::new(Vec::new()));
        let looked_at = Arc::clone(&targets);
        tracker.looking_at =
            Some(Arc::new(move || Some(looked_at.lock().unwrap().first().map_or([0.5, 0.5], |t| t.1))));
        let mut answers: HashMap<&'static str, Answer> = answers.into_iter().collect();
        answers.entry("positioning").or_insert_with(|| always(Action::Continue));
        let window = ScriptedWindow {
            channel: channel.clone(),
            answers,
            view: FULLSCREEN_VIEW,
            moved_view: None,
            monitor: None,
            follows_moves: true,
            seen: Arc::clone(&seen),
            targets,
            stopped: Arc::new(AtomicBool::new(false)),
        };
        Self {
            channel,
            state: Arc::clone(&tracker.state),
            tracker: Some(tracker),
            options: SetupOptions::in_directory(&directory),
            directory,
            desktop: Desktop::default(),
            reports: Arc::new(Mutex::new(Vec::new())),
            window: Some(window),
            seen,
            dry_run_held: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_desktop(mut self, desktop: Desktop) -> Self {
        self.window.as_mut().unwrap().monitor = desktop.current_monitor().map(|m| m.key.clone());
        self.desktop = desktop;
        self
    }

    fn window(&mut self) -> &mut ScriptedWindow {
        self.window.as_mut().unwrap()
    }

    fn store(&self) -> ProfileStore {
        ProfileStore::new(self.options.profiles_directory.clone())
    }

    fn run(&mut self) -> SetupOutcome {
        let window = self.window.take().unwrap();
        let stopped = Arc::clone(&window.stopped);
        let page: JoinHandle<()> = std::thread::spawn(move || window.run());
        let reports = Arc::clone(&self.reports);
        let report: Report = Box::new(move |kind, values| reports.lock().unwrap().push((kind.to_string(), values)));
        let flow = SetupFlow::new(self.channel.clone(), self.options.clone(), report, self.desktop.clone())
            .with_move_timeout(TEST_MOVE_TIMEOUT);
        let tracker = self.tracker.take().unwrap();
        let held = Arc::clone(&self.dry_run_held);
        let dry_run = self.options.dry_run;
        let open: OpenSession = Box::new(move || {
            let report: Option<crate::device::WouldSend> =
                dry_run.then(|| Box::new(move |m: &tobii_protocol::Message| held.lock().unwrap().push(m.command)) as _);
            Ok((session_on(tracker, report), None))
        });
        let outcome = flow.run(open);
        let last = self.channel.current().seq;
        let _ = self.channel.wait_ack::<ChannelError>(last, Duration::from_secs(2), None);
        stopped.store(true, Ordering::Relaxed);
        page.join().unwrap();
        outcome
    }

    fn names(&self) -> Vec<&'static str> {
        self.seen.lock().unwrap().iter().map(Screen::name).collect()
    }

    fn calibration_phases(&self) -> Vec<CalibrationPhase> {
        let seen = self.seen.lock().unwrap();
        seen.iter()
            .filter_map(|s| if let ScreenKind::Calibration(p) = &s.kind { Some(p.clone()) } else { None })
            .collect()
    }

    fn commands(&self) -> Vec<u32> {
        self.state.lock().unwrap().commands()
    }

    fn reports(&self, kind: &str) -> Vec<Json> {
        self.reports.lock().unwrap().iter().filter(|(k, _)| k == kind).map(|(_, v)| v.clone()).collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

const WRITES: [u32; 6] = [
    gc::CALIBRATION_START,
    gc::CALIBRATION_CLEAR,
    gc::CALIBRATION_COLLECT_2D,
    gc::CALIBRATION_COMPUTE_AND_APPLY,
    gc::SET_DISPLAY_AREA,
    gc::CALIBRATION_APPLY,
];

fn unmapped() -> Simulation {
    Simulation { mapped: false, ..Simulation::default() }
}

#[test]
fn first_setup_runs_display_positioning_calibration_and_names_the_profile() {
    let answers =
        vec![("display", done_with(0.0)), ("display-save", always(Action::Save)), ("profile", save_profile("Garulf"))];
    let mut harness = Harness::new(unmapped(), answers);
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Finished, "{}", outcome.message);
    let names = harness.names();
    for screen in ["display", "display-save", "positioning", "calibration", "profile", "finished"] {
        assert!(names.contains(&screen), "{screen} in {names:?}");
    }
    let at = |name| names.iter().position(|n| *n == name).unwrap();
    assert!(at("display") < at("positioning") && at("positioning") < at("calibration"));
    assert!(outcome.display_written);
    assert!(ds::is_mapped(Some(&harness.state.lock().unwrap().area)));
    assert_eq!(outcome.profile.as_deref(), Some("Garulf"));
    let store = harness.store();
    assert_eq!((store.names().unwrap(), store.current().unwrap()), (vec!["Garulf".to_string()], Some("Garulf".into())));
    let tracker_id = harness.state.lock().unwrap().calibration_id;
    assert_eq!(store.load("Garulf").unwrap().calibration.calibration_id, tracker_id);
    assert!(!harness.commands().contains(&gc::CALIBRATION_APPLY));
    let display = harness.seen.lock().unwrap().iter().find_map(|s| match &s.kind {
        ScreenKind::Display(lines) => Some(lines.clone()),
        _ => None,
    });
    let display = display.unwrap();
    assert!(display.intro && !display.previous && !display.next);
    assert_eq!(display.instructions, "Move the lines to the marks on top of your eye tracker.");
}

#[test]
fn mapped_tracker_skips_the_display_and_the_intro() {
    let mut harness = Harness::new(Simulation::default(), vec![("profile", save_profile("Garulf"))]);
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Finished);
    assert!(!harness.names().contains(&"display"));
    assert!(!harness.commands().contains(&gc::SET_DISPLAY_AREA));
    let phases = harness.calibration_phases();
    assert_eq!(phases[0], CalibrationPhase::Intro { welcome: true, again: false });
    assert!(matches!(phases[1], CalibrationPhase::Targets { stage: 0, .. }));
    assert_eq!(phases.iter().filter(|p| matches!(p, CalibrationPhase::Pop { .. })).count(), 7);
    let stages: Vec<Vec<usize>> = phases
        .iter()
        .filter_map(|p| match p {
            CalibrationPhase::Targets { targets, .. } => Some(targets.iter().map(|t| t.0).collect()),
            _ => None,
        })
        .collect();
    assert_eq!(stages, [vec![0], vec![1, 2, 3], vec![4, 5, 6]]);
}

#[test]
fn unchanged_lines_write_nothing_and_ask_nothing() {
    let mut harness = Harness::new(Simulation::default(), vec![("display", done_with(0.0))]);
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Finished);
    assert!(!harness.names().contains(&"display-save"));
    assert!(!harness.commands().contains(&gc::SET_DISPLAY_AREA));
    assert!(harness.options.display_record.exists());
}

#[test]
fn done_outside_fullscreen_shows_the_lines_again_where_they_were() {
    let shown = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&shown);
    let mut count = 0;
    let display: Answer = Box::new(move |screen| {
        let ScreenKind::Display(lines) = &screen.kind else { return None };
        seen.lock().unwrap().push((lines.left, lines.right, lines.intro));
        count += 1;
        if count == 1 {
            let windowed = View { height: 600.0, ..FULLSCREEN_VIEW };
            return Some((Action::Done { left: 0.3, right: 0.6 }, Some(windowed)));
        }
        Some((Action::Done { left: lines.left, right: lines.right }, Some(FULLSCREEN_VIEW)))
    });
    let mut harness = Harness::new(unmapped(), vec![("display", display), ("display-save", always(Action::Skip))]);
    harness.options.calibrate = false;
    harness.run();
    let shown = shown.lock().unwrap();
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[1], (0.3, 0.6, false));
    assert!(shown[0].2);
}

#[test]
fn a_changed_area_is_kept_when_the_user_declines() {
    let answers = vec![("display", done_with(-0.02)), ("display-save", always(Action::Skip))];
    let mut harness = Harness::new(Simulation::default(), answers);
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    harness.run();
    assert!(harness.names().contains(&"display-save"));
    assert!(!harness.commands().contains(&gc::SET_DISPLAY_AREA));
}

#[test]
fn save_display_without_asking_writes_and_keeps_a_backup() {
    let mut harness = Harness::new(Simulation::default(), vec![("display", done_with(-0.02))]);
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    harness.options.save_display_without_asking = true;
    let outcome = harness.run();
    assert!(!harness.names().contains(&"display-save"));
    let commands = harness.commands();
    let write = commands.iter().position(|c| *c == gc::SET_DISPLAY_AREA).unwrap();
    assert_eq!(commands[write + 1], command::GET_DISPLAY_AREA);
    assert_eq!(outcome.backups.len(), 1);
    assert_eq!(ds::load_display_area(&outcome.backups[0]).unwrap(), ALBEDO_AREA.as_float32());
    assert!(outcome.display_written);
}

#[test]
fn recalibration_improves_the_current_profile_without_asking_for_a_name() {
    let mut harness = Harness::new(Simulation::default(), Vec::new());
    let calibration = {
        let state = harness.state.lock().unwrap();
        LocalCalibration { calibration_id: state.calibration_id, blob: state.blob.clone() }
    };
    let store = harness.store();
    store.save("Garulf", &calibration, false).unwrap();
    store.set_current("Garulf").unwrap();
    let outcome = harness.run();
    assert_eq!((outcome.status, outcome.profile.as_deref()), (Status::Finished, Some("Garulf")));
    assert!(!harness.names().contains(&"profile"));
    let commands = harness.commands();
    let clear = commands.iter().position(|c| *c == gc::CALIBRATION_CLEAR).unwrap();
    let apply = commands.iter().position(|c| *c == gc::CALIBRATION_APPLY).unwrap();
    assert!(clear < apply);
    let tracker_id = harness.state.lock().unwrap().calibration_id;
    assert_eq!(store.load("Garulf").unwrap().calibration.calibration_id, tracker_id);
    assert_eq!(harness.calibration_phases()[0], CalibrationPhase::Intro { welcome: false, again: false });
}

#[test]
fn all_points_failed_offers_try_again() {
    let answers = vec![("calibration-failed", always(Action::Retry)), ("profile", save_profile("Garulf"))];
    let mut harness = Harness::new(Simulation { fail_compute_at: vec![2], ..Simulation::default() }, answers);
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Finished, "{}", outcome.message);
    let names = harness.names();
    let failed = names.iter().position(|n| *n == "calibration-failed").unwrap();
    assert_eq!(names.iter().filter(|n| **n == "positioning").count(), 2);
    assert!(names.len() - failed > 3);
    assert!(harness.calibration_phases().contains(&CalibrationPhase::Intro { welcome: false, again: false }));
}

#[test]
fn cancel_after_the_calibration_started_puts_the_previous_calibration_back() {
    let simulation = Simulation { fail_compute_at: vec![2], ..Simulation::default() };
    let mut harness = Harness::new(simulation, vec![("calibration-failed", always(Action::Cancel))]);
    let before = harness.state.lock().unwrap().blob.clone();
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Cancelled);
    assert_eq!(harness.state.lock().unwrap().blob, before);
    let commands = harness.commands();
    assert_eq!(commands[commands.len() - 2], gc::CALIBRATION_APPLY);
    assert_eq!(*harness.names().last().unwrap(), "cancelled");
    let last = harness.channel.current();
    assert!(
        matches!(last.kind, ScreenKind::Cancelled { ref text, .. } if text == "Your previous calibration is back.")
    );
    assert_eq!(outcome.backups.len(), 1);
}

#[test]
fn cancel_while_positioning_changes_nothing() {
    let mut harness = Harness::new(Simulation::default(), vec![("positioning", always(Action::Cancel))]);
    harness.state.lock().unwrap().user.present = false;
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Cancelled);
    let commands = harness.commands();
    assert!(WRITES.iter().all(|write| !commands.contains(write)));
    assert_eq!(*commands.last().unwrap(), command::UNSUBSCRIBE_STREAM);
    let last = harness.channel.current();
    assert!(matches!(last.kind, ScreenKind::Cancelled { ref text, .. } if text == "Nothing was changed."));
}

#[test]
fn profile_names_are_checked_like_the_app() {
    let mut attempts = vec!["", "Guest", "a/b", "Garulf"].into_iter();
    let answer: Answer = Box::new(move |_| Some((Action::SaveProfile { name: attempts.next()?.into() }, None)));
    let mut harness = Harness::new(Simulation::default(), vec![("profile", answer)]);
    let outcome = harness.run();
    assert_eq!(outcome.profile.as_deref(), Some("Garulf"));
    let problems: Vec<String> = harness
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|s| if let ScreenKind::Profile { problem, .. } = &s.kind { Some(problem.clone()) } else { None })
        .collect();
    assert_eq!(
        problems,
        [
            "",
            "Please type a name for the profile.",
            "Guest is a reserved name.",
            "Use only letters, digits, spaces, brackets and dashes."
        ]
    );
}

#[test]
fn dry_run_sends_no_write() {
    let answers =
        vec![("display", done_with(0.0)), ("display-save", always(Action::Save)), ("profile", save_profile("Garulf"))];
    let mut harness = Harness::new(unmapped(), answers);
    harness.options.dry_run = true;
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Finished, "{}", outcome.message);
    let commands = harness.commands();
    assert!(WRITES.iter().all(|write| !commands.contains(write)), "{commands:?}");
    assert!(harness.store().names().unwrap().is_empty());
    let held = harness.dry_run_held.lock().unwrap();
    assert!(held.contains(&gc::SET_DISPLAY_AREA) && held.contains(&gc::CALIBRATION_COMPUTE_AND_APPLY));
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap()).collect()
}

/// albedo as Windows lists it: the primary monitor and the portrait one left of it.
fn albedo() -> (Monitor, Monitor) {
    let primary = Monitor {
        key: r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41221".into(),
        name: r"\\.\DISPLAY2".into(),
        bounds: Some([0, 0, 2560, 1440]),
        primary: true,
        rotation: 0,
        edid: Some(hex(PRIMARY_EDID)),
        instance_id: Some(r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41221".into()),
        connector: Some("card0-DP-1".into()),
    };
    let portrait = Monitor {
        key: r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41219".into(),
        name: r"\\.\DISPLAY1".into(),
        bounds: Some([-1440, -588, 1440, 2560]),
        primary: false,
        rotation: 90,
        edid: Some(hex(PORTRAIT_EDID)),
        instance_id: Some(r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41219".into()),
        connector: None,
    };
    (primary, portrait)
}

fn albedo_desktop(movable: bool) -> Desktop {
    let (primary, portrait) = albedo();
    Desktop { monitors: vec![portrait, primary], current: Some(1), movable }
}

fn display_screens(harness: &Harness) -> Vec<DisplayLines> {
    let seen = harness.seen.lock().unwrap();
    seen.iter().filter_map(|s| if let ScreenKind::Display(l) = &s.kind { Some(l.clone()) } else { None }).collect()
}

#[test]
fn previous_moves_to_the_portrait_monitor_and_records_it() {
    let mut moved = false;
    let display: Answer = Box::new(move |screen| {
        let ScreenKind::Display(lines) = &screen.kind else { return None };
        if moved {
            return Some((Action::Done { left: lines.left, right: lines.right }, None));
        }
        moved = true;
        Some((Action::Previous, None))
    });
    let answers = vec![("display", display), ("display-save", always(Action::Save))];
    let mut harness = Harness::new(Simulation::default(), answers).with_desktop(albedo_desktop(true));
    let portrait = View {
        width: 1440.0,
        height: 2560.0,
        screen_width: 1440.0,
        screen_height: 2560.0,
        screen_px: [1440.0, 2560.0],
        fullscreen: false,
    };
    harness.window().moved_view = Some(portrait);
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Finished, "{}", outcome.message);
    let displays = display_screens(&harness);
    assert!(displays[0].previous && !displays[0].next);
    assert!(!displays[1].previous && displays[1].next);
    assert!(harness.names().contains(&"moving"));
    assert!(harness.commands().contains(&gc::SET_DISPLAY_AREA));
    let record = load_display_record(&harness.options.display_record);
    assert_eq!(record["monitor"]["key"], r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41219");
    assert_eq!(record["display_id"], albedo().1.display_id().unwrap());
    let size = harness.reports("display_confirmed")[0]["size_mm"][0].as_f64().unwrap();
    assert!((size - 336.0).abs() < 1e-3);
}

#[test]
fn unchanged_area_on_the_mapped_monitor_records_its_display_id() {
    let mut harness =
        Harness::new(Simulation::default(), vec![("display", done_with(0.0))]).with_desktop(albedo_desktop(true));
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    let outcome = harness.run();
    assert!(!harness.commands().contains(&gc::SET_DISPLAY_AREA));
    let display_id = outcome.monitor.unwrap()["display_id"].clone();
    assert_eq!(display_id, r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41221##eId:1E6D7F5B0B220100061E#or:1#v:1.0");
}

#[test]
fn a_window_that_never_arrives_keeps_the_setup_where_it_was() {
    let mut moved = false;
    let display: Answer = Box::new(move |screen| {
        let ScreenKind::Display(lines) = &screen.kind else { return None };
        if moved {
            return Some((Action::Done { left: lines.left, right: lines.right }, None));
        }
        moved = true;
        Some((Action::Previous, None))
    });
    let mut harness =
        Harness::new(Simulation::default(), vec![("display", display)]).with_desktop(albedo_desktop(true));
    harness.window().follows_moves = false;
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    let started = std::time::Instant::now();
    let move_timeout = TEST_MOVE_TIMEOUT;
    let outcome = harness.run();
    assert!(started.elapsed() >= move_timeout);
    assert_eq!(outcome.status, Status::Finished);
    assert_eq!(display_screens(&harness).len(), 2);
    assert_eq!(outcome.monitor.unwrap()["key"], r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41221");
}

#[test]
fn a_desktop_that_cannot_move_shows_no_switcher() {
    let mut harness =
        Harness::new(Simulation::default(), vec![("display", done_with(0.0))]).with_desktop(albedo_desktop(false));
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    harness.run();
    let displays = display_screens(&harness);
    assert_eq!((displays[0].previous, displays[0].next), (false, false));
}

#[test]
fn aspect_warning_yes_stops_the_setup() {
    let (primary, _) = albedo();
    let desktop = Desktop { monitors: vec![primary], current: Some(0), movable: false };
    let mut harness =
        Harness::new(Simulation::default(), vec![("aspect-warning", always(Action::Yes))]).with_desktop(desktop);
    harness.window().view = View { screen_px: [1280.0, 1024.0], ..FULLSCREEN_VIEW };
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Cancelled);
    assert!(harness.names().contains(&"aspect-warning"));
    assert!(!harness.names().contains(&"display"));
    let last = harness.channel.current();
    assert!(
        matches!(last.kind, ScreenKind::Cancelled { ref title, .. } if title == "Change the screen resolution first")
    );
}

#[test]
fn profile_name_rules() {
    let bob = ["Bob".to_string()];
    assert_eq!(profile_name_problem("Anna (glasses)", &bob), None);
    for (name, existing) in [("   ", &[][..]), (&"x".repeat(16), &[]), ("guest", &[]), ("bob", &bob), ("a.b", &[])] {
        assert!(profile_name_problem(name, existing).is_some(), "{name}");
    }
    assert_eq!(suggested_profile_name(&[], Some("garulf")), "garulf");
    assert_eq!(suggested_profile_name(&bob, Some("garulf")), "");
    assert_eq!(suggested_profile_name(&[], Some("guest")), "");
    assert_eq!(suggested_profile_name(&[], Some("averyveryverylongname")), "averyveryverylo");
}

#[test]
fn summaries_and_record_times() {
    assert_eq!(display_summary(&ALBEDO_AREA), "Screen 597 x 336 mm (27.0 in)");
    assert_eq!(civil_time(0), [1970, 1, 1, 0, 0, 0]);
    assert_eq!(civil_time(1_792_150_200), [2026, 10, 16, 11, 30, 0]);
    assert!(valid_lines(0.02, 0.07) && !valid_lines(0.01, 0.5) && !valid_lines(0.5, 0.54));
    let statuses = [Status::Finished, Status::Cancelled, Status::Failed, Status::Running];
    assert_eq!(statuses.map(|s| ExitCodes::Legacy.status(s)), [0, 2, 1, 1]);
    assert_eq!(statuses.map(|s| ExitCodes::Tobii.status(s)), [0, 2, 3, 3], "Tobii's ConfigurationResult");
    assert_eq!((ExitCodes::Legacy.argument_error(), ExitCodes::Tobii.argument_error()), (2, 1));
}

/// The tracker's current calibration, as a profile would hold it.
fn tracker_calibration(harness: &Harness) -> LocalCalibration {
    let state = harness.state.lock().unwrap();
    LocalCalibration { calibration_id: state.calibration_id, blob: state.blob.clone() }
}

fn with_current_profile(harness: &Harness, name: &str, calibration_id: u32) {
    let store = harness.store();
    let calibration = LocalCalibration { calibration_id, ..tracker_calibration(harness) };
    store.save(name, &calibration, false).unwrap();
    store.set_current(name).unwrap();
}

#[test]
fn display_record_has_connector_and_edid() {
    let mut harness =
        Harness::new(Simulation::default(), vec![("display", done_with(0.0))]).with_desktop(albedo_desktop(true));
    harness.options.display = DisplayMode::Always;
    harness.options.calibrate = false;
    harness.run();
    let record = load_display_record(&harness.options.display_record);
    let monitor = &record["monitor"];
    assert_eq!(monitor["connector"], "card0-DP-1");
    assert_eq!(monitor["edid"], PRIMARY_EDID);
    assert_eq!(monitor["key"], r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41221");
    assert_eq!(recorded_monitor_key(&record).as_deref(), Some(r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41221"));
    let service_key = profiles::recorded_monitor_key(&harness.options.display_record);
    assert_eq!(service_key.as_deref(), Some("1E6D7F5B0B220100061E"));

    let bare = monitor_record(&Monitor { key: "HDMI-1".into(), name: "HDMI-1".into(), ..Monitor::default() });
    assert!(bare.get("connector").is_none() && bare.get("edid").is_none(), "{bare}");
}

#[test]
fn calibration_is_saved_for_the_recorded_monitor() {
    let mut harness = Harness::new(Simulation::default(), Vec::new());
    with_current_profile(&harness, "Garulf", 0x1234);
    let record = r#"{"monitor": {"key": "DP-1", "edid_id": "1E6D7F5B0B220100061E"}}"#;
    std::fs::write(&harness.options.display_record, record).unwrap();
    let outcome = harness.run();
    assert_eq!((outcome.status, outcome.profile.as_deref()), (Status::Finished, Some("Garulf")));
    let store = harness.store();
    let tracker_id = harness.state.lock().unwrap().calibration_id;
    assert_eq!(store.calibration_for("Garulf", Some("1E6D7F5B0B220100061E")).unwrap().calibration_id, tracker_id);
    assert_eq!(store.load("Garulf").unwrap().calibration.calibration_id, 0x1234, "the legacy file is kept");
}

#[test]
fn guest_saves_and_selects_guest_profile() {
    let mut harness = Harness::new(Simulation::default(), Vec::new());
    with_current_profile(&harness, "Garulf", 0x1234);
    harness.options.flow = FlowKind::Guest;
    let outcome = harness.run();
    assert_eq!(
        (outcome.status, outcome.profile.as_deref()),
        (Status::Finished, Some(profiles::GUEST)),
        "{}",
        outcome.message
    );
    assert!(!harness.names().contains(&"profile"), "no name page");
    assert!(!harness.commands().contains(&gc::CALIBRATION_APPLY), "a fresh calibration, not an improved one");
    let store = harness.store();
    let tracker_id = harness.state.lock().unwrap().calibration_id;
    assert_eq!(store.current().unwrap().as_deref(), Some(profiles::GUEST));
    assert_eq!(store.load(profiles::GUEST).unwrap().calibration.calibration_id, tracker_id);
    assert_eq!(store.tracked_eyes(profiles::GUEST).unwrap(), 0, "Tobii's guest tracks both eyes");
    assert_eq!(store.load("Garulf").unwrap().calibration.calibration_id, 0x1234);
    let pops = harness.calibration_phases().iter().filter(|p| matches!(p, CalibrationPhase::Pop { .. })).count();
    assert_eq!(pops, 7, "the 7-index three-stage sequence");

    let mut again = Harness::new(Simulation { calibration_id: 0x4321, ..Simulation::default() }, Vec::new());
    again.options = harness.options.clone();
    store.set_current("Garulf").unwrap();
    let outcome = again.run();
    assert_eq!(outcome.status, Status::Finished, "{}", outcome.message);
    let tracker_id = again.state.lock().unwrap().calibration_id;
    assert_eq!(store.load(profiles::GUEST).unwrap().calibration.calibration_id, tracker_id, "overwritten");
    assert_eq!(store.current().unwrap().as_deref(), Some(profiles::GUEST));
}

#[test]
fn new_profile_asks_for_a_name_even_with_a_current_profile() {
    let mut harness = Harness::new(Simulation::default(), vec![("profile", save_profile("Anna"))]);
    with_current_profile(&harness, "Garulf", 0x1234);
    harness.options.flow = FlowKind::NewProfile;
    let outcome = harness.run();
    assert_eq!((outcome.status, outcome.profile.as_deref()), (Status::Finished, Some("Anna")), "{}", outcome.message);
    assert!(harness.names().contains(&"profile"));
    let store = harness.store();
    assert_eq!(store.current().unwrap().as_deref(), Some("Anna"));
    assert_eq!(store.load("Garulf").unwrap().calibration.calibration_id, 0x1234);
}

#[test]
fn resolve_configuration_calibrates_only_without_a_profile() {
    let answers = || vec![("display", done_with(0.0)), ("display-save", always(Action::Save))];
    let mut complete = Harness::new(unmapped(), answers());
    with_current_profile(&complete, "Garulf", 0x1234);
    complete.options.flow = FlowKind::ResolveConfiguration;
    let outcome = complete.run();
    assert_eq!(outcome.status, Status::Finished, "{}", outcome.message);
    assert!(complete.names().contains(&"display") && !complete.names().contains(&"calibration"));

    let mut answers = answers();
    answers.push(("profile", save_profile("Anna")));
    let mut empty = Harness::new(unmapped(), answers);
    empty.options.flow = FlowKind::ResolveConfiguration;
    let outcome = empty.run();
    assert_eq!(outcome.profile.as_deref(), Some("Anna"), "{}", outcome.message);
    let names = empty.names();
    assert!(names.contains(&"display") && names.contains(&"calibration") && names.contains(&"profile"));
}

/// `-R` after a move to the portrait monitor of albedo's desktop, with Garulf calibrated by
/// `calibrated_for` (None: only the legacy NAME.setpm).
fn resolve_on_the_portrait_monitor(calibrated_for: Option<&str>) -> Harness {
    let (_, portrait) = albedo();
    let desktop = Desktop { monitors: vec![portrait], current: Some(0), movable: false };
    let mut harness =
        Harness::new(unmapped(), vec![("display", done_with(0.0)), ("display-save", always(Action::Save))])
            .with_desktop(desktop);
    with_current_profile(&harness, "Garulf", 0x1234);
    if let Some(key) = calibrated_for {
        let calibration = LocalCalibration { calibration_id: 0x5678, ..tracker_calibration(&harness) };
        harness.store().save_for("Garulf", Some(key), &calibration, false).unwrap();
    }
    harness.window().view = View { screen_px: [1440.0, 2560.0], ..FULLSCREEN_VIEW };
    harness.options.flow = FlowKind::ResolveConfiguration;
    harness
}

const PORTRAIT_KEY: &str = "1E6D7F5B5FC20300061E";

#[test]
fn resolve_calibrates_a_monitor_the_profile_has_no_calibration_for() {
    let mut harness = resolve_on_the_portrait_monitor(None);
    let outcome = harness.run();
    assert_eq!((outcome.status, outcome.profile.as_deref()), (Status::Finished, Some("Garulf")), "{}", outcome.message);
    assert!(harness.names().contains(&"display") && harness.names().contains(&"calibration"));
    let store = harness.store();
    let tracker_id = harness.state.lock().unwrap().calibration_id;
    assert_eq!(profiles::recorded_monitor_key(&harness.options.display_record).as_deref(), Some(PORTRAIT_KEY));
    assert!(store.has_calibration_for("Garulf", Some(PORTRAIT_KEY)).unwrap());
    assert_eq!(store.calibration_for("Garulf", Some(PORTRAIT_KEY)).unwrap().calibration_id, tracker_id);
    assert_eq!(store.load("Garulf").unwrap().calibration.calibration_id, 0x1234);
}

#[test]
fn resolve_ends_after_the_display_on_a_calibrated_monitor() {
    let mut harness = resolve_on_the_portrait_monitor(Some(PORTRAIT_KEY));
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Finished, "{}", outcome.message);
    assert!(harness.names().contains(&"display") && !harness.names().contains(&"calibration"));
    assert_eq!(outcome.message, "Display setup done");
}

#[test]
fn resolve_with_nothing_to_do_ends_without_a_page() {
    let mut harness = Harness::new(Simulation::default(), Vec::new());
    with_current_profile(&harness, "Garulf", 0x1234);
    harness.options.flow = FlowKind::ResolveConfiguration;
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Finished, "{}", outcome.message);
    assert!(outcome.closes_window, "the window closes as soon as the run ends");
    assert_eq!(harness.names(), ["connecting"], "no page of its own");
    assert!(!harness.commands().iter().any(|c| WRITES.contains(c)));
}

#[test]
fn resolve_without_a_display_setup_fails_even_with_a_calibrated_profile() {
    let mut harness =
        Harness::new(unmapped(), vec![("display", done_with(0.0)), ("display-save", always(Action::Skip))]);
    with_current_profile(&harness, "Garulf", 0x1234);
    harness.options.flow = FlowKind::ResolveConfiguration;
    let outcome = harness.run();
    assert_eq!(outcome.status, Status::Failed, "{}", outcome.message);
    assert!(outcome.message.contains("no display setup"), "{}", outcome.message);
}
