//! The GTK 4 app shell: one instance per session, the tray and the service link on the GTK
//! main loop, on/off from the tray, Tobii's quit and `--guest`.
//!
//! The link and the tray report on their own threads. Both feed one bounded `async_channel`
//! that a future on the GTK main context drains, so every state change happens on the GTK
//! thread without polling (glib 0.22 has no `MainContext::channel`). Gaze samples are dropped
//! while the queue is full; status, failures and tray actions wait for room.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;
use ksni::blocking::{Handle, TrayMethods};
use tobii_control::{Request, Status};
use tobii_gaze::profiles::ProfileStore;

use crate::cli::{self, APP_ID, Command, Options};
use crate::guest::{GUEST_ARGUMENTS, SetupLauncher, SetupProcess, guest_allowed, setup_binary, setup_result};
use crate::link::{Link, LinkUpdate};
use crate::page::{self, PageEvent, PageModel};
use crate::quit::{self, QuitStep, ServiceUnit, Systemctl, quit_step};
use crate::settings::Settings;
use crate::status::{DeviceStatus, IconState, device_status, icon_state};
use crate::tray::{Tray, TrayAction};

/// The page window's title and width (docs/experience-app.md section 4.1).
pub const PAGE_TITLE: &str = "Tobii Experience";
pub const PAGE_WIDTH: i32 = 550;
/// The height of Tobii's host window, which the 550 px sidebar fills.
pub const PAGE_HEIGHT: i32 = 810;
/// Events queued for the GTK thread: a few seconds of gaze samples.
const EVENT_QUEUE: usize = 256;

/// What the app needs from the system; tests replace the parts that act on it.
#[derive(Debug)]
pub struct Deps {
    pub service: Arc<dyn ServiceUnit>,
    pub setup: Rc<dyn SetupLauncher>,
    /// Where the settings live; `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// Whether to publish the tray icon on the session bus.
    pub tray: bool,
    /// The calibration profiles the page lists, switches and deletes.
    pub profiles: ProfileStore,
}

impl Deps {
    /// `systemctl --user`, `tobii-linux-setup` pointed at the service at `service`, the
    /// settings file and the tray.
    pub fn system(service: SocketAddr) -> Deps {
        let exe = std::env::current_exe().ok();
        Deps {
            service: Arc::new(Systemctl),
            setup: Rc::new(SetupProcess::new(setup_binary(exe.as_deref()), service)),
            settings_path: Settings::default_path(),
            tray: true,
            profiles: ProfileStore::default_location(),
        }
    }
}

/// The `gtk::Application` and, from its first command line until shutdown, the
/// [`Controller`].
#[derive(Debug, Clone)]
pub struct Shell {
    app: gtk::Application,
    controller: Rc<RefCell<Option<Controller>>>,
}

impl Shell {
    /// A unique application with id [`APP_ID`]. The instance that owns the id builds the
    /// controller from its own command line and then acts on every forwarded one; any other
    /// instance forwards its command line and exits with the primary's answer. Shutdown
    /// releases the controller, which stops the link and the tray.
    pub fn new(deps: Deps) -> Shell {
        let app = gtk::Application::builder()
            .application_id(APP_ID)
            .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
            .build();
        let controller: Rc<RefCell<Option<Controller>>> = Rc::new(RefCell::new(None));
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
            let existing = cell.borrow().clone();
            let controller = match existing {
                Some(controller) => controller,
                None => {
                    let Some(deps) = deps.borrow_mut().take() else {
                        log::warn!("ignored a command line after shutdown");
                        return glib::ExitCode::FAILURE;
                    };
                    let controller = Controller::new(app, &options, deps);
                    *cell.borrow_mut() = Some(controller.clone());
                    controller
                }
            };
            controller.handle(&options, command_line.is_remote());
            glib::ExitCode::SUCCESS
        });
        let cell = controller.clone();
        app.connect_shutdown(move |_| {
            let released = cell.borrow_mut().take();
            if let Some(controller) = &released {
                page::on_shutdown(controller);
            }
            drop(released);
        });
        Shell { app, controller }
    }

    pub fn application(&self) -> &gtk::Application {
        &self.app
    }

    /// The controller, once this instance became the primary one.
    pub fn controller(&self) -> Option<Controller> {
        self.controller.borrow().clone()
    }

    /// Runs the application with `argv` (program name first).
    pub fn run<I: IntoIterator<Item = String>>(&self, argv: I) -> glib::ExitCode {
        let argv: Vec<String> = argv.into_iter().collect();
        self.app.run_with_args(&argv)
    }
}

pub(crate) enum AppEvent {
    Link(LinkUpdate),
    Tray(TrayAction),
    Page(PageEvent),
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

/// A [`Controller`] that does not keep the app alive, for callbacks that outlive a call.
#[derive(Debug, Clone)]
pub(crate) struct WeakController(Weak<Inner>);

impl WeakController {
    pub(crate) fn upgrade(&self) -> Option<Controller> {
        self.0.upgrade().map(Controller)
    }
}

struct Inner {
    app: gtk::Application,
    events: async_channel::Sender<AppEvent>,
    _hold: gio::ApplicationHoldGuard,
    link: Link,
    tray: Option<Handle<Tray>>,
    service: Arc<dyn ServiceUnit>,
    service_unit: String,
    setup: Rc<dyn SetupLauncher>,
    settings: RefCell<Settings>,
    settings_path: Option<PathBuf>,
    /// `--no-banner` and `--no-auto-setup` turn these off for this run only.
    banner: bool,
    auto_setup: bool,
    state: RefCell<ServiceState>,
    quitting: Cell<bool>,
    /// The confirmation dialog, a stop in flight or the failure box is up.
    quit_busy: Cell<bool>,
    page: RefCell<Option<gtk::Window>>,
    update_hooks: RefCell<Vec<UpdateHook>>,
    profiles: ProfileStore,
    page_model: PageModel,
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

impl Drop for Inner {
    /// Closes the queue first so that a link or tray thread waiting for room gives up, then
    /// stops the tray; the link thread is joined when `link` drops.
    fn drop(&mut self) {
        self.events.close();
        if let Some(tray) = &self.tray {
            tray.shutdown().wait();
        }
    }
}

impl Controller {
    /// Starts the service unit, the link and the tray for `app`, which this controller keeps
    /// running without windows.
    pub fn new(app: &gtk::Application, options: &Options, deps: Deps) -> Controller {
        let (events, received) = async_channel::bounded(EVENT_QUEUE);
        let link_events = events.clone();
        let link = Link::start(options.service, move |update| queue_link_update(&link_events, update));
        let tray = if deps.tray { spawn_tray(events.clone()) } else { None };
        let settings = match &deps.settings_path {
            Some(path) => Settings::load(path).unwrap_or_else(|error| {
                log::warn!("{}: {error}; using the default settings", path.display());
                Settings::default()
            }),
            None => Settings::default(),
        };
        let controller = Controller(Rc::new(Inner {
            app: app.clone(),
            events,
            _hold: app.hold(),
            link,
            tray,
            service: deps.service,
            service_unit: options.service_unit.clone(),
            setup: deps.setup,
            settings: RefCell::new(settings),
            settings_path: deps.settings_path,
            banner: options.banner,
            auto_setup: options.auto_setup,
            state: RefCell::new(ServiceState::Initializing),
            quitting: Cell::new(false),
            quit_busy: Cell::new(false),
            page: RefCell::new(None),
            update_hooks: RefCell::new(Vec::new()),
            profiles: deps.profiles,
            page_model: PageModel::default(),
        }));
        controller.start_service();
        crate::overlay::attach(&controller);
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

    pub(crate) fn downgrade(&self) -> WeakController {
        WeakController(self.weak())
    }

    /// The queue to the GTK thread, for work that ends on another thread.
    pub(crate) fn events(&self) -> async_channel::Sender<AppEvent> {
        self.0.events.clone()
    }

    pub(crate) fn page_model(&self) -> &PageModel {
        &self.0.page_model
    }

    pub(crate) fn setup(&self) -> &dyn SetupLauncher {
        &*self.0.setup
    }

    pub(crate) fn set_page_window(&self, window: Option<gtk::Window>) {
        self.0.page.replace(window);
    }

    /// The calibration profiles the page works on.
    pub fn profiles(&self) -> &ProfileStore {
        &self.0.profiles
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

    /// Whether the page shows its set-up banner: neither `--no-banner` nor the setting hides it.
    pub fn shows_banner(&self) -> bool {
        self.0.banner && self.0.settings.borrow().show_banner
    }

    /// Whether the app may start the setup by itself: neither `--no-auto-setup` nor the
    /// setting turns it off.
    pub fn auto_setup(&self) -> bool {
        self.0.auto_setup && self.0.settings.borrow().auto_setup
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

    /// [`Controller::send`], then `done` on the link thread once the reply was reported.
    pub(crate) fn send_then(&self, request: Request, done: impl FnOnce() + Send + 'static) {
        self.0.link.send_then(request, done);
    }

    /// Calls `hook` on the GTK thread for every link update, after the controller took it in.
    /// The hook gets the controller as an argument; capturing a clone of it would keep the
    /// controller, its link and its tray alive after shutdown.
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

    /// Opens the Eye tracking page window, or raises it when it is open.
    pub fn open_page(&self) -> gtk::Window {
        if let Some(window) = self.page_window() {
            window.present();
            return window;
        }
        let window = page::open_window(self);
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
        let on_exit = Box::new(|exit| log::info!("the guest calibration ended: {:?} ({exit:?})", setup_result(&exit)));
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

    /// Acts on a command line: the own one at start or one a second instance forwarded. A
    /// second launch without `--guest` presents the page of the running app.
    fn handle(&self, options: &Options, remote: bool) {
        if options.page || (remote && !options.guest) {
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
                if let LinkUpdate::Status(_) = update {
                    page::on_service_update(self);
                }
            }
            AppEvent::Page(event) => page::on_page_event(self, event),
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

/// Queues a link update for the GTK thread: a gaze sample is dropped when the queue is full,
/// anything else waits for room (or for the queue to close at shutdown).
fn queue_link_update(events: &async_channel::Sender<AppEvent>, update: LinkUpdate) {
    let event = AppEvent::Link(update);
    let _ = match event {
        AppEvent::Link(LinkUpdate::Gaze(_)) => events.try_send(event).map_err(drop),
        _ => events.send_blocking(event).map_err(drop),
    };
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tobii_control::GazeSample;

    use super::*;

    fn gaze() -> LinkUpdate {
        LinkUpdate::Gaze(GazeSample { timestamp_us: 1, x: 0.5, y: 0.5, valid: true })
    }

    fn received(events: &async_channel::Receiver<AppEvent>) -> Option<LinkUpdate> {
        match events.try_recv().ok()? {
            AppEvent::Link(update) => Some(update),
            AppEvent::Tray(_) | AppEvent::Page(_) => None,
        }
    }

    #[test]
    fn a_full_queue_drops_gaze_but_keeps_status() {
        let (events, queue) = async_channel::bounded(1);
        queue_link_update(&events, gaze());
        queue_link_update(&events, gaze());
        assert_eq!(queue.len(), 1, "the second sample is dropped, not queued or waited for");

        let sender = events.clone();
        let status = std::thread::spawn(move || queue_link_update(&sender, LinkUpdate::Status(None)));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!status.is_finished(), "a status waits for room");
        assert_eq!(received(&queue), Some(gaze()));
        status.join().unwrap();
        assert_eq!(received(&queue), Some(LinkUpdate::Status(None)));
    }

    #[test]
    fn closing_the_queue_releases_a_waiting_status() {
        let (events, queue) = async_channel::bounded(1);
        queue_link_update(&events, LinkUpdate::Failed("full".into()));
        let sender = events.clone();
        let waiting = std::thread::spawn(move || queue_link_update(&sender, LinkUpdate::Status(None)));
        std::thread::sleep(Duration::from_millis(100));
        events.close();
        waiting.join().unwrap();
        drop(queue);
    }
}
