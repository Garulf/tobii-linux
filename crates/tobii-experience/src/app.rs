//! The GTK 4 app shell: one instance per session, the tray and the service link on the GTK
//! main loop, on/off from the tray, Tobii's quit and `--guest`.
//!
//! The link and the tray report on their own threads. Both feed one unbounded
//! `async_channel` that a future on the GTK main context drains, so every state change
//! happens on the GTK thread without polling (glib 0.22 has no `MainContext::channel`).

use std::cell::{Cell, OnceCell, RefCell};
use std::fmt;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;
use ksni::blocking::{Handle, TrayMethods};
use tobii_control::{Request, Status};

use crate::cli::{self, APP_ID, Command, Options};
use crate::guest::{GUEST_ARGUMENTS, SetupLauncher, SetupProcess, guest_allowed, setup_binary};
use crate::link::{Link, LinkUpdate};
use crate::quit::{self, QuitStep, ServiceUnit, Systemctl, quit_step};
use crate::settings::Settings;
use crate::status::{DeviceStatus, IconState, device_status, icon_state};
use crate::tray::{Tray, TrayAction};

/// The page window's title and width (docs/experience-app.md section 4.1).
pub const PAGE_TITLE: &str = "Tobii Experience";
pub const PAGE_WIDTH: i32 = 550;
/// The height of Tobii's host window, which the 550 px sidebar fills.
pub const PAGE_HEIGHT: i32 = 810;

/// What the app needs from the system; tests replace the parts that act on it.
#[derive(Debug)]
pub struct Deps {
    pub service: Arc<dyn ServiceUnit>,
    pub setup: Rc<dyn SetupLauncher>,
    /// Where the settings live; `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// Whether to publish the tray icon on the session bus.
    pub tray: bool,
}

impl Deps {
    /// `systemctl --user`, `tobii-linux-setup`, the settings file and the tray.
    pub fn system() -> Deps {
        let exe = std::env::current_exe().ok();
        Deps {
            service: Arc::new(Systemctl),
            setup: Rc::new(SetupProcess::new(setup_binary(exe.as_deref()))),
            settings_path: Settings::default_path(),
            tray: true,
        }
    }
}

/// The `gtk::Application` and, once its first command line arrived, the [`Controller`].
#[derive(Debug, Clone)]
pub struct Shell {
    app: gtk::Application,
    controller: Rc<OnceCell<Controller>>,
}

impl Shell {
    /// A unique application with id [`APP_ID`]. The instance that owns the id builds the
    /// controller from its own command line and then acts on every forwarded one; any other
    /// instance forwards its command line and exits with the primary's answer.
    pub fn new(deps: Deps) -> Shell {
        let app = gtk::Application::builder()
            .application_id(APP_ID)
            .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
            .build();
        let controller = Rc::new(OnceCell::new());
        let deps = RefCell::new(Some(deps));
        let cell = controller.clone();
        app.connect_command_line(move |app, command_line| {
            let arguments = command_line.arguments().into_iter().skip(1).map(|a| a.to_string_lossy().into_owned());
            let options = match cli::parse(arguments) {
                Ok(Command::Run(options)) => options,
                Ok(Command::Help) => return glib::ExitCode::SUCCESS,
                Err(error) => {
                    log::warn!("ignored a command line: {error}");
                    return glib::ExitCode::from(2);
                }
            };
            let controller = cell.get_or_init(|| {
                let deps = deps.borrow_mut().take().expect("the controller is built once");
                Controller::new(app, &options, deps)
            });
            controller.handle(&options);
            glib::ExitCode::SUCCESS
        });
        Shell { app, controller }
    }

    pub fn application(&self) -> &gtk::Application {
        &self.app
    }

    /// The controller, once this instance became the primary one.
    pub fn controller(&self) -> Option<Controller> {
        self.controller.get().cloned()
    }

    /// Runs the application with `argv` (program name first).
    pub fn run<I: IntoIterator<Item = String>>(&self, argv: I) -> glib::ExitCode {
        let argv: Vec<String> = argv.into_iter().collect();
        self.app.run_with_args(&argv)
    }
}

enum AppEvent {
    Link(LinkUpdate),
    Tray(TrayAction),
}

/// What the app knows of the service.
#[derive(Debug, Clone)]
enum ServiceState {
    /// No status since the link started: Tobii's `Initializing`.
    Initializing,
    /// The latest status; `None` while the service is unreachable.
    Reported(Option<Status>),
}

type UpdateHook = Rc<dyn Fn(&Controller, &LinkUpdate)>;

/// The primary instance's state, owned by the GTK thread; cheap to clone.
#[derive(Clone)]
pub struct Controller(Rc<Inner>);

struct Inner {
    app: gtk::Application,
    _hold: gio::ApplicationHoldGuard,
    link: Link,
    tray: Option<Handle<Tray>>,
    service: Arc<dyn ServiceUnit>,
    service_unit: String,
    setup: Rc<dyn SetupLauncher>,
    settings: RefCell<Settings>,
    settings_path: Option<PathBuf>,
    state: RefCell<ServiceState>,
    quitting: Cell<bool>,
    /// The confirmation dialog, a stop in flight or the failure box is up.
    quit_busy: Cell<bool>,
    page: RefCell<Option<gtk::Window>>,
    update_hooks: RefCell<Vec<UpdateHook>>,
}

impl fmt::Debug for Controller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = &self.0;
        f.debug_struct("Controller")
            .field("link", &inner.link)
            .field("tray", &inner.tray.is_some())
            .field("service_unit", &inner.service_unit)
            .field("settings", &inner.settings.borrow())
            .field("state", &inner.state.borrow())
            .field("quitting", &inner.quitting.get())
            .finish_non_exhaustive()
    }
}

impl Controller {
    /// Starts the service unit, the link and the tray for `app`, which this controller keeps
    /// running without windows.
    pub fn new(app: &gtk::Application, options: &Options, deps: Deps) -> Controller {
        let (events, received) = async_channel::unbounded();
        let link_events = events.clone();
        let link = Link::start(options.service, move |update| {
            let _ = link_events.send_blocking(AppEvent::Link(update));
        });
        let tray = if deps.tray { spawn_tray(events) } else { None };
        let settings = match &deps.settings_path {
            Some(path) => Settings::load(path).unwrap_or_else(|error| {
                log::warn!("{}: {error}; using the default settings", path.display());
                Settings::default()
            }),
            None => Settings::default(),
        };
        let controller = Controller(Rc::new(Inner {
            app: app.clone(),
            _hold: app.hold(),
            link,
            tray,
            service: deps.service,
            service_unit: options.service_unit.clone(),
            setup: deps.setup,
            settings: RefCell::new(settings),
            settings_path: deps.settings_path,
            state: RefCell::new(ServiceState::Initializing),
            quitting: Cell::new(false),
            quit_busy: Cell::new(false),
            page: RefCell::new(None),
            update_hooks: RefCell::new(Vec::new()),
        }));
        controller.start_service();
        let weak = Rc::downgrade(&controller.0);
        glib::spawn_future_local(async move {
            while let Ok(event) = received.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                Controller(inner).on_event(event);
            }
        });
        controller
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    pub fn application(&self) -> &gtk::Application {
        &self.0.app
    }

    /// The latest service status; `None` before the first one and while unreachable.
    pub fn status(&self) -> Option<Status> {
        match &*self.0.state.borrow() {
            ServiceState::Initializing => None,
            ServiceState::Reported(status) => status.clone(),
        }
    }

    /// Initializing until the first status after start, then [`device_status`].
    pub fn device_status(&self) -> DeviceStatus {
        match &*self.0.state.borrow() {
            ServiceState::Initializing => DeviceStatus::Initializing,
            ServiceState::Reported(status) => device_status(status.as_ref()),
        }
    }

    /// Tobii's `EyeTrackingEnabledState == true`; false while it is unknown.
    pub fn enabled(&self) -> bool {
        self.status().is_some_and(|status| status.enabled)
    }

    pub fn quitting(&self) -> bool {
        self.0.quitting.get()
    }

    pub fn settings(&self) -> Settings {
        self.0.settings.borrow().clone()
    }

    /// Stores `settings` and keeps them; a failed write is logged and the change kept.
    pub fn set_settings(&self, settings: Settings) {
        if let Some(path) = &self.0.settings_path
            && let Err(error) = settings.store(path)
        {
            log::warn!("{}: {error}", path.display());
        }
        *self.0.settings.borrow_mut() = settings;
    }

    /// Queues a request for the service; the reply arrives as a [`LinkUpdate`].
    pub fn send(&self, request: Request) {
        self.0.link.send(request);
    }

    /// Calls `hook` on the GTK thread for every link update, after the controller took it in.
    pub fn connect_update(&self, hook: impl Fn(&Controller, &LinkUpdate) + 'static) {
        self.0.update_hooks.borrow_mut().push(Rc::new(hook));
    }

    /// The tray's enable or disable item: `SetEnabled(!enabled)`.
    pub fn toggle_enabled(&self) {
        self.send(Request::SetEnabled(!self.enabled()));
    }

    /// The page window, when it is open.
    pub fn page_window(&self) -> Option<gtk::Window> {
        self.0.page.borrow().clone()
    }

    /// Opens the page window, or raises it when it is open.
    pub fn open_page(&self) -> gtk::Window {
        if let Some(window) = self.page_window() {
            window.present();
            return window;
        }
        let window: gtk::Window = gtk::ApplicationWindow::builder()
            .application(&self.0.app)
            .title(PAGE_TITLE)
            .default_width(PAGE_WIDTH)
            .default_height(PAGE_HEIGHT)
            .build()
            .upcast();
        let weak = self.weak();
        window.connect_close_request(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.page.replace(None);
            }
            glib::Propagation::Proceed
        });
        self.0.page.replace(Some(window.clone()));
        window.present();
        window
    }

    /// `--guest`: runs the guest calibration when Tobii's status gate allows it; true when
    /// setup was started.
    pub fn guest(&self) -> bool {
        let status = self.device_status();
        if !guest_allowed(status) {
            log::info!("--guest ignored: the device status is {status:?}");
            return false;
        }
        let on_exit = Box::new(|exit| log::info!("the guest calibration ended: {exit:?}"));
        match self.0.setup.launch(&GUEST_ARGUMENTS, on_exit) {
            Ok(()) => true,
            Err(error) => {
                log::info!("--guest: {error}");
                false
            }
        }
    }

    /// The tray's Quit (docs/experience-app.md section 3).
    pub fn quit(&self) {
        let skip = self.0.settings.borrow().skip_quit_confirmation;
        match quit_step(self.0.quit_busy.get(), skip) {
            QuitStep::Ignore => {}
            QuitStep::Stop => self.stop_and_exit(false),
            QuitStep::Confirm => {
                self.0.quit_busy.set(true);
                let weak = self.weak();
                quit::dialog::confirm(&self.0.app, move |answer| {
                    let Some(inner) = weak.upgrade() else { return };
                    let controller = Controller(inner);
                    controller.0.quit_busy.set(false);
                    if let Some(dont_show_again) = answer {
                        controller.stop_and_exit(dont_show_again);
                    }
                });
            }
        }
    }

    /// Tobii's `ButtonQuit`: persist the checkbox, show Configuring, stop the unit, exit; on
    /// failure ask to retry.
    fn stop_and_exit(&self, dont_show_again: bool) {
        if dont_show_again {
            let mut settings = self.settings();
            settings.skip_quit_confirmation = true;
            self.set_settings(settings);
        }
        self.0.quitting.set(true);
        self.0.quit_busy.set(true);
        self.refresh_tray();
        let service = self.0.service.clone();
        let unit = self.0.service_unit.clone();
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let stopped = gio::spawn_blocking(move || service.stop(&unit))
                .await
                .unwrap_or_else(|_| Err("the stop panicked".into()));
            let Some(inner) = weak.upgrade() else { return };
            let controller = Controller(inner);
            match stopped {
                Ok(()) => controller.0.app.quit(),
                Err(error) => {
                    log::warn!("quit: {error}");
                    let weak = controller.weak();
                    quit::dialog::failure(&controller.0.app, move |retry| {
                        let Some(inner) = weak.upgrade() else { return };
                        let controller = Controller(inner);
                        if retry {
                            controller.stop_and_exit(dont_show_again);
                        } else {
                            controller.0.quitting.set(false);
                            controller.0.quit_busy.set(false);
                            controller.refresh_tray();
                        }
                    });
                }
            }
        });
    }

    /// Tobii clears its exit flag when the app launches; here that is starting the unit.
    fn start_service(&self) {
        let service = self.0.service.clone();
        let unit = self.0.service_unit.clone();
        glib::spawn_future_local(async move {
            let started = gio::spawn_blocking(move || service.start(&unit))
                .await
                .unwrap_or_else(|_| Err("the start panicked".into()));
            if let Err(error) = started {
                log::warn!("start: {error}");
            }
        });
    }

    /// Acts on a command line: the own one at start or one a second instance forwarded.
    fn handle(&self, options: &Options) {
        if options.page {
            self.open_page();
        }
        if options.guest {
            self.guest();
        }
    }

    fn on_event(&self, event: AppEvent) {
        match event {
            AppEvent::Link(update) => {
                match &update {
                    LinkUpdate::Status(status) => {
                        *self.0.state.borrow_mut() = ServiceState::Reported(status.clone());
                        self.refresh_tray();
                    }
                    LinkUpdate::Failed(reason) => log::warn!("service: {reason}"),
                    LinkUpdate::Gaze(_) => {}
                }
                let hooks = self.0.update_hooks.borrow().clone();
                for hook in hooks {
                    hook(self, &update);
                }
            }
            AppEvent::Tray(TrayAction::OpenPage) => {
                self.open_page();
            }
            AppEvent::Tray(TrayAction::ToggleEnabled) => self.toggle_enabled(),
            AppEvent::Tray(TrayAction::Quit) => self.quit(),
        }
    }

    fn icon_state(&self) -> IconState {
        icon_state(self.device_status(), self.quitting())
    }

    fn refresh_tray(&self) {
        if let Some(tray) = &self.0.tray {
            let (state, enabled) = (self.icon_state(), self.enabled());
            tray.update(|tray| tray.update(state, enabled));
        }
    }
}

/// Publishes the tray; its actions reach the GTK thread through `events`. Without a session
/// bus the app runs without a tray. The tray waits for a StatusNotifierWatcher that appears
/// later (a bar started after the app).
fn spawn_tray(events: async_channel::Sender<AppEvent>) -> Option<Handle<Tray>> {
    let (actions, received) = std::sync::mpsc::channel();
    let forward = std::thread::Builder::new().name("tobii-experience-tray".into()).spawn(move || {
        for action in received {
            if events.send_blocking(AppEvent::Tray(action)).is_err() {
                break;
            }
        }
    });
    if let Err(error) = forward {
        log::warn!("no tray: {error}");
        return None;
    }
    match Tray::new(actions, IconState::Initializing, false).assume_sni_available(true).spawn() {
        Ok(handle) => Some(handle),
        Err(error) => {
            log::warn!("no tray: {error}");
            None
        }
    }
}
