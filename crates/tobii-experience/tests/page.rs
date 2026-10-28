//! The Eye tracking page in a real GTK window against a stub service, a fake setup launcher
//! and a profile store in a temporary directory. GTK needs a display: `make experience-test`
//! runs this under Xvfb; without one it is skipped. It runs on the main thread (no test
//! harness), as GTK wants. With `TOBII_EXPERIENCE_SCREENSHOT=<path>` it also writes a PNG of
//! the page in state Tracking.

use std::cell::{Cell, RefCell};
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4 as gtk;
use tobii_control::protocol::{
    FRAME_CONTROL_EVENT, FRAME_CONTROL_HELLO, FRAME_CONTROL_REPLY, FRAME_CONTROL_REQUEST, decode_request, encode,
    split_frame,
};
use tobii_control::{Event, Reply, Request, Status, TrackedEyes};
use tobii_experience::app::{Controller, Deps, PAGE_TITLE, PAGE_WIDTH};
use tobii_experience::cli::Options;
use tobii_experience::guest::{LaunchError, OnExit, SetupLauncher};
use tobii_experience::page::PageState;
use tobii_experience::quit::ServiceUnit;
use tobii_gaze::profiles::ProfileStore;

const DEADLINE: Duration = Duration::from_secs(10);

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

/// The service's state: a version that every change bumps, the status, the requests seen
/// and how long replies wait.
#[derive(Debug)]
struct Service {
    version: u64,
    status: Status,
    requests: Vec<Request>,
    reply_delay: Duration,
}

/// A control service that keeps its status like `tobii-linux serve`: it applies on/off,
/// switches and tracked eyes to its status and the profile store, answers every request with
/// the new status and pushes changes to every connection.
#[derive(Clone)]
struct StubService {
    addr: SocketAddr,
    state: Arc<Mutex<Service>>,
}

impl StubService {
    fn start(store: ProfileStore) -> StubService {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let service = Service { version: 0, status: tracking(), requests: Vec::new(), reply_delay: Duration::ZERO };
        let state = Arc::new(Mutex::new(service));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (state, store) = (shared.clone(), store.clone());
                std::thread::spawn(move || serve(stream, &state, &store));
            }
        });
        StubService { addr, state }
    }

    fn change(&self, change: impl FnOnce(&mut Status)) {
        let mut state = self.state.lock().unwrap();
        change(&mut state.status);
        state.version += 1;
    }

    fn requests(&self) -> Vec<Request> {
        self.state.lock().unwrap().requests.clone()
    }

    fn set_reply_delay(&self, delay: Duration) {
        self.state.lock().unwrap().reply_delay = delay;
    }
}

fn apply(request: &Request, status: &mut Status, store: &ProfileStore) {
    match request {
        Request::SetEnabled(on) => status.enabled = *on,
        Request::SwitchProfile { name } => {
            store.set_current(name).unwrap();
            status.profile = Some(name.clone());
        }
        Request::SetTrackedEyes(eyes) => {
            let current = store.current().unwrap().unwrap();
            store.set_tracked_eyes(&current, eyes.registry_value()).unwrap();
        }
        _ => {}
    }
}

fn serve(mut stream: TcpStream, state: &Mutex<Service>, store: &ProfileStore) {
    stream.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut sent_version = None;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(read) => received.extend_from_slice(&chunk[..read]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        let mut frames = Vec::new();
        while let Some((kind, payload, used)) = split_frame(&received).unwrap() {
            received.drain(..used);
            if kind == FRAME_CONTROL_HELLO {
                let state = state.lock().unwrap();
                sent_version = Some(state.version);
                frames.push(encode(FRAME_CONTROL_EVENT, &Event::Status(state.status.clone())).unwrap());
            } else if kind == FRAME_CONTROL_REQUEST {
                let (id, request) = decode_request(&payload).unwrap();
                let delay = state.lock().unwrap().reply_delay;
                std::thread::sleep(delay);
                let mut state = state.lock().unwrap();
                let mut status = state.status.clone();
                apply(&request, &mut status, store);
                if status != state.status {
                    state.status = status;
                    state.version += 1;
                }
                state.requests.push(request);
                sent_version = Some(state.version);
                frames.push(encode(FRAME_CONTROL_REPLY, &Reply { id, result: Ok(state.status.clone()) }).unwrap());
            }
        }
        {
            let state = state.lock().unwrap();
            if sent_version.is_some_and(|sent| sent != state.version) {
                sent_version = Some(state.version);
                frames.push(encode(FRAME_CONTROL_EVENT, &Event::Status(state.status.clone())).unwrap());
            }
        }
        for frame in frames {
            if stream.write_all(&frame).is_err() {
                return;
            }
        }
    }
}

#[derive(Debug)]
struct NoUnit;

impl ServiceUnit for NoUnit {
    fn start(&self, _: &str) -> Result<(), String> {
        Ok(())
    }

    fn stop(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Records setup runs and keeps each one running until the test finishes it.
#[derive(Default)]
struct FakeSetup {
    runs: RefCell<Vec<Vec<String>>>,
    running: RefCell<Option<OnExit>>,
}

impl std::fmt::Debug for FakeSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeSetup").field("runs", &self.runs.borrow()).finish_non_exhaustive()
    }
}

impl SetupLauncher for FakeSetup {
    fn launch(&self, arguments: &[&str], on_exit: OnExit) -> Result<(), LaunchError> {
        if self.running.borrow().is_some() {
            return Err(LaunchError::AlreadyRunning);
        }
        self.runs.borrow_mut().push(arguments.iter().map(|argument| argument.to_string()).collect());
        self.running.replace(Some(on_exit));
        Ok(())
    }
}

impl FakeSetup {
    fn finish(&self, code: i32) {
        if let Some(on_exit) = self.running.take() {
            on_exit(Ok(ExitStatus::from_raw(code << 8)));
        }
    }

    fn last_run(&self) -> Option<Vec<String>> {
        self.runs.borrow().last().cloned()
    }
}

/// Runs the main loop until `condition` holds.
fn spin_until(what: &str, condition: impl Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + DEADLINE;
    let context = glib::MainContext::default();
    while !condition() {
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}"));
        }
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

/// Runs the main loop for `duration`.
fn spin_for(duration: Duration) {
    let until = Instant::now() + duration;
    let _ = spin_until("time to pass", || Instant::now() >= until);
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

/// The texts of the visible labels, in widget-tree order.
fn visible_texts(widget: &gtk::Widget) -> Vec<String> {
    if !widget.is_visible() {
        return Vec::new();
    }
    let mut texts = Vec::new();
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        texts.push(label.label().to_string());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        texts.extend(visible_texts(&current));
        child = current.next_sibling();
    }
    texts
}

fn has_text(widget: &gtk::Widget, text: &str) -> bool {
    descendants(widget).iter().any(|widget| widget.downcast_ref::<gtk::Label>().is_some_and(|l| l.label() == text))
}

fn find<W: IsA<gtk::Widget>>(window: &gtk::Window, matches: impl Fn(&W) -> bool) -> Result<W, String> {
    descendants(window.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<W>().ok())
        .find(|widget| matches(widget))
        .ok_or_else(|| format!("no {} found", W::static_type().name()))
}

fn button(window: &gtk::Window, text: &str) -> Result<gtk::Button, String> {
    find(window, |button: &gtk::Button| has_text(button.upcast_ref(), text)).map_err(|_| format!("no button {text:?}"))
}

fn toggles(window: &impl IsA<gtk::Widget>) -> Vec<gtk::ToggleButton> {
    descendants(window.upcast_ref()).into_iter().filter_map(|widget| widget.downcast().ok()).collect()
}

fn radio(window: &gtk::Window, text: &str) -> Result<gtk::CheckButton, String> {
    find(window, |radio: &gtk::CheckButton| radio.label().as_deref() == Some(text))
}

/// The button with `class` in the profile row of `name`.
fn row_button(window: &gtk::Window, name: &str, class: &str) -> Option<gtk::Button> {
    let row: gtk::Grid =
        find(window, |row: &gtk::Grid| row.has_css_class("tobii-profile-row") && has_text(row.upcast_ref(), name))
            .ok()?;
    descendants(row.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.has_css_class(class))
}

fn listed(controller: &Controller) -> Vec<String> {
    controller
        .profile_list()
        .map(|list| list.items().iter().map(|item| item.name.clone()).collect())
        .unwrap_or_default()
}

fn check(condition: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    if condition { Ok(()) } else { Err(message()) }
}

struct Fixture {
    controller: Controller,
    service: StubService,
    setup: Rc<FakeSetup>,
    store: ProfileStore,
}

impl Fixture {
    fn open(&self) -> Result<gtk::Window, String> {
        let window = self.controller.open_page();
        spin_until("the page window to map", || window.is_mapped())?;
        Ok(window)
    }

    fn closed(&self) -> Result<(), String> {
        spin_until("the page to close", || self.controller.page_window().is_none())
    }

    fn profile_names(&self) -> Vec<String> {
        self.store.names().unwrap()
    }
}

/// Header, then Tobii's sections in order; the fold-out opens on a click.
fn page_sections_in_tobii_order(f: &Fixture, screenshot: Option<&Path>) -> Result<(), String> {
    let window = f.open()?;
    check(window.title().as_deref() == Some(PAGE_TITLE) && window.default_width() == PAGE_WIDTH, || {
        format!("unexpected window {:?} {}", window.title(), window.default_width())
    })?;
    let improve_body = "If the light conditions change or if you experience less tracker precision, you might \
                        benefit from improving your calibration.";
    let mut expected = vec![
        "Garulf",
        "Tobii Experience",
        "ON",
        "Improve my calibration",
        improve_body,
        "Improve calibration",
        "Preview my gaze",
        "OFF",
        "Shows you a visual trail of your gaze",
        "Select eyes to detect",
        "Change screen",
        "If you move the sensor to a different monitor, you\u{2019}ll need to set up the new display.",
        "Set up display",
        "Change user profile",
        "Each user needs a unique profile. You may also need separate profiles to track your eyes with or without \
         glasses/contacts. Press Ctrl+Shft+F10 for a temporary guest profile.",
        "Create new profile",
        "Alice",
        "Bob",
    ];
    let texts = visible_texts(window.upcast_ref());
    check(texts == expected, || format!("collapsed page reads {texts:#?}"))?;
    if let Some(path) = screenshot {
        spin_for(Duration::from_millis(300));
        save_screenshot(&window, path)?;
        let scrolled: gtk::ScrolledWindow = find(&window, |_: &gtk::ScrolledWindow| true)?;
        let adjustment = scrolled.vadjustment();
        adjustment.set_value(adjustment.upper());
        spin_for(Duration::from_millis(1000));
        save_screenshot(&window, &path.with_file_name("page-scrolled.png"))?;
    }

    button(&window, "Select eyes to detect")?.emit_clicked();
    let eyes_body =
        "If you typically squint or have poor sight\u{a0}in one eye, you can make the eye tracker detect one eye only.";
    let at = expected.iter().position(|text| *text == "Select eyes to detect").unwrap() + 1;
    expected.splice(at..at, [eyes_body, "Both eyes", "Left eye only", "Right eye only"]);
    let texts = visible_texts(window.upcast_ref());
    check(texts == expected, || format!("expanded page reads {texts:#?}"))?;
    window.close();
    f.closed()
}

/// The page's sensitivity follows the page state, and the preview toggle is off at launch,
/// sensitive only while tracking, and reported to its hook.
fn availability_follows_the_state(f: &Fixture) -> Result<(), String> {
    let seen = Rc::new(Cell::new(None));
    let hook_seen = seen.clone();
    f.controller.connect_preview(move |_, on| hook_seen.set(Some(on)));
    let window = f.open()?;
    let [switch, preview] = toggles(&window).try_into().map_err(|_| "expected two toggles".to_string())?;
    check(!f.controller.preview_requested() && !preview.is_active(), || "the preview starts off".into())?;
    check(switch.is_sensitive() && preview.is_sensitive(), || "Tracking enables both toggles".into())?;
    for text in ["Improve calibration", "Set up display", "Create new profile", "Alice", "Select eyes to detect"] {
        check(button(&window, text)?.is_sensitive(), || format!("{text} is insensitive in Tracking"))?;
    }
    preview.set_active(true);
    check(f.controller.preview_requested() && seen.get() == Some(true), || "the preview toggle was not taken".into())?;

    f.service.change(|status| status.display_area = false);
    spin_until("InvalidMonitorSetup", || f.controller.page_state() == PageState::InvalidMonitorSetup)?;
    check(!preview.is_sensitive() && preview.is_active(), || "the preview stays on but insensitive".into())?;
    for text in ["Improve calibration", "Create new profile", "Alice"] {
        check(!button(&window, text)?.is_sensitive(), || format!("{text} is sensitive in InvalidMonitorSetup"))?;
    }
    button(&window, "Set up display")?.emit_clicked();
    check(f.setup.last_run() == Some(vec!["-R".into()]), || format!("ran {:?}", f.setup.runs.borrow()))?;
    f.setup.finish(0);
    f.closed()?;
    f.service.change(|status| status.display_area = true);
    spin_until("Tracking", || f.controller.page_state() == PageState::Tracking)?;
    f.controller.set_preview_requested(false);
    check(seen.get() == Some(false), || "the hook missed the preview going off".into())
}

/// Improve calibration, Set up display and Create new profile run setup with Tobii's
/// arguments, keep the page insensitive for the whole run and close it whatever the result;
/// a run while another one is active is refused and closes the page too.
fn buttons_launch_setup_and_hold_the_page(f: &Fixture) -> Result<(), String> {
    for (text, argument, code) in [("Improve calibration", "-C", 0), ("Set up display", "-S", 2)] {
        let window = f.open()?;
        row_button(&window, "Alice", "tobii-trash").ok_or("no trash for Alice")?.emit_clicked();
        button(&window, text)?.emit_clicked();
        check(f.setup.last_run() == Some(vec![argument.into()]), || format!("{text} ran {:?}", f.setup.last_run()))?;
        spin_for(Duration::from_millis(100));
        check(f.controller.page_busy() && !button(&window, text)?.is_sensitive(), || format!("{text}: page not busy"))?;
        check(f.controller.page_window().is_some(), || format!("{text}: the page closed during the run"))?;
        check(f.profile_names().contains(&"Alice".into()), || format!("{text} deleted the marked profile first"))?;
        f.setup.finish(code);
        f.closed()?;
        check(!f.controller.page_busy(), || format!("{text}: still busy after the run"))?;
        check(!f.profile_names().contains(&"Alice".into()), || format!("{text}: closing kept the marked profile"))?;
        restore_profile(f, "Alice");
    }

    let window = f.open()?;
    row_button(&window, "Alice", "tobii-trash").ok_or("no trash for Alice")?.emit_clicked();
    button(&window, "Create new profile")?.emit_clicked();
    check(f.setup.last_run() == Some(vec!["-N".into()]), || format!("ran {:?}", f.setup.last_run()))?;
    check(!f.profile_names().contains(&"Alice".into()), || "Create new profile kept the marked profile".into())?;
    f.setup.finish(1);
    f.closed()?;
    restore_profile(f, "Alice");

    check(f.controller.guest(), || "the guest run did not start".into())?;
    let window = f.open()?;
    button(&window, "Improve calibration")?.emit_clicked();
    f.closed()?;
    check(!f.controller.page_busy(), || "a refused run left the page busy".into())?;
    check(f.setup.runs.borrow().last() == Some(&vec!["--guest".to_string()]), || {
        "the refused run was recorded".into()
    })?;
    f.setup.finish(0);
    Ok(())
}

fn restore_profile(f: &Fixture, name: &str) {
    std::fs::write(f.store.directory.join(format!("{name}.setpm")), b"").unwrap();
}

/// The switch holds the page while its request runs, and 500 ms longer when it turns
/// tracking off.
fn disabling_waits_500_ms(f: &Fixture) -> Result<(), String> {
    let window = f.open()?;
    let switch = toggles(&window).into_iter().next().ok_or("no switch")?;
    check(switch.is_active(), || "the switch shows tracking off".into())?;
    let clicked = Instant::now();
    switch.set_active(false);
    check(f.controller.page_busy(), || "turning off did not hold the page".into())?;
    spin_until("the off reply", || !f.controller.enabled())?;
    let replied = clicked.elapsed();
    spin_until("the page to come back", || !f.controller.page_busy())?;
    let released = clicked.elapsed();
    check(released >= Duration::from_millis(500) && released >= replied, || {
        format!("released after {released:?} (reply after {replied:?})")
    })?;
    check(!switch.is_active(), || "the switch did not stay off".into())?;

    let clicked = Instant::now();
    switch.set_active(true);
    spin_until("the page to come back", || !f.controller.page_busy())?;
    check(clicked.elapsed() < Duration::from_millis(450), || format!("enabling held {:?}", clicked.elapsed()))?;
    check(
        f.controller.enabled()
            && f.service.requests().ends_with(&[Request::SetEnabled(false), Request::SetEnabled(true)]),
        || format!("the service saw {:?}", f.service.requests()),
    )?;
    window.close();
    f.closed()
}

/// A radio change sets the current profile's tracked eyes and holds the page meanwhile.
fn tracked_eyes_follow_the_radios(f: &Fixture) -> Result<(), String> {
    let window = f.open()?;
    check(radio(&window, "Both eyes")?.is_active(), || "Both eyes is not selected".into())?;
    f.service.set_reply_delay(Duration::from_millis(300));
    radio(&window, "Left eye only")?.set_active(true);
    check(f.controller.page_busy(), || "the radio did not hold the page".into())?;
    spin_until("the reply", || !f.controller.page_busy())?;
    f.service.set_reply_delay(Duration::ZERO);
    check(f.service.requests().last() == Some(&Request::SetTrackedEyes(TrackedEyes::Left)), || {
        format!("the service saw {:?}", f.service.requests())
    })?;
    check(f.store.tracked_eyes("Garulf").unwrap() == 1 && radio(&window, "Left eye only")?.is_active(), || {
        "Left eye only did not stick".into()
    })?;
    radio(&window, "Both eyes")?.set_active(true);
    spin_until("the reply", || !f.controller.page_busy())?;
    window.close();
    f.closed()
}

/// Trash marks a row (Undo shows, the profile button greys, nothing is deleted), Undo
/// clears it, and closing the window really deletes it.
fn deletions_commit_on_close(f: &Fixture) -> Result<(), String> {
    let window = f.open()?;
    row_button(&window, "Alice", "tobii-trash").ok_or("no trash for Alice")?.emit_clicked();
    let undo = row_button(&window, "Alice", "tobii-undo").ok_or("no Undo for a marked row")?;
    check(row_button(&window, "Alice", "tobii-trash").is_none(), || "a marked row still shows its trash".into())?;
    check(!button(&window, "Alice")?.is_sensitive(), || "a marked profile can still be switched to".into())?;
    check(f.profile_names().contains(&"Alice".into()), || "marking deleted the profile".into())?;
    undo.emit_clicked();
    check(row_button(&window, "Alice", "tobii-trash").is_some(), || "Undo did not bring the trash back".into())?;
    row_button(&window, "Alice", "tobii-trash").ok_or("no trash for Alice")?.emit_clicked();
    window.close();
    f.closed()?;
    check(!f.profile_names().contains(&"Alice".into()), || "closing kept the marked profile".into())?;
    let window = f.open()?;
    check(listed(&f.controller) == ["Bob"], || format!("listed {:?}", listed(&f.controller)))?;
    window.close();
    f.closed()?;
    restore_profile(f, "Alice");
    Ok(())
}

/// A click on a listed profile switches to it and closes the page; the next page lists
/// the former current profile and heads with the new one.
fn select_sends_switch_request(f: &Fixture) -> Result<(), String> {
    let window = f.open()?;
    button(&window, "Bob")?.emit_clicked();
    check(f.controller.page_busy(), || "the switch did not hold the page".into())?;
    f.closed()?;
    let switch = Request::SwitchProfile { name: "Bob".into() };
    check(f.service.requests().last() == Some(&switch), || format!("the service saw {:?}", f.service.requests()))?;
    let window = f.open()?;
    let texts = visible_texts(window.upcast_ref());
    check(texts.first().map(String::as_str) == Some("Bob"), || format!("the header reads {:?}", texts.first()))?;
    check(listed(&f.controller) == ["Alice", "Garulf"], || format!("listed {:?}", listed(&f.controller)))?;
    window.close();
    f.closed()
}

fn deps(setup: Rc<FakeSetup>, store: &ProfileStore) -> Deps {
    Deps { service: Arc::new(NoUnit), setup, settings_path: None, tray: false, profiles: store.clone() }
}

/// With no status yet (Initializing) and with the service unreachable (Error), Tobii's
/// `SystemEnabled` reads as true: the switch shows ON, the 4.3 table makes it insensitive,
/// and a click sends nothing.
fn unknown_state_keeps_the_switch_on(app: &gtk::Application, store: &ProfileStore) -> Result<(), String> {
    let unreachable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap();
    let options = Options { service: unreachable, ..Options::default() };
    let failing = Controller::new(app, &options, deps(Rc::default(), store));
    spin_until("state Error", || failing.page_state() == PageState::Error)?;
    let service = StubService::start(store.clone());
    let options = Options { service: service.addr, ..Options::default() };
    let starting = Controller::new(app, &options, deps(Rc::default(), store));
    for (state, controller) in [(PageState::Error, &failing), (PageState::Initializing, &starting)] {
        check(controller.page_state() == state, || format!("expected {state:?}, got {:?}", controller.page_state()))?;
        let page = tobii_experience::page::page(controller);
        let switch = toggles(&page).into_iter().next().ok_or("no switch")?;
        check(switch.is_active() && switch.label_is_on(), || format!("{state:?}: the switch shows OFF"))?;
        check(!switch.is_sensitive(), || format!("{state:?}: the switch is sensitive"))?;
        switch.set_active(false);
        check(!controller.page_busy() && switch.is_active(), || format!("{state:?}: the click was taken"))?;
    }
    spin_until("state Tracking", || starting.page_state() == PageState::Tracking)?;
    spin_for(Duration::from_millis(200));
    check(!service.requests().iter().any(|request| matches!(request, Request::SetEnabled(_))), || {
        format!("the service saw {:?}", service.requests())
    })?;
    check(!failing.page_busy(), || "Error: the page is busy".into())
}

trait SwitchLabel {
    fn label_is_on(&self) -> bool;
}

impl SwitchLabel for gtk::ToggleButton {
    fn label_is_on(&self) -> bool {
        has_text(self.upcast_ref(), "ON")
    }
}

/// Renders the window through its own renderer into a PNG.
fn save_screenshot(window: &gtk::Window, path: &Path) -> Result<(), String> {
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    let (width, height) = (f64::from(window.width()), f64::from(window.height()));
    paintable.snapshot(&snapshot, width, height);
    let node = snapshot.to_node().ok_or("the window drew nothing")?;
    let renderer = window.renderer().ok_or("the window has no renderer")?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture: gdk::Texture = renderer.render_texture(&node, Some(&bounds));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    texture.save_to_png(path).map_err(|error| error.to_string())?;
    println!("wrote {} ({}x{})", path.display(), texture.width(), texture.height());
    Ok(())
}

fn run(screenshot: Option<PathBuf>) -> Vec<(&'static str, Result<(), String>)> {
    let directory = std::env::temp_dir().join(format!("tobii-experience-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let store = ProfileStore::new(directory.join("profiles"));
    std::fs::create_dir_all(&store.directory).unwrap();
    for name in ["Garulf", "Alice", "Bob", "Guest"] {
        std::fs::write(store.directory.join(format!("{name}.setpm")), b"").unwrap();
    }
    store.set_current("Garulf").unwrap();

    let app = gtk::Application::builder()
        .application_id("org.tobii_linux.ExperiencePageTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    if let Err(error) = app.register(None::<&gio::Cancellable>) {
        return vec![("register", Err(error.to_string()))];
    }
    let service = StubService::start(store.clone());
    let setup = Rc::new(FakeSetup::default());
    let options = Options { service: service.addr, ..Options::default() };
    let deps = Deps { settings_path: Some(directory.join("experience.json")), ..deps(setup.clone(), &store) };
    let controller = Controller::new(&app, &options, deps);
    let fixture = Fixture { controller, service, setup, store };
    let mut results = vec![(
        "page_reaches_tracking",
        spin_until("state Tracking", || fixture.controller.page_state() == PageState::Tracking),
    )];
    if results[0].1.is_ok() {
        results.push(("page_sections_in_tobii_order", page_sections_in_tobii_order(&fixture, screenshot.as_deref())));
        results.push(("availability_follows_the_state", availability_follows_the_state(&fixture)));
        results.push(("buttons_launch_setup_and_hold_the_page", buttons_launch_setup_and_hold_the_page(&fixture)));
        results.push(("disabling_waits_500_ms", disabling_waits_500_ms(&fixture)));
        results.push(("tracked_eyes_follow_the_radios", tracked_eyes_follow_the_radios(&fixture)));
        results.push(("deletions_commit_on_close", deletions_commit_on_close(&fixture)));
        results.push(("select_sends_switch_request", select_sends_switch_request(&fixture)));
        results.push(("unknown_state_keeps_the_switch_on", unknown_state_keeps_the_switch_on(&app, &fixture.store)));
    }
    if let Some(window) = fixture.controller.page_window() {
        window.close();
    }
    drop(fixture);
    let _ = std::fs::remove_dir_all(&directory);
    results
}

/// Set by `make experience-test`: a missing display fails instead of skipping.
const REQUIRE_DISPLAY: &str = "TOBII_EXPERIENCE_REQUIRE_DISPLAY";

fn main() {
    if std::env::var_os("GDK_BACKEND").is_none() || gtk::init().is_err() {
        if std::env::var_os(REQUIRE_DISPLAY).is_some() {
            println!("test page ... FAILED: no display, and {REQUIRE_DISPLAY} is set");
            std::process::exit(1);
        }
        println!("test page ... skipped (no display; run make experience-test)");
        return;
    }
    let screenshot = std::env::var_os("TOBII_EXPERIENCE_SCREENSHOT").map(PathBuf::from);
    let mut failed = false;
    for (name, result) in run(screenshot) {
        match result {
            Ok(()) => println!("test {name} ... ok"),
            Err(message) => {
                failed = true;
                println!("test {name} ... FAILED: {message}");
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
