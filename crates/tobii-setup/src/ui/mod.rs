//! The GTK 4 window: draws the screens the flow publishes and reports what the user does.
//!
//! The window never touches the tracker. A 16 ms timer on the GTK main loop polls the
//! [`UiChannel`] for a new screen and the live eye positions, advances the animations and
//! acknowledges each screen once it is drawn, as the browser page did.

mod draw;
mod pages;

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use serde_json::{Value as Json, json};

use crate::alignment::{self, Alignment, KeyOutcome, Rect};
use crate::channel::{UiChannel, WindowEvent};
use crate::cli::{self, Args, Command};
use crate::device::{self, WouldSend};
use crate::flow::{Desktop, ExitCodes, OpenSession, Report, SetupFlow, SetupOutcome, Status};
use crate::keys::{Key, KeyResponse, respond};
use crate::motion::fade_in;
use crate::particles::{CalibrationView, ack_delay_s};
use crate::positioning_view::{self as pv, CURSOR_HIDE_S};
use crate::screen::{Action, EyesLive, Screen, ScreenKind, View, fills_screen};
use crate::style;
use pages::{
    AspectPage, BusyPage, CalibrationPage, DisplayPage, EndPage, FailedPage, PositioningPage, ProfilePage, SavePage,
};

const POLL: Duration = Duration::from_millis(16);
const VIEW_REPORT_S: f64 = 1.0;
const MOVE_SETTLE: Duration = Duration::from_millis(300);
const CLOSE_SIZE: i32 = 55;
/// An option whose file or device cannot be used: a failure under the legacy codes, an
/// argument error under Tobii's.
const UNUSABLE_OPTION: u8 = 1;

type SharedLog = Arc<Mutex<Option<std::fs::File>>>;

fn write_log(log: &SharedLog, kind: &str, values: Json) {
    let mut log = log.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(file) = log.as_mut() else { return };
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let mut record = json!({ "kind": kind, "time": time });
    if let (Some(record), Json::Object(values)) = (record.as_object_mut(), values) {
        record.extend(values);
    }
    let _ = writeln!(file, "{record}");
}

/// What the window needs to start the flow once GTK is up.
struct Launch {
    args: Args,
    options: crate::flow::SetupOptions,
    desktop: Desktop,
    log: SharedLog,
}

fn open_session(args: &Args, log: &SharedLog) -> OpenSession {
    let dry_run = args.dry_run;
    let log = Arc::clone(log);
    let dry_run_report = Box::new(move || -> Option<WouldSend> {
        dry_run.then(|| {
            let log = Arc::clone(&log);
            Box::new(move |message: &tobii_protocol::Message| write_log(&log, "would_send", device::describe(message)))
                as WouldSend
        })
    });
    cli::session_opener(args, dry_run_report, device::open_usb)
}

/// The command line entry point of `tobii-linux-setup`.
pub fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let codes = cli::exit_codes(arguments.iter().cloned());
    let args = match cli::parse(arguments) {
        Ok(Command::Run(args)) => *args,
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            return std::process::ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{}\nerror: {error}", cli::USAGE.lines().next().unwrap_or_default());
            return std::process::ExitCode::from(codes.argument_error());
        }
    };
    let scratch = match args.simulate.map(|_| cli::scratch_directory()).transpose() {
        Ok(scratch) => scratch,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(codes.status(Status::Failed));
        }
    };
    let options = match cli::options_from(&args, scratch) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::from(UNUSABLE_OPTION);
        }
    };
    let log = match &args.log {
        Some(path) => match std::fs::OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => Some(file),
            Err(error) => {
                eprintln!("error: {}: {error}", path.display());
                return std::process::ExitCode::from(UNUSABLE_OPTION);
            }
        },
        None => None,
    };
    let desktop = cli::desktop(&args, &options);
    let launch = Rc::new(RefCell::new(Some(Launch { args, options, desktop, log: Arc::new(Mutex::new(log)) })));
    let exit = Rc::new(Cell::new(codes.status(Status::Running)));
    let app = gtk::Application::builder().flags(gio::ApplicationFlags::NON_UNIQUE).build();
    let keep: RefCell<Option<Rc<Ui>>> = RefCell::new(None);
    let exit_code = Rc::clone(&exit);
    app.connect_activate(move |app| {
        if let Some(launch) = launch.borrow_mut().take() {
            keep.replace(Some(Ui::start(app, launch, codes, Rc::clone(&exit_code))));
        }
    });
    app.run_with_args::<&str>(&[]);
    std::process::ExitCode::from(exit.get())
}

struct State {
    screen: Screen,
    monitor: Option<String>,
    sent_view: Option<View>,
    view_reported_at: f64,
    css_height: f64,
    eyes: Option<EyesLive>,
    positioning_since: f64,
    leaving_since: Option<f64>,
    last_motion: f64,
    cursor_hidden: Option<bool>,
    lines_shown: Option<bool>,
    outcome: Option<SetupOutcome>,
    close_requested: bool,
    alignment: Alignment,
    calibration: CalibrationView,
}

struct Pages {
    busy: BusyPage,
    end: EndPage,
    save: SavePage,
    aspect: AspectPage,
    failed: FailedPage,
    profile: ProfilePage,
    display: DisplayPage,
    positioning: PositioningPage,
    calibration: CalibrationPage,
}

struct Ui {
    window: gtk::ApplicationWindow,
    channel: UiChannel,
    desktop: Desktop,
    windowed: bool,
    stack: gtk::Stack,
    close: gtk::Button,
    banner: gtk::Label,
    css: gtk::CssProvider,
    epoch: Instant,
    finished: Arc<Mutex<Option<SetupOutcome>>>,
    exit_codes: ExitCodes,
    exit: Rc<Cell<u8>>,
    state: RefCell<State>,
    pages: Pages,
}

/// Places overlay children by rule: (child, rule from the overlay's size to a rectangle).
type Rule = Box<dyn Fn(&gtk::Widget, f64, f64) -> Rect>;

fn place(overlay: &gtk::Overlay, rules: Vec<(gtk::Widget, Rule)>) {
    overlay.connect_get_child_position(move |overlay, widget| {
        let (_, rule) = rules.iter().find(|(child, _)| child == widget)?;
        let rect = rule(widget, f64::from(overlay.width()), f64::from(overlay.height()));
        Some(gdk::Rectangle::new(
            rect.x.round() as i32,
            rect.y.round() as i32,
            rect.width.round().max(1.0) as i32,
            rect.height.round().max(1.0) as i32,
        ))
    });
}

fn natural_height(widget: &gtk::Widget, width: f64) -> f64 {
    f64::from(widget.measure(gtk::Orientation::Vertical, width as i32).1)
}

/// A full-width text row whose top is `top(height)`, or whose bottom is `bottom(height)`.
fn row_at(top: impl Fn(f64, f64) -> f64 + 'static) -> Rule {
    Box::new(move |widget, width, height| {
        let h = natural_height(widget, width);
        Rect { x: 0.0, y: top(height, h), width, height: h }
    })
}

fn gdk_monitors() -> Vec<gdk::Monitor> {
    let Some(display) = gdk::Display::default() else { return Vec::new() };
    let model = display.monitors();
    (0..model.n_items()).filter_map(|i| model.item(i).and_downcast::<gdk::Monitor>()).collect()
}

fn section(kind: &ScreenKind) -> &'static str {
    match kind {
        ScreenKind::Connecting | ScreenKind::Busy { .. } | ScreenKind::Moving { .. } => "busy",
        ScreenKind::Display(_) => "display",
        ScreenKind::DisplaySave { .. } => "save",
        ScreenKind::AspectWarning => "aspect",
        ScreenKind::Positioning { .. } => "positioning",
        ScreenKind::Calibration(_) => "calibration",
        ScreenKind::CalibrationFailed => "failed",
        ScreenKind::Profile { .. } => "profile",
        ScreenKind::Finished { .. } | ScreenKind::Cancelled { .. } | ScreenKind::Error { .. } => "end",
    }
}

fn key_of(keyval: gdk::Key) -> Key {
    match keyval {
        gdk::Key::Escape => Key::Escape,
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => Key::Enter,
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => Key::Tab,
        gdk::Key::Left | gdk::Key::KP_Left => Key::Left,
        gdk::Key::Right | gdk::Key::KP_Right => Key::Right,
        gdk::Key::F11 => Key::F11,
        other => other.to_unicode().map_or(Key::Other, Key::Char),
    }
}

impl Ui {
    fn start(app: &gtk::Application, launch: Launch, exit_codes: ExitCodes, exit: Rc<Cell<u8>>) -> Rc<Ui> {
        let Launch { args, options, desktop, log } = launch;
        let window = gtk::ApplicationWindow::builder().application(app).title("Eye tracker setup").build();
        window.set_decorated(false);
        let channel = UiChannel::new();
        let css = gtk::CssProvider::new();
        css.load_from_string(&style::css(720.0));
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        let stack = gtk::Stack::new();
        let pages = Pages {
            busy: BusyPage::new(),
            end: EndPage::new(),
            save: SavePage::new(),
            aspect: AspectPage::new(),
            failed: FailedPage::new(),
            profile: ProfilePage::new(),
            display: DisplayPage::new(),
            positioning: PositioningPage::new(),
            calibration: CalibrationPage::new(),
        };
        stack.add_named(&pages.busy.root, Some("busy"));
        stack.add_named(&pages.end.root, Some("end"));
        stack.add_named(&pages.save.root, Some("save"));
        stack.add_named(&pages.aspect.root, Some("aspect"));
        stack.add_named(&pages.failed.root, Some("failed"));
        stack.add_named(&pages.profile.root, Some("profile"));
        stack.add_named(&pages.display.root, Some("display"));
        stack.add_named(&pages.positioning.root, Some("positioning"));
        stack.add_named(&pages.calibration.root, Some("calibration"));
        stack.set_visible_child_name("busy");
        let root = gtk::Overlay::new();
        root.set_child(Some(&stack));
        let close = gtk::Button::new();
        close.add_css_class("flat-icon");
        close.add_css_class("close");
        close.set_tooltip_text(Some("Cancel (Esc)"));
        let cross = gtk::DrawingArea::new();
        cross.set_content_width(CLOSE_SIZE);
        cross.set_content_height(CLOSE_SIZE);
        cross.set_can_target(false);
        cross.set_draw_func(|area, cr, width, _| draw::cross(cr, f64::from(width), &area.color()));
        close.set_child(Some(&cross));
        close.set_halign(gtk::Align::End);
        close.set_valign(gtk::Align::Start);
        root.add_overlay(&close);
        let banner = pages::label(pages::BANNER, &["banner"]);
        banner.set_valign(gtk::Align::Start);
        banner.set_hexpand(true);
        banner.set_visible(false);
        root.add_overlay(&banner);
        window.set_child(Some(&root));

        let monitor = desktop.current_monitor().map(|m| m.key.clone());
        let state = State {
            screen: channel.current(),
            monitor: monitor.clone(),
            sent_view: None,
            view_reported_at: f64::NEG_INFINITY,
            css_height: 720.0,
            eyes: None,
            positioning_since: 0.0,
            leaving_since: None,
            last_motion: 0.0,
            cursor_hidden: None,
            lines_shown: None,
            outcome: None,
            close_requested: false,
            alignment: Alignment::default(),
            calibration: CalibrationView::new(0x7E57),
        };
        let ui = Rc::new(Ui {
            window,
            channel,
            desktop,
            windowed: args.windowed,
            stack,
            close,
            banner,
            css,
            epoch: Instant::now(),
            finished: Arc::new(Mutex::new(None)),
            exit_codes,
            exit,
            state: RefCell::new(state),
            pages,
        });
        ui.place_window(monitor.as_deref());
        ui.connect(&Rc::downgrade(&ui));
        ui.window.present();
        ui.start_flow(options, open_session(&args, &log), &log);
        let weak = Rc::downgrade(&ui);
        glib::timeout_add_local(POLL, move || match weak.upgrade() {
            Some(ui) => {
                ui.poll();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        ui
    }

    fn start_flow(&self, options: crate::flow::SetupOptions, open: OpenSession, log: &SharedLog) {
        let report_log = Arc::clone(log);
        let report: Report = Box::new(move |kind, values| write_log(&report_log, kind, values));
        let flow = SetupFlow::new(self.channel.clone(), options, report, self.desktop.clone());
        let finished = Arc::clone(&self.finished);
        std::thread::Builder::new()
            .name("setup-flow".into())
            .spawn(move || {
                let outcome = flow.run(open);
                *finished.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            })
            .expect("the flow thread starts");
    }

    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    // Placement and the view -----------------------------------------------------------------

    fn gdk_monitor(&self, key: &str) -> Option<gdk::Monitor> {
        let monitor = self.desktop.monitors.iter().find(|m| m.key == key)?;
        gdk_monitors().into_iter().find(|g| g.connector().is_some_and(|c| c == monitor.name))
    }

    fn place_window(&self, key: Option<&str>) {
        let target = key.and_then(|key| self.gdk_monitor(key)).or_else(|| gdk_monitors().into_iter().next());
        if let Some(target) = &target {
            let geometry = target.geometry();
            self.window.set_default_size(geometry.width(), geometry.height());
        }
        if self.windowed {
            return;
        }
        match target.filter(|_| key.is_some()) {
            Some(target) => self.window.fullscreen_on_monitor(&target),
            None => self.window.fullscreen(),
        }
    }

    fn view(&self) -> Option<View> {
        let (width, height) = (f64::from(self.window.width()), f64::from(self.window.height()));
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let display = gdk::Display::default()?;
        let on = self.window.surface().and_then(|surface| display.monitor_at_surface(&surface));
        let monitor = on.or_else(|| gdk_monitors().into_iter().next())?;
        let geometry = monitor.geometry();
        let scale = monitor.scale();
        let (screen_width, screen_height) = (f64::from(geometry.width()), f64::from(geometry.height()));
        let state = self.state.borrow();
        let logical = state.monitor.as_deref().and_then(|key| self.desktop.monitors.iter().find(|m| m.key == key));
        let simulated = logical
            .filter(|m| state.monitor.as_deref().is_some_and(|k| self.gdk_monitor(k).is_none()) && m.bounds.is_some());
        let screen_px = match simulated.and_then(|m| m.resolution_px()) {
            Some([w, h]) => [w as f64, h as f64],
            None => [(screen_width * scale).round(), (screen_height * scale).round()],
        };
        Some(View { width, height, screen_width, screen_height, screen_px, fullscreen: self.window.is_fullscreen() })
    }

    fn event(&self, seq: Option<u64>) -> WindowEvent {
        WindowEvent { seq, view: self.view(), monitor: self.state.borrow().monitor.clone(), ..WindowEvent::default() }
    }

    fn act(&self, action: Action) {
        let seq = self.state.borrow().screen.seq;
        self.channel.window_event(WindowEvent { action: Some(action), ..self.event(Some(seq)) });
    }

    fn ack(&self, seq: u64) {
        self.channel.window_event(WindowEvent { ack: true, ..self.event(Some(seq)) });
    }

    fn ack_after(self: &Rc<Self>, seq: u64, seconds: f64) {
        if seconds <= 0.0 {
            self.ack(seq);
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs_f64(seconds), move || {
            if let Some(ui) = weak.upgrade() {
                ui.ack(seq);
            }
        });
    }

    fn fullscreen_view(&self) -> bool {
        fills_screen(self.view().as_ref())
    }

    fn request_fullscreen(&self) {
        if !self.windowed && !self.window.is_fullscreen() {
            self.window.fullscreen();
        }
    }

    // Signals ---------------------------------------------------------------------------------

    fn connect(&self, weak: &Weak<Ui>) {
        let with = |weak: &Weak<Ui>, action: Action| {
            let weak = weak.clone();
            move |_: &gtk::Button| {
                if let Some(ui) = weak.upgrade() {
                    ui.act(action.clone());
                }
            }
        };
        let p = &self.pages;
        self.close.connect_clicked(with(weak, Action::Cancel));
        p.save.save.connect_clicked(with(weak, Action::Save));
        p.save.keep.connect_clicked(with(weak, Action::Skip));
        p.aspect.yes.connect_clicked(with(weak, Action::Yes));
        p.aspect.no.connect_clicked(with(weak, Action::No));
        p.aspect.close.connect_clicked(with(weak, Action::No));
        p.failed.retry.connect_clicked(with(weak, Action::Retry));
        p.failed.cancel.connect_clicked(with(weak, Action::Cancel));
        let w = weak.clone();
        p.profile.save.connect_clicked(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.save_profile();
            }
        });
        let w = weak.clone();
        p.profile.entry.connect_activate(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.save_profile();
            }
        });
        let w = weak.clone();
        p.end.close.connect_clicked(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.close_window();
            }
        });
        self.connect_display(weak);
        self.connect_drawing(weak);
        self.connect_input(weak);
        self.connect_layout();
    }

    fn connect_display(&self, weak: &Weak<Ui>) {
        let display = &self.pages.display;
        let w = weak.clone();
        display.this_display.connect_clicked(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.this_display();
            }
        });
        let w = weak.clone();
        display.done.connect_clicked(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.finish_lines();
            }
        });
        for (button, action) in [(&display.previous, Action::Previous), (&display.next, Action::Next)] {
            let w = weak.clone();
            button.connect_clicked(move |_| {
                if let Some(ui) = w.upgrade() {
                    ui.switch_monitor(action.clone());
                }
            });
        }
        let press = gtk::GestureClick::new();
        let w = weak.clone();
        press.connect_pressed(move |_, _, x, y| {
            if let Some(ui) = w.upgrade() {
                ui.state.borrow_mut().alignment.press(x, y);
                ui.pages.display.canvas.queue_draw();
            }
        });
        let w = weak.clone();
        press.connect_released(move |_, _, _, _| {
            if let Some(ui) = w.upgrade() {
                ui.state.borrow_mut().alignment.release();
                ui.pages.display.canvas.queue_draw();
            }
        });
        display.canvas.add_controller(press);
        let motion = gtk::EventControllerMotion::new();
        let w = weak.clone();
        motion.connect_motion(move |_, x, y| {
            if let Some(ui) = w.upgrade() {
                let movable = ui.state.borrow_mut().alignment.motion(x, y);
                ui.pages.display.canvas.set_cursor_from_name(Some(if movable { "ew-resize" } else { "default" }));
                ui.pages.display.canvas.queue_draw();
            }
        });
        let w = weak.clone();
        motion.connect_leave(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.state.borrow_mut().alignment.leave();
                ui.pages.display.canvas.queue_draw();
            }
        });
        display.canvas.add_controller(motion);
    }

    fn connect_drawing(&self, weak: &Weak<Ui>) {
        let p = &self.pages;
        let w = weak.clone();
        p.busy.spinner.set_draw_func(move |_, cr, _, _| {
            if let Some(ui) = w.upgrade() {
                draw::spinner(cr, 0.0, 0.0, 1.0, ui.now());
            }
        });
        let w = weak.clone();
        p.display.canvas.set_draw_func(move |_, cr, _, height| {
            if let Some(ui) = w.upgrade() {
                let state = ui.state.borrow();
                let top = alignment::rows(f64::from(height))[3].0;
                draw::alignment(cr, &state.alignment, top);
            }
        });
        for (icon, previous) in [(&p.display.previous_icon, true), (&p.display.next_icon, false)] {
            icon.set_draw_func(move |area, cr, width, height| {
                draw::switcher(cr, previous, f64::from(width), f64::from(height), &area.color());
            });
        }
        let w = weak.clone();
        p.positioning.canvas.set_draw_func(move |_, cr, width, height| {
            if let Some(ui) = w.upgrade() {
                ui.draw_positioning(cr, f64::from(width), f64::from(height));
            }
        });
        let w = weak.clone();
        p.calibration.canvas.set_draw_func(move |_, cr, width, height| {
            if let Some(ui) = w.upgrade() {
                let state = ui.state.borrow();
                draw::calibration(cr, &state.calibration, ui.now(), f64::from(width), f64::from(height));
            }
        });
    }

    fn connect_input(&self, weak: &Weak<Ui>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = weak.clone();
        keys.connect_key_pressed(move |_, keyval, _, modifiers| match w.upgrade() {
            Some(ui) => ui.key(key_of(keyval), modifiers.contains(gdk::ModifierType::SHIFT_MASK)),
            None => glib::Propagation::Proceed,
        });
        self.window.add_controller(keys);
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = weak.clone();
        click.connect_pressed(move |_, _, _, _| {
            if let Some(ui) = w.upgrade()
                && ui.state.borrow().screen.kind.needs_fullscreen()
            {
                ui.request_fullscreen();
            }
        });
        self.window.add_controller(click);
        let continue_click = gtk::GestureClick::new();
        let w = weak.clone();
        continue_click.connect_pressed(move |_, _, _, _| {
            if let Some(ui) = w.upgrade() {
                ui.act(Action::Continue);
            }
        });
        self.pages.positioning.canvas.add_controller(continue_click);
        let motion = gtk::EventControllerMotion::new();
        let w = weak.clone();
        motion.connect_motion(move |_, _, _| {
            if let Some(ui) = w.upgrade() {
                ui.state.borrow_mut().last_motion = ui.now();
            }
        });
        self.window.add_controller(motion);
        let w = weak.clone();
        self.window.connect_close_request(move |_| match w.upgrade() {
            Some(ui) if ui.state.borrow().outcome.is_none() => {
                ui.state.borrow_mut().close_requested = true;
                ui.act(Action::Cancel);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
    }

    fn connect_layout(&self) {
        let display = &self.pages.display;
        let rule = |pick: fn(&alignment::Layout) -> Rect| -> Rule {
            Box::new(move |_, width, height| pick(&alignment::layout(width, height)))
        };
        place(
            &display.root,
            vec![
                (display.text.clone().upcast(), rule(|l| l.text)),
                (display.buttons.clone().upcast(), rule(|l| l.buttons)),
                (display.previous.clone().upcast(), rule(|l| l.previous)),
                (display.next.clone().upcast(), rule(|l| l.next)),
            ],
        );
        let positioning = &self.pages.positioning;
        let in_box = |at: fn([f64; 4], f64) -> f64| -> Rule {
            Box::new(move |widget, width, height| {
                let track = pv::track_box(width, height);
                let h = natural_height(widget, track[2]);
                Rect { x: track[0], y: at(track, h), width: track[2], height: h }
            })
        };
        place(
            &positioning.root,
            vec![
                (positioning.upright.clone().upcast(), row_at(|height, _| 0.09 * height)),
                (positioning.welcome_one.clone().upcast(), in_box(|t, h| t[1] + (t[3] - h) / 2.0)),
                (positioning.welcome_two.clone().upcast(), in_box(|t, h| t[1] + t[3] - 40.0 - h)),
                (positioning.hint.clone().upcast(), in_box(|t, h| t[1] + t[3] - 40.0 - h)),
                (positioning.status.clone().upcast(), row_at(|height, h| 0.88 * height - h)),
            ],
        );
        let message = || -> Rule {
            Box::new(|widget, width, height| {
                let column = pages::MESSAGE_WIDTH.min(width);
                let h = natural_height(widget, column);
                Rect { x: (width - column) / 2.0, y: (height - h) / 2.0, width: column, height: h }
            })
        };
        place(&self.pages.failed.root, vec![(self.pages.failed.column.clone().upcast(), message())]);
        place(&self.pages.profile.root, vec![(self.pages.profile.column.clone().upcast(), message())]);
        let calibration = &self.pages.calibration;
        place(
            &calibration.root,
            vec![
                (calibration.welcome.clone().upcast(), row_at(|height, _| height / 2.0 - 120.0)),
                (calibration.instruction.clone().upcast(), row_at(|height, _| height / 2.0 - 120.0)),
            ],
        );
    }

    // Actions ---------------------------------------------------------------------------------

    fn save_profile(&self) {
        let name = self.pages.profile.entry.text().to_string();
        self.act(Action::SaveProfile { name });
    }

    fn close_window(&self) {
        if self.state.borrow().outcome.is_some() {
            self.window.destroy();
        }
    }

    fn this_display(&self) {
        let now = self.now();
        if self.state.borrow_mut().alignment.choose_this_display(now) {
            self.pages.display.this_display.set_sensitive(false);
            self.request_fullscreen();
        }
    }

    fn finish_lines(&self) {
        let can_finish = self.fullscreen_view();
        let done = self.state.borrow_mut().alignment.finish(can_finish);
        if let Some((left, right)) = done {
            self.pages.display.done.set_sensitive(false);
            self.act(Action::Done { left, right });
        }
    }

    fn switch_monitor(&self, action: Action) {
        let ScreenKind::Display(lines) = &self.state.borrow().screen.kind.clone() else { return };
        let allowed = match action {
            Action::Previous => lines.previous,
            _ => lines.next,
        };
        if !allowed {
            return;
        }
        self.pages.display.previous.set_sensitive(false);
        self.pages.display.next.set_sensitive(false);
        self.act(action);
    }

    fn key(&self, key: Key, shift: bool) -> glib::Propagation {
        let kind = self.state.borrow().screen.kind.clone();
        match respond(&kind, key) {
            KeyResponse::Pass => return glib::Propagation::Proceed,
            KeyResponse::Send(action) => self.act(action),
            KeyResponse::SaveProfile => self.save_profile(),
            KeyResponse::Close => self.close_window(),
            KeyResponse::ToggleFullscreen => {
                if self.window.is_fullscreen() {
                    self.window.unfullscreen();
                } else {
                    self.window.fullscreen();
                }
            }
            KeyResponse::Continue => {
                self.request_fullscreen();
                self.act(Action::Continue);
            }
            KeyResponse::Lines(line_key) => {
                let (can_finish, now) = (self.fullscreen_view(), self.now());
                let outcome = self.state.borrow_mut().alignment.key(line_key, shift, can_finish, now);
                match outcome {
                    KeyOutcome::Ignored => return glib::Propagation::Proceed,
                    KeyOutcome::Redraw => self.pages.display.canvas.queue_draw(),
                    KeyOutcome::ThisDisplay => {
                        self.pages.display.this_display.set_sensitive(false);
                        self.request_fullscreen();
                    }
                    KeyOutcome::Done(left, right) => {
                        self.pages.display.done.set_sensitive(false);
                        self.act(Action::Done { left, right });
                    }
                }
            }
        }
        glib::Propagation::Stop
    }

    // Rendering ---------------------------------------------------------------------------------

    fn poll(self: &Rc<Self>) {
        let now = self.now();
        self.report_view(now);
        let snapshot = self.channel.snapshot();
        self.state.borrow_mut().eyes = snapshot.eyes;
        if snapshot.screen.seq != self.state.borrow().screen.seq {
            self.render(snapshot.screen, now);
        }
        self.animate(now);
        self.check_finished();
    }

    fn report_view(&self, now: f64) {
        let Some(view) = self.view() else { return };
        let (changed, stale) = {
            let state = self.state.borrow();
            (state.sent_view != Some(view), now - state.view_reported_at >= VIEW_REPORT_S)
        };
        if changed || stale {
            self.channel.window_event(self.event(None));
            let mut state = self.state.borrow_mut();
            state.sent_view = Some(view);
            state.view_reported_at = now;
        }
        if (view.height - self.state.borrow().css_height).abs() >= 1.0 {
            self.css.load_from_string(&style::css(view.height));
            self.state.borrow_mut().css_height = view.height;
        }
        let scale = self.window.surface().map_or(1.0, |s| s.scale());
        self.state.borrow_mut().alignment.resize(view.width, view.height, scale);
    }

    fn render(self: &Rc<Self>, screen: Screen, now: f64) {
        let previous = std::mem::replace(&mut self.state.borrow_mut().screen, screen.clone());
        let seq = screen.seq;
        self.stack.set_visible_child_name(section(&screen.kind));
        self.close.set_visible(screen.kind.shows_close());
        if matches!(screen.kind, ScreenKind::Calibration(_)) {
            self.window.add_css_class("calibrating");
        } else {
            self.window.remove_css_class("calibrating");
        }
        let p = &self.pages;
        match &screen.kind {
            ScreenKind::Connecting => {
                p.busy.connecting.set_visible(true);
                p.busy.title.set_visible(false);
                self.ack(seq);
            }
            ScreenKind::Busy { title } => {
                p.busy.connecting.set_visible(false);
                p.busy.title.set_visible(true);
                p.busy.title.set_text(title);
                self.ack(seq);
            }
            ScreenKind::Moving { to } => {
                p.busy.connecting.set_visible(false);
                p.busy.title.set_visible(false);
                self.ack(seq);
                self.move_to(to.clone());
            }
            ScreenKind::Display(lines) => {
                self.state.borrow_mut().alignment.show(lines.clone(), now);
                self.state.borrow_mut().lines_shown = None;
                for (button, shown) in [(&p.display.previous, lines.previous), (&p.display.next, lines.next)] {
                    button.set_opacity(if shown { 1.0 } else { 0.0 });
                    button.set_can_target(shown);
                    button.set_sensitive(true);
                }
                p.display.this_display.set_sensitive(true);
                self.ack(seq);
            }
            ScreenKind::DisplaySave { summary, .. } => {
                p.save.summary.set_text(summary);
                p.save.save.grab_focus();
                self.ack(seq);
            }
            ScreenKind::AspectWarning => {
                p.aspect.yes.grab_focus();
                self.ack(seq);
            }
            ScreenKind::Positioning { sit_upright, leaving } => {
                p.positioning.upright.set_visible(*sit_upright);
                if *leaving {
                    self.state.borrow_mut().leaving_since = Some(now);
                    self.ack_after(seq, pv::BLINK_S + pv::FADE_OUT_S);
                } else {
                    if !matches!(previous.kind, ScreenKind::Positioning { .. }) {
                        let mut state = self.state.borrow_mut();
                        state.positioning_since = now;
                        state.leaving_since = None;
                        state.last_motion = now;
                    }
                    self.ack(seq);
                }
            }
            ScreenKind::Calibration(phase) => {
                self.state.borrow_mut().calibration.apply(phase, now);
                self.ack_after(seq, ack_delay_s(phase));
            }
            ScreenKind::CalibrationFailed => {
                p.failed.retry.grab_focus();
                self.ack(seq);
            }
            ScreenKind::Profile { suggested, problem } => {
                let entry = &p.profile.entry;
                if entry.text() != suggested.as_str() || problem.is_empty() {
                    entry.set_text(suggested);
                }
                p.profile.problem.set_text(problem);
                entry.grab_focus();
                entry.select_region(0, -1);
                self.ack(seq);
            }
            ScreenKind::Finished { title, text }
            | ScreenKind::Cancelled { title, text }
            | ScreenKind::Error { title, text } => {
                p.end.title.set_text(title);
                p.end.text.set_text(text);
                p.end.hint.set_text("");
                p.end.close.grab_focus();
                self.ack(seq);
            }
        }
    }

    fn move_to(self: &Rc<Self>, key: String) {
        if let Some(target) = self.gdk_monitor(&key) {
            let geometry = target.geometry();
            self.window.set_default_size(geometry.width(), geometry.height());
            if !self.windowed {
                self.window.fullscreen_on_monitor(&target);
            }
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(MOVE_SETTLE, move || {
            if let Some(ui) = weak.upgrade() {
                ui.state.borrow_mut().monitor = Some(key);
                ui.channel.window_event(ui.event(None));
            }
        });
    }

    fn animate(&self, now: f64) {
        let kind = self.state.borrow().screen.kind.clone();
        let fullscreen = self.fullscreen_view();
        self.banner.set_visible(!fullscreen && kind.needs_fullscreen());
        let hide_cursor = match kind {
            ScreenKind::Calibration(_) => true,
            ScreenKind::Positioning { .. } => now - self.state.borrow().last_motion > CURSOR_HIDE_S,
            _ => false,
        };
        if self.state.borrow().cursor_hidden != Some(hide_cursor) {
            self.state.borrow_mut().cursor_hidden = Some(hide_cursor);
            self.window.set_cursor_from_name(Some(if hide_cursor { "none" } else { "default" }));
        }
        let p = &self.pages;
        match kind {
            ScreenKind::Connecting | ScreenKind::Busy { .. } | ScreenKind::Moving { .. } => p.busy.spinner.queue_draw(),
            ScreenKind::Display(lines) => self.animate_display(&lines, fullscreen, now),
            ScreenKind::Positioning { .. } => self.animate_positioning(now),
            ScreenKind::Calibration(_) => {
                let overlays = {
                    let mut state = self.state.borrow_mut();
                    state.calibration.tick(now);
                    state.calibration.overlays(now)
                };
                p.calibration.welcome.set_opacity(overlays.welcome);
                p.calibration.instruction.set_opacity(overlays.instruction);
                p.calibration.canvas.queue_draw();
            }
            _ => {}
        }
    }

    fn animate_display(&self, lines: &crate::screen::DisplayLines, fullscreen: bool, now: f64) {
        let display = &self.pages.display;
        let (shows_lines, shown_at, animating, done_enabled) = {
            let mut state = self.state.borrow_mut();
            let was_animating = state.alignment.animating();
            state.alignment.tick(now);
            let alignment = &state.alignment;
            (alignment.shows_lines(), alignment.shown_at, was_animating, alignment.done_enabled(fullscreen))
        };
        if self.state.borrow().lines_shown != Some(shows_lines) {
            self.state.borrow_mut().lines_shown = Some(shows_lines);
            if shows_lines {
                display.explaining.set_text(lines.instructions);
                display.headline.set_visible(false);
            } else {
                display.explaining.set_text(alignment::WHERE);
                display.headline.set_text(alignment::ATTACH);
                display.headline.set_visible(true);
            }
            display.this_display.set_visible(!shows_lines);
            display.done.set_visible(shows_lines);
        }
        let opacity = fade_in(now - shown_at);
        display.text.set_opacity(opacity);
        display.buttons.set_opacity(opacity);
        display.done.set_sensitive(done_enabled);
        if animating || opacity < 1.0 {
            display.canvas.queue_draw();
        }
    }

    fn animate_positioning(&self, now: f64) {
        let p = &self.pages.positioning;
        let (elapsed, leaving, eyes) = {
            let state = self.state.borrow();
            (now - state.positioning_since, state.leaving_since, state.eyes)
        };
        let (one, two, hint) = pv::text_opacities(elapsed);
        let leaving = leaving.is_some();
        p.welcome_one.set_opacity(if leaving { 0.0 } else { one });
        p.welcome_two.set_opacity(if leaving { 0.0 } else { two });
        p.hint.set_opacity(if leaving { 0.0 } else { hint });
        if let Some(eyes) = eyes.filter(|_| !leaving) {
            if p.hint.text() != eyes.hint {
                p.hint.set_text(eyes.hint);
            }
            let status = pv::status_text(eyes.good, elapsed);
            if p.status.text() != status {
                p.status.set_text(status);
            }
        } else if leaving {
            p.status.set_text("");
        }
        p.canvas.queue_draw();
    }

    fn draw_positioning(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let now = self.now();
        let state = self.state.borrow();
        let [x, y, w, h] = pv::track_box(width, height);
        let (blink, box_opacity) = state.leaving_since.map_or((1.0, 1.0), |since| pv::leaving(now, since));
        draw::fill_rect(cr, [x, y, w, h], [1.0, 1.0, 1.0, 0.03 * box_opacity]);
        let Some(eyes) = state.eyes else { return };
        let elapsed = now - state.positioning_since;
        cr.save().ok();
        cr.rectangle(x, y, w, h);
        cr.clip();
        cr.translate(x, y);
        for (eye, vertical) in [(eyes.eyes.left, 1.0), (eyes.eyes.right, blink)] {
            let Some(eye) = eye else { continue };
            let drawn = pv::eye_draw(&eye, eyes.eyes.size, w, h, elapsed);
            draw::disc(cr, drawn.centre, drawn.diameter, drawn.grey, drawn.opacity * box_opacity, vertical);
        }
        cr.restore().ok();
    }

    fn check_finished(&self) {
        if self.state.borrow().outcome.is_some() {
            return;
        }
        let Some(outcome) = self.finished.lock().unwrap_or_else(PoisonError::into_inner).take() else { return };
        println!("{}: {}", outcome.status.name(), outcome.message);
        for path in &outcome.backups {
            println!("backup: {}", path.display());
        }
        if let Some(id) = outcome.monitor.as_ref().and_then(|m| m.get("instance_id")).and_then(Json::as_str) {
            println!("monitor: {id} (serve --display-id for window tracking)");
        }
        self.exit.set(self.exit_codes.status(outcome.status));
        let close = self.state.borrow().close_requested || outcome.closes_window;
        self.state.borrow_mut().outcome = Some(outcome);
        if close {
            self.window.destroy();
        }
    }
}
