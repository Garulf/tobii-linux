//! The app's single instance and Tobii's quit flow, run in this process as the primary against
//! a stub service and a fake service unit, with the real binary as the second instance. GApplication uniqueness needs a session bus and
//! GTK a display: `make experience-test` runs this under `dbus-run-session` and Xvfb; without
//! either it is skipped. It runs on the main thread (no test harness), as GTK wants.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use tobii_control::protocol::{
    FRAME_CONTROL_EVENT, FRAME_CONTROL_HELLO, FRAME_CONTROL_REPLY, FRAME_CONTROL_REQUEST, decode_request, encode,
    split_frame,
};
use tobii_control::{Event, Reply, Status};
use tobii_experience::Settings;
use tobii_experience::app::{Deps, PAGE_TITLE, PAGE_WIDTH, Shell};
use tobii_experience::guest::{LaunchError, OnExit, SetupLauncher};
use tobii_experience::quit::{CANCEL_TEXT, CHECKBOX_TEXT, CONFIRM_TEXT, FAILURE_CAPTION, ServiceUnit};
use tobii_experience::status::DeviceStatus;
use tobii_gaze::profiles::ProfileStore;

const BINARY: &str = env!("CARGO_BIN_EXE_tobii-linux-experience");
const UNIT: &str = "tobii-instance-test.service";
const DEADLINE: Duration = Duration::from_secs(30);

fn tracking() -> Status {
    Status {
        attached: true,
        enabled: true,
        released: false,
        display_area: true,
        profile: Some("Garulf".into()),
        calibration_matches_profile: true,
    }
}

/// A control service that greets every connection with a tracking status and answers every
/// request with it, counting the connections it accepted and the ones still open.
struct StubService {
    addr: SocketAddr,
    accepted: Arc<AtomicUsize>,
    open: Arc<AtomicUsize>,
}

fn stub_service() -> StubService {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(AtomicUsize::new(0));
    let (counted, still_open) = (accepted.clone(), open.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::AcqRel);
            still_open.fetch_add(1, Ordering::AcqRel);
            let closed = still_open.clone();
            std::thread::spawn(move || {
                serve(stream);
                closed.fetch_sub(1, Ordering::AcqRel);
            });
        }
    });
    StubService { addr, accepted, open }
}

fn serve(mut stream: TcpStream) {
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(read @ 1..) = stream.read(&mut chunk) {
        received.extend_from_slice(&chunk[..read]);
        while let Some((kind, payload, used)) = split_frame(&received).unwrap() {
            received.drain(..used);
            let frame = if kind == FRAME_CONTROL_HELLO {
                encode(FRAME_CONTROL_EVENT, &Event::Status(tracking())).unwrap()
            } else if kind == FRAME_CONTROL_REQUEST {
                let (id, _) = decode_request(&payload).unwrap();
                encode(FRAME_CONTROL_REPLY, &Reply { id, result: Ok(tracking()) }).unwrap()
            } else {
                continue;
            };
            if stream.write_all(&frame).is_err() {
                return;
            }
        }
    }
}

/// Records the unit's starts and stops; the first `failing_stops` stops fail.
#[derive(Debug, Default)]
struct FakeUnit {
    calls: Mutex<Vec<String>>,
    failing_stops: AtomicUsize,
}

impl ServiceUnit for FakeUnit {
    fn start(&self, unit: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("start {unit}"));
        Ok(())
    }

    fn stop(&self, unit: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("stop {unit}"));
        let fail = self.failing_stops.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)).is_ok();
        if fail { Err("Job for the unit failed".into()) } else { Ok(()) }
    }
}

#[derive(Debug, Default)]
struct FakeSetup {
    runs: RefCell<Vec<Vec<String>>>,
}

impl SetupLauncher for FakeSetup {
    fn launch(&self, arguments: &[&str], on_exit: OnExit) -> Result<(), LaunchError> {
        self.runs.borrow_mut().push(arguments.iter().map(|argument| argument.to_string()).collect());
        on_exit(Ok(ExitStatus::from_raw(0)));
        Ok(())
    }
}

/// The steps the test walks through on the primary's main loop.
enum Step {
    WaitForTracking,
    Guest(Child),
    Bare(Child),
    Page(Child),
    Dialog,
    FirstFailure,
    Cleared,
    SecondFailure,
    Quitting,
}

fn second_instance(arguments: &[&str]) -> Child {
    Command::new(BINARY).args(arguments).args(["--service-unit", "never-started.service"]).spawn().unwrap()
}

/// The exit status of a second instance once it ended, failing the test if it hangs.
fn ended(child: &mut Child, started: Instant) -> Result<Option<ExitStatus>, String> {
    match child.try_wait().map_err(|error| error.to_string())? {
        Some(status) => Ok(Some(status)),
        None if started.elapsed() > Duration::from_secs(10) => Err("the second instance did not exit".into()),
        None => Ok(None),
    }
}

fn windows_titled(app: &gtk::Application, title: &str, page: Option<&gtk::Window>) -> Vec<gtk::Window> {
    app.windows()
        .into_iter()
        .filter(|window| window.title().as_deref() == Some(title) && Some(window) != page)
        .collect()
}

fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut found = vec![widget.clone()];
    let mut child = widget.first_child();
    while let Some(current) = child {
        found.extend(descendants(&current));
        child = current.next_sibling();
    }
    found
}

fn button(window: &gtk::Window, label: &str) -> Option<gtk::Button> {
    descendants(window.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.label().as_deref() == Some(label))
}

/// The trash button in the page's profile row of `name`.
fn trash_button(window: &gtk::Window, name: &str) -> Option<gtk::Button> {
    let has_name = |widget: &gtk::Widget| {
        descendants(widget).iter().any(|w| w.downcast_ref::<gtk::Label>().is_some_and(|l| l.label() == name))
    };
    let row = descendants(window.upcast_ref())
        .into_iter()
        .find(|widget| widget.has_css_class("tobii-profile-row") && has_name(widget))?;
    descendants(&row)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.has_css_class("tobii-trash"))
}

fn checkbox(window: &gtk::Window) -> Option<gtk::CheckButton> {
    descendants(window.upcast_ref()).into_iter().find_map(|widget| widget.downcast::<gtk::CheckButton>().ok())
}

#[derive(Debug, Clone, PartialEq)]
struct Outcome {
    forwarding: Result<(), String>,
    quit: Result<(), String>,
    shutdown: Result<(), String>,
}

fn run_primary() -> Outcome {
    let directory = std::env::temp_dir().join(format!("tobii-experience-instance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let settings_path: PathBuf = directory.join("experience.json");

    let profiles = ProfileStore::new(directory.join("profiles"));
    std::fs::create_dir_all(&profiles.directory).unwrap();
    for name in ["Garulf", "Alice"] {
        std::fs::write(profiles.directory.join(format!("{name}.setpm")), b"").unwrap();
    }
    profiles.set_current("Garulf").unwrap();
    let unit = Arc::new(FakeUnit::default());
    let setup = Rc::new(FakeSetup::default());
    let shell = Shell::new(Deps {
        service: unit.clone(),
        setup: setup.clone(),
        settings_path: Some(settings_path.clone()),
        tray: true,
        profiles: profiles.clone(),
    });

    let not_reached = || Err::<(), String>("not reached".into());
    let outcome =
        Rc::new(RefCell::new(Outcome { forwarding: not_reached(), quit: not_reached(), shutdown: not_reached() }));
    let service = stub_service();
    let step = RefCell::new(Step::WaitForTracking);
    let started = Instant::now();
    let shell_step = shell.clone();
    let result = outcome.clone();
    let stops = unit.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let fail = |message: String| {
            let mut result = result.borrow_mut();
            if result.forwarding.is_ok() {
                result.quit = Err(message);
            } else {
                result.forwarding = Err(message);
            }
            shell_step.application().quit();
            glib::ControlFlow::Break
        };
        if started.elapsed() > DEADLINE {
            return fail("the test ran out of time".into());
        }
        let Some(controller) = shell_step.controller() else { return glib::ControlFlow::Continue };
        let app = controller.application().clone();
        let page = controller.page_window();
        let failure_box = || windows_titled(&app, FAILURE_CAPTION, page.as_ref()).into_iter().next();
        let mut step = step.borrow_mut();
        let next = match &mut *step {
            Step::WaitForTracking if controller.device_status() == DeviceStatus::Tracking => {
                Step::Guest(second_instance(&["--guest"]))
            }
            Step::WaitForTracking => return glib::ControlFlow::Continue,
            Step::Guest(child) => match ended(child, started) {
                Err(message) => return fail(message),
                Ok(None) => return glib::ControlFlow::Continue,
                Ok(Some(status)) if !status.success() => return fail(format!("--guest exited with {status}")),
                Ok(Some(_)) if *setup.runs.borrow() != [vec!["--guest".to_string()]] => {
                    return fail(format!("the primary ran setup with {:?}", setup.runs.borrow()));
                }
                Ok(Some(_)) if page.is_some() => return fail("--guest opened the page".into()),
                Ok(Some(_)) => Step::Bare(second_instance(&[])),
            },
            Step::Bare(child) => match ended(child, started) {
                Err(message) => return fail(message),
                Ok(None) => return glib::ControlFlow::Continue,
                Ok(Some(status)) if !status.success() => return fail(format!("a bare launch exited with {status}")),
                Ok(Some(_)) => {
                    let Some(page) = page else { return fail("a bare second launch opened no page".into()) };
                    if setup.runs.borrow().len() != 1 {
                        return fail(format!("a bare launch ran setup: {:?}", setup.runs.borrow()));
                    }
                    page.close();
                    Step::Page(second_instance(&["--page"]))
                }
            },
            Step::Page(child) => match ended(child, started) {
                Err(message) => return fail(message),
                Ok(None) => return glib::ControlFlow::Continue,
                Ok(Some(status)) if !status.success() => return fail(format!("--page exited with {status}")),
                Ok(Some(_)) => {
                    let Some(page) = page else { return fail("--page opened no page".into()) };
                    if page.title().as_deref() != Some(PAGE_TITLE) || page.default_width() != PAGE_WIDTH {
                        return fail(format!("unexpected page {:?} {}", page.title(), page.default_width()));
                    }
                    let Some(trash) = trash_button(&page, "Alice") else {
                        return fail("the page lists no Alice to mark".into());
                    };
                    trash.emit_clicked();
                    result.borrow_mut().forwarding = Ok(());
                    stops.failing_stops.store(2, Ordering::Release);
                    controller.quit();
                    controller.quit();
                    Step::Dialog
                }
            },
            Step::Dialog => {
                let dialogs = windows_titled(&app, "Confirmation Dialog", None);
                let [dialog] = dialogs.as_slice() else {
                    return fail(format!("{} confirmation dialogs after two clicks on Quit", dialogs.len()));
                };
                let (Some(checkbox), Some(confirm)) = (checkbox(dialog), button(dialog, CONFIRM_TEXT)) else {
                    return fail("the dialog lacks the checkbox or the confirm button".into());
                };
                if checkbox.label().as_deref() != Some(CHECKBOX_TEXT) || button(dialog, CANCEL_TEXT).is_none() {
                    return fail("the dialog's texts are not Tobii's".into());
                }
                checkbox.set_active(true);
                confirm.emit_clicked();
                Step::FirstFailure
            }
            Step::FirstFailure => {
                let Some(failure) = failure_box() else { return glib::ControlFlow::Continue };
                if !windows_titled(&app, "Confirmation Dialog", None).is_empty() {
                    return fail("the dialog stayed open after Confirm".into());
                }
                if !controller.quitting() {
                    return fail("the failure box is up but the app is not quitting".into());
                }
                if !Settings::load(&settings_path).is_ok_and(|settings| settings.skip_quit_confirmation) {
                    return fail("the checkbox was not persisted before the stop".into());
                }
                let Some(no) = button(&failure, "_No") else { return fail("the failure box has no No".into()) };
                no.emit_clicked();
                Step::Cleared
            }
            Step::Cleared => {
                if failure_box().is_some() || controller.quitting() {
                    return fail("No did not clear the quitting flag and close the box".into());
                }
                controller.quit();
                Step::SecondFailure
            }
            Step::SecondFailure => {
                if !windows_titled(&app, "Confirmation Dialog", None).is_empty() {
                    return fail("the dialog was shown although it was turned off".into());
                }
                let Some(failure) = failure_box() else { return glib::ControlFlow::Continue };
                let Some(yes) = button(&failure, "_Yes") else { return fail("the failure box has no Yes".into()) };
                result.borrow_mut().quit = Ok(());
                yes.emit_clicked();
                Step::Quitting
            }
            Step::Quitting => return glib::ControlFlow::Continue,
        };
        *step = next;
        glib::ControlFlow::Continue
    });

    let exit = shell.run([
        "tobii-linux-experience".to_string(),
        "--service".into(),
        service.addr.to_string(),
        "--service-unit".into(),
        UNIT.into(),
    ]);
    let mut outcome = outcome.borrow().clone();
    if outcome.quit.is_ok() && profiles.names().unwrap() != ["Garulf"] {
        outcome.quit = Err(format!("quitting with the page open kept {:?}", profiles.names().unwrap()));
    }
    let _ = std::fs::remove_dir_all(&directory);
    let calls = unit.calls.lock().unwrap().clone();
    if outcome.forwarding.is_ok() && calls.first() != Some(&format!("start {UNIT}")) {
        outcome.forwarding = Err(format!("the unit saw {calls:?}; only the primary starts it"));
    }
    let stop = format!("stop {UNIT}");
    if outcome.quit.is_ok() && calls.get(1..) != Some(&[stop.clone(), stop.clone(), stop][..]) {
        outcome.quit = Err(format!("the unit saw {calls:?}: expected a failed stop, then a failed one and its retry"));
    }
    if outcome.quit.is_ok() && exit != glib::ExitCode::SUCCESS {
        outcome.quit = Err(format!("the primary exited with {exit:?}"));
    }
    outcome.shutdown = shutdown_released(&shell, &service);
    outcome
}

/// After the app ran, the shell no longer holds the controller and the link's connection to
/// the service is closed, which only happens when the `Link` is dropped and its thread ends.
fn shutdown_released(shell: &Shell, service: &StubService) -> Result<(), String> {
    if shell.controller().is_some() {
        return Err("the shell still holds the controller after shutdown".into());
    }
    if service.accepted.load(Ordering::Acquire) == 0 {
        return Err("the link never connected".into());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while service.open.load(Ordering::Acquire) != 0 {
        if Instant::now() > deadline {
            return Err("the link's connection stayed open after shutdown".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn report(name: &str, result: &Result<(), String>) -> bool {
    match result {
        Ok(()) => println!("test {name} ... ok"),
        Err(message) => println!("test {name} ... FAILED: {message}"),
    }
    result.is_ok()
}

/// Set by `make experience-test`: a missing display or session bus fails instead of skipping.
const REQUIRE_DISPLAY: &str = "TOBII_EXPERIENCE_REQUIRE_DISPLAY";

fn main() {
    const TESTS: &str =
        "second_instance_forwards_guest, quit_confirms_persists_and_retries, shutdown_releases_the_link";
    let missing = if std::env::var_os("GDK_BACKEND").is_none() || gtk::init().is_err() {
        Some("no display")
    } else if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        Some("no session bus")
    } else {
        None
    };
    if let Some(missing) = missing {
        if std::env::var_os(REQUIRE_DISPLAY).is_some() {
            println!("test {TESTS} ... FAILED: {missing}, and {REQUIRE_DISPLAY} is set");
            std::process::exit(1);
        }
        println!("test {TESTS} ... skipped ({missing}; run make experience-test)");
        return;
    }
    let outcome = run_primary();
    let forwarding = report("second_instance_forwards_guest", &outcome.forwarding);
    let quit = report("quit_confirms_persists_and_retries", &outcome.quit);
    let shutdown = report("shutdown_releases_the_link", &outcome.shutdown);
    if !(forwarding && quit && shutdown) {
        std::process::exit(1);
    }
}
