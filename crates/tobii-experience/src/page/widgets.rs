//! The Eye tracking page in GTK 4: the window, its sections in Tobii's order, and the page
//! state the controller keeps for it (busy marker, preview request, profile list).

use std::cell::{Cell, RefCell};
use std::fmt;
use std::io;
use std::process::ExitStatus;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use gtk4 as gtk;
use tobii_control::{Request, TrackedEyes};

use super::style::{self, Icon, Paint, Segment};
use super::{
    Availability, BOTH_EYES, DISABLE_DELAY, EYES_BODY, EYES_TITLE, GazeCapability, IMPROVE_BODY, IMPROVE_BUTTON,
    IMPROVE_TITLE, LEFT_EYE, NEW_PROFILE_BUTTON, OFF_TEXT, ON_TEXT, PREVIEW_BODY, PREVIEW_TITLE, PROFILE_BODY,
    PROFILE_TITLE, PageAction, PageState, ProfileList, RIGHT_EYE, SCREEN_BODY, SCREEN_BUTTON, SCREEN_TITLE,
    SWITCH_TITLE, SetupFlow, UNDO_TEXT, availability, gaze_capability, page_state, system_enabled, visibility,
};
use crate::app::{AppEvent, Controller, PAGE_HEIGHT, PAGE_TITLE, PAGE_WIDTH, WeakController};
use crate::guest::{OnExit, SetupResult, setup_result};

/// Work that ends on another thread and comes back to the page.
pub(crate) enum PageEvent {
    CommandDone(AfterCommand),
    SetupEnded(io::Result<ExitStatus>),
}

/// What a finished request does with the busy marker.
#[derive(Debug, Clone, Copy)]
pub(crate) enum AfterCommand {
    Release,
    /// Turning eye tracking off: release after [`DISABLE_DELAY`].
    ReleaseAfterDelay,
    /// A profile switch: release and close the page.
    ReleaseAndClose,
}

type PreviewHook = Rc<dyn Fn(&Controller, bool)>;

/// The page's state on the controller; it outlives the window.
#[derive(Default)]
pub(crate) struct PageModel {
    /// Tobii's `IsPaused` busy marker, held by every command in flight.
    busy: Cell<u32>,
    preview: Cell<bool>,
    /// The tracked-eyes fold-out; navigation does not reset it.
    eyes_open: Cell<bool>,
    /// The value a click asked for, shown until its request is answered.
    pending_enabled: Cell<Option<bool>>,
    pending_eyes: Cell<Option<TrackedEyes>>,
    list: RefCell<Option<ProfileList>>,
    view: RefCell<Option<Rc<PageView>>>,
    preview_hooks: RefCell<Vec<PreviewHook>>,
}

impl fmt::Debug for PageModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageModel")
            .field("busy", &self.busy.get())
            .field("preview", &self.preview.get())
            .field("eyes_open", &self.eyes_open.get())
            .field("list", &self.list.borrow())
            .finish_non_exhaustive()
    }
}

impl Controller {
    /// Tobii's `TrackingDeviceState` for the latest service status.
    pub fn page_state(&self) -> PageState {
        page_state(self.device_status(), self.status().as_ref())
    }

    pub fn gaze_capability(&self) -> GazeCapability {
        let connected = self.settings().gaze_tracker_has_been_connected;
        gaze_capability(self.page_state(), self.status().as_ref(), connected)
    }

    /// The current profile as the profile store has it.
    pub fn current_profile(&self) -> Option<String> {
        self.profiles().current().unwrap_or_else(|error| {
            log::warn!("profiles: {error}");
            None
        })
    }

    /// Which page controls Tobii's `RefreshAvailability` enables now.
    pub fn availability(&self) -> Availability {
        let current = self.current_profile();
        availability(self.page_state(), self.gaze_capability(), current.as_deref(), self.system_enabled())
    }

    /// Tobii's `SystemEnabled`: true while the status is unknown, as Tobii reads a missing
    /// value.
    pub fn system_enabled(&self) -> bool {
        system_enabled(self.status().as_ref())
    }

    /// Whether a page command is in flight; the page is insensitive meanwhile.
    pub fn page_busy(&self) -> bool {
        self.page_model().busy.get() > 0
    }

    /// The "Preview my gaze" toggle: off at every launch, never stored.
    pub fn preview_requested(&self) -> bool {
        self.page_model().preview.get()
    }

    pub fn set_preview_requested(&self, on: bool) {
        let model = self.page_model();
        if model.preview.replace(on) == on {
            return;
        }
        let hooks = model.preview_hooks.borrow().clone();
        for hook in hooks {
            hook(self, on);
        }
        refresh(self);
    }

    /// Calls `hook` with the new value whenever the preview toggle changes.
    pub fn connect_preview(&self, hook: impl Fn(&Controller, bool) + 'static) {
        self.page_model().preview_hooks.borrow_mut().push(Rc::new(hook));
    }

    /// The page's profile list while the page is open.
    pub fn profile_list(&self) -> Option<ProfileList> {
        self.page_model().list.borrow().clone()
    }
}

/// Takes in a service status: remembers that a tracker was attached, then refreshes the page.
pub(crate) fn on_service_update(controller: &Controller) {
    let attached = controller.status().is_some_and(|status| status.attached);
    let mut settings = controller.settings();
    if attached && !settings.gaze_tracker_has_been_connected {
        settings.gaze_tracker_has_been_connected = true;
        controller.set_settings(settings);
    }
    refresh(controller);
}

pub(crate) fn on_page_event(controller: &Controller, event: PageEvent) {
    if let PageEvent::CommandDone(_) = event {
        controller.page_model().pending_enabled.set(None);
        controller.page_model().pending_eyes.set(None);
    }
    match event {
        PageEvent::CommandDone(AfterCommand::Release) => release(controller),
        PageEvent::CommandDone(AfterCommand::ReleaseAfterDelay) => {
            let weak = controller.downgrade();
            glib::timeout_add_local_once(DISABLE_DELAY, move || {
                if let Some(controller) = weak.upgrade() {
                    release(&controller);
                }
            });
        }
        PageEvent::CommandDone(AfterCommand::ReleaseAndClose) => {
            release(controller);
            close_page(controller);
        }
        PageEvent::SetupEnded(exit) => {
            match setup_result(&exit) {
                SetupResult::Success => log::info!("setup ended ({exit:?})"),
                result => log::error!("setup ended with {result:?} ({exit:?})"),
            }
            release(controller);
            close_page(controller);
        }
    }
}

fn hold(controller: &Controller) {
    let busy = &controller.page_model().busy;
    busy.set(busy.get() + 1);
    refresh(controller);
}

fn release(controller: &Controller) {
    let busy = &controller.page_model().busy;
    busy.set(busy.get().saturating_sub(1));
    refresh(controller);
}

/// Sends `request` while holding the busy marker; `after` says how the marker ends.
fn run_command(controller: &Controller, request: Request, after: AfterCommand) {
    hold(controller);
    let events = controller.events();
    controller.send_then(request, move || {
        let _ = events.send_blocking(AppEvent::Page(PageEvent::CommandDone(after)));
    });
}

/// Tobii's `SetEyeTrackingEnabled`: turning off keeps the page disabled 500 ms longer.
fn set_enabled(controller: &Controller, on: bool) {
    let after = if on { AfterCommand::Release } else { AfterCommand::ReleaseAfterDelay };
    controller.page_model().pending_enabled.set(Some(on));
    run_command(controller, Request::SetEnabled(on), after);
}

fn set_tracked_eyes(controller: &Controller, eyes: TrackedEyes) {
    if tracked_eyes(controller) != eyes {
        controller.page_model().pending_eyes.set(Some(eyes));
        run_command(controller, Request::SetTrackedEyes(eyes), AfterCommand::Release);
    }
}

fn tracked_eyes(controller: &Controller) -> TrackedEyes {
    let value = controller.current_profile().and_then(|name| controller.profiles().tracked_eyes(&name).ok());
    match value {
        Some(1) => TrackedEyes::Left,
        Some(2) => TrackedEyes::Right,
        _ => TrackedEyes::Both,
    }
}

/// Runs `action` on the open page's profile list.
fn with_list<T>(controller: &Controller, action: impl FnOnce(&mut ProfileList) -> T) -> Option<T> {
    let mut list = controller.page_model().list.borrow_mut();
    list.as_mut().map(action)
}

fn commit_before(controller: &Controller, action: PageAction) {
    let store = controller.profiles();
    if let Some(Err(error)) = with_list(controller, |list| list.before(action, store)) {
        log::warn!("deleting marked profiles: {error}");
    }
}

/// A click on a listed profile: delete the marked ones, switch, then close the page.
fn switch_profile(controller: &Controller, name: &str) {
    let Some(Some(request)) = with_list(controller, |list| list.select(name)) else { return };
    commit_before(controller, PageAction::SwitchProfile);
    run_command(controller, request, AfterCommand::ReleaseAndClose);
}

fn mark_deleted(controller: &Controller, name: &str) {
    if with_list(controller, |list| list.mark_deleted(name)) == Some(true) {
        rows_changed(controller);
    }
}

fn undo_delete(controller: &Controller, name: &str) {
    with_list(controller, |list| list.undo(name));
    rows_changed(controller);
}

/// The page's buttons: run `tobii-linux-setup` with the flow's argument, keep the page
/// disabled until it exits, then close the page whatever the result was.
fn start_setup(controller: &Controller, flow: SetupFlow) {
    commit_before(controller, flow.action());
    hold(controller);
    let events = controller.events();
    let on_exit: OnExit = Box::new(move |exit| {
        let _ = events.send_blocking(AppEvent::Page(PageEvent::SetupEnded(exit)));
    });
    if let Err(error) = controller.setup().launch(&[flow.argument()], on_exit) {
        log::warn!("{flow:?}: {error}");
        release(controller);
        close_page(controller);
    }
}

/// Tobii's sidebar hiding: marked profiles are really deleted.
fn navigate_from(controller: &Controller) {
    commit_before(controller, PageAction::NavigateFrom);
    rows_changed(controller);
}

fn close_page(controller: &Controller) {
    if let Some(window) = controller.page_window() {
        window.close();
    }
}

/// A fresh list from the store, as on every page show.
fn rebuild_list(controller: &Controller) {
    let list = ProfileList::from_store(controller.profiles()).map_err(|error| log::warn!("profiles: {error}")).ok();
    controller.page_model().list.replace(list);
    rows_changed(controller);
}

fn rows_changed(controller: &Controller) {
    if let Some(view) = controller.page_model().view.borrow().as_ref() {
        view.rows_dirty.set(true);
    }
    refresh(controller);
}

/// Tobii's view-model disposal: the app quits with the page open (`gtk::Application::quit`
/// destroys the window without a close request), so marked profiles are deleted now.
pub(crate) fn on_shutdown(controller: &Controller) {
    if controller.page_model().view.borrow().is_some() {
        commit_before(controller, PageAction::NavigateFrom);
    }
}

/// Brings the open page up to date with the controller.
fn refresh(controller: &Controller) {
    let Some(view) = controller.page_model().view.borrow().clone() else { return };
    let stale = controller.page_model().list.borrow().as_ref().is_some_and(|list| list.is_stale(controller.profiles()));
    if stale {
        let list = ProfileList::from_store(controller.profiles()).ok();
        controller.page_model().list.replace(list);
        view.rows_dirty.set(true);
    }
    view.refresh(controller);
}

/// The window: 550 px wide, titled "Tobii Experience", holding the sidebar content under a
/// header row with the current profile's name (Tobii shows it on the ribbon's Settings
/// button). Closing it, or minimizing it where the backend reports that (X11; Wayland's
/// xdg-shell has no minimized state), is Tobii's sidebar hiding. Being merely hidden
/// (another workspace, occluded) is not: Tobii would not delete marked profiles then.
pub(crate) fn open_window(controller: &Controller) -> gtk::Window {
    let header = text("", "body");
    header.add_css_class("tobii-header");
    margins(&header, 45, 15, 25, 0);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&page(controller));
    if let Some(view) = controller.page_model().view.borrow().as_ref() {
        view.header.replace(Some(header));
    }
    let window: gtk::Window = gtk::ApplicationWindow::builder()
        .application(controller.application())
        .title(PAGE_TITLE)
        .default_width(PAGE_WIDTH)
        .default_height(PAGE_HEIGHT)
        .child(&content)
        .build()
        .upcast();
    window.add_css_class("tobii-page");
    let weak = controller.downgrade();
    window.connect_close_request(move |_| {
        if let Some(controller) = weak.upgrade() {
            navigate_from(&controller);
            controller.page_model().view.replace(None);
            controller.page_model().list.replace(None);
            controller.set_page_window(None);
        }
        glib::Propagation::Proceed
    });
    let weak = controller.downgrade();
    window.connect_realize(move |window| {
        let Some(toplevel) = window.surface().and_then(|surface| surface.downcast::<gdk::Toplevel>().ok()) else {
            return;
        };
        let (weak, minimized) = (weak.clone(), Cell::new(false));
        toplevel.connect_state_notify(move |toplevel| {
            let now = toplevel.state().contains(gdk::ToplevelState::MINIMIZED);
            let Some(controller) = weak.upgrade().filter(|_| minimized.replace(now) != now) else { return };
            if now { navigate_from(&controller) } else { rebuild_list(&controller) }
        });
    });
    controller.set_page_window(Some(window.clone()));
    refresh(controller);
    window
}

/// Builds the page's content, Tobii's sidebar, in its order: the on/off row, Improve my
/// calibration, Preview my gaze, Select eyes to detect, Change screen and Change user
/// profile with Create new profile and the list.
pub fn page(controller: &Controller) -> gtk::Widget {
    install_css();
    let view = Rc::new(PageView::build(controller));
    controller.page_model().view.replace(Some(view.clone()));
    rebuild_list(controller);
    view.root.clone().upcast()
}

fn install_css() {
    thread_local!(static INSTALLED: Cell<bool> = const { Cell::new(false) });
    if INSTALLED.replace(true) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&style::css());
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

/// WPF's `Thickness` order: left, top, right, bottom.
fn margins(widget: &impl IsA<gtk::Widget>, left: i32, top: i32, right: i32, bottom: i32) {
    widget.set_margin_start(left);
    widget.set_margin_top(top);
    widget.set_margin_end(right);
    widget.set_margin_bottom(bottom);
}

fn text(content: &str, class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(content)
        .css_classes([class])
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .build()
}

/// A text on one line, for the accent buttons' fixed columns.
fn single_line(content: &str, class: &str) -> gtk::Label {
    let label = text(content, class);
    label.set_wrap(false);
    label
}

/// A Body text with Tobii's margin `0 5 0 10`.
fn body(content: &str) -> gtk::Label {
    let label = text(content, "body");
    margins(&label, 0, 5, 0, 10);
    label
}

fn vertical(top: i32) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    section.set_margin_top(top);
    section
}

fn horizontal(children: &[&gtk::Widget]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    for child in children {
        row.append(*child);
    }
    row
}

/// `GazeEnabledButton`.
fn gaze_button(label: &str) -> gtk::Button {
    gtk::Button::builder().label(label).css_classes(["tobii-gaze-button"]).halign(gtk::Align::Start).build()
}

/// The accent-theme button around `content`; `icons` repaint with its foreground.
fn accent_button(content: &impl IsA<gtk::Widget>, icons: Vec<gtk::DrawingArea>) -> gtk::Button {
    let button = gtk::Button::builder().child(content).css_classes(["tobii-accent"]).build();
    button.connect_state_flags_changed(move |_, _| icons.iter().for_each(WidgetExt::queue_draw));
    button
}

/// An icon drawn from Tobii's path data in the foreground colour, scaled uniformly to
/// `width` (else `height`) like a WPF `Viewbox`.
fn icon(icon: Icon, width: Option<f64>, height: Option<f64>) -> gtk::DrawingArea {
    let scale = match (width, height) {
        (Some(width), _) => width / icon.width,
        (None, Some(height)) => height / icon.height,
        (None, None) => 1.0,
    };
    let paths: Vec<(Vec<Segment>, Paint)> = icon
        .paths
        .iter()
        .filter_map(|path| match style::parse_path(path.data) {
            Ok(segments) => Some((segments, path.paint)),
            Err(error) => {
                log::warn!("icon: {error}");
                None
            }
        })
        .collect();
    let area = gtk::DrawingArea::builder()
        .content_width((icon.width * scale).round() as i32)
        .content_height((icon.height * scale).round() as i32)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(move |area, cr, _, _| {
        let color = area.color();
        cr.set_source_rgba(color.red().into(), color.green().into(), color.blue().into(), color.alpha().into());
        cr.scale(scale, scale);
        for (segments, paint) in &paths {
            cr.new_path();
            for segment in segments {
                match *segment {
                    Segment::Move(x, y) => cr.move_to(x, y),
                    Segment::Line(x, y) => cr.line_to(x, y),
                    Segment::Cubic([x1, y1, x2, y2, x, y]) => cr.curve_to(x1, y1, x2, y2, x, y),
                    Segment::Close => cr.close_path(),
                }
            }
            let painted = match paint {
                Paint::Fill => {
                    cr.set_fill_rule(cairo::FillRule::Winding);
                    cr.fill()
                }
                Paint::Stroke(thickness) => {
                    cr.set_line_width(*thickness);
                    cr.set_line_join(cairo::LineJoin::Round);
                    cr.stroke()
                }
            };
            if let Err(error) = painted {
                log::warn!("icon: {error}");
            }
        }
    });
    area
}

/// `GazeEnabledToggleStyle`: a 42 x 19 track with its knob, then the ON or OFF label.
struct Toggle {
    button: gtk::ToggleButton,
}

impl Toggle {
    fn new() -> Toggle {
        let knob =
            gtk::Box::builder().css_classes(["knob"]).halign(gtk::Align::Start).valign(gtk::Align::Center).build();
        let track = gtk::Box::builder().css_classes(["track"]).valign(gtk::Align::Center).build();
        track.append(&knob);
        let state = gtk::Label::builder().label(OFF_TEXT).css_classes(["state"]).xalign(0.0).build();
        let content = horizontal(&[track.upcast_ref(), state.upcast_ref()]);
        let button = gtk::ToggleButton::builder().child(&content).css_classes(["tobii-toggle"]).build();
        button.set_valign(gtk::Align::Center);
        button.connect_toggled(move |button| state.set_label(if button.is_active() { ON_TEXT } else { OFF_TEXT }));
        Toggle { button }
    }
}

/// The page's widgets, refreshed from the controller.
pub(crate) struct PageView {
    root: gtk::Box,
    header: RefCell<Option<gtk::Label>>,
    switch: Toggle,
    profiles_section: gtk::Box,
    calibration: gtk::Box,
    improve: gtk::Button,
    gaze_trace: gtk::Box,
    preview: Toggle,
    tracked_eyes: gtk::Box,
    eyes_button: gtk::Button,
    down_arrow: gtk::DrawingArea,
    right_arrow: gtk::DrawingArea,
    eyes_block: gtk::Box,
    radios: [(TrackedEyes, gtk::CheckButton); 3],
    display_setup: gtk::Box,
    set_up_display: gtk::Button,
    user_profile: gtk::Box,
    create: gtk::Button,
    list: gtk::Box,
    rows_dirty: Cell<bool>,
    /// Set while the view writes values, so that its own writes send no requests.
    updating: Cell<bool>,
}

impl PageView {
    fn build(controller: &Controller) -> PageView {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        margins(&content, 0, 0, 10, 0);
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
        margins(&inner, 5, 0, 0, 0);
        inner.append(&content);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .overlay_scrolling(false)
            .vexpand(true)
            .child(&inner)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        margins(&root, 40, 0, 0, 15);
        root.set_vexpand(true);
        root.append(&scrolled);

        let switch = Toggle::new();
        margins(&switch.button, 30, 0, 0, 0);
        let title = text(SWITCH_TITLE, "h3");
        title.set_valign(gtk::Align::Center);
        title.set_margin_bottom(3);
        let switch_section = vertical(25);
        switch_section.set_margin_bottom(20);
        switch_section.append(&horizontal(&[title.upcast_ref(), switch.button.upcast_ref()]));
        content.append(&switch_section);

        let profiles_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&profiles_section);

        let calibration = vertical(15);
        let improve = gaze_button(IMPROVE_BUTTON);
        calibration.append(&text(IMPROVE_TITLE, "h3"));
        calibration.append(&body(IMPROVE_BODY));
        calibration.append(&improve);
        profiles_section.append(&calibration);

        let gaze_trace = vertical(15);
        let setting = gtk::Box::new(gtk::Orientation::Vertical, 0);
        setting.set_margin_bottom(20);
        let preview = Toggle::new();
        margins(&preview.button, 30, 0, 10, 0);
        let preview_title = text(PREVIEW_TITLE, "h3");
        preview_title.set_valign(gtk::Align::Center);
        preview_title.set_margin_bottom(4);
        let title_row = horizontal(&[preview_title.upcast_ref(), preview.button.upcast_ref()]);
        margins(&title_row, 0, 20, 0, 10);
        setting.append(&title_row);
        setting.append(&body(PREVIEW_BODY));
        gaze_trace.append(&setting);
        profiles_section.append(&gaze_trace);

        let tracked_eyes = vertical(10);
        let down_arrow = icon(style::DOWN_ARROW, Some(18.0), None);
        let right_arrow = icon(style::RIGHT_ARROW, None, Some(18.0));
        let arrows = horizontal(&[down_arrow.upcast_ref(), right_arrow.upcast_ref()]);
        arrows.set_valign(gtk::Align::Center);
        margins(&arrows, 0, 0, 10, 0);
        let eyes_label = text(EYES_TITLE, "h3");
        let eyes_button = accent_button(
            &horizontal(&[arrows.upcast_ref(), eyes_label.upcast_ref()]),
            vec![down_arrow.clone(), right_arrow.clone()],
        );
        eyes_button.set_halign(gtk::Align::Start);
        tracked_eyes.append(&eyes_button);
        let eyes_block = gtk::Box::new(gtk::Orientation::Vertical, 0);
        eyes_block.append(&body(EYES_BODY));
        let both = gtk::CheckButton::builder().label(BOTH_EYES).css_classes(["tobii-radio"]).build();
        let left = gtk::CheckButton::builder().label(LEFT_EYE).css_classes(["tobii-radio"]).group(&both).build();
        let right = gtk::CheckButton::builder().label(RIGHT_EYE).css_classes(["tobii-radio"]).group(&both).build();
        margins(&both, 0, 0, 15, 0);
        margins(&left, 0, 0, 15, 0);
        eyes_block.append(&horizontal(&[both.upcast_ref(), left.upcast_ref(), right.upcast_ref()]));
        tracked_eyes.append(&eyes_block);
        profiles_section.append(&tracked_eyes);

        let display_setup = vertical(20);
        let set_up_display = gaze_button(SCREEN_BUTTON);
        display_setup.append(&text(SCREEN_TITLE, "h3"));
        display_setup.append(&body(SCREEN_BODY));
        display_setup.append(&set_up_display);
        profiles_section.append(&display_setup);

        let user_profile = vertical(20);
        user_profile.append(&text(PROFILE_TITLE, "h3"));
        user_profile.append(&body(PROFILE_BODY));
        let new_profile_icon = icon(style::NEW_PROFILE, Some(35.0), None);
        let create_label = single_line(NEW_PROFILE_BUTTON, "body");
        create_label.set_valign(gtk::Align::Center);
        create_label.set_margin_start(5);
        let create = accent_button(
            &horizontal(&[new_profile_icon.upcast_ref(), create_label.upcast_ref()]),
            vec![new_profile_icon.clone()],
        );
        create.set_size_request(200, -1);
        create.set_halign(gtk::Align::Start);
        create.set_margin_top(20);
        user_profile.append(&create);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        user_profile.append(&list);
        profiles_section.append(&user_profile);

        let view = PageView {
            root,
            header: RefCell::new(None),
            switch,
            profiles_section,
            calibration,
            improve,
            gaze_trace,
            preview,
            tracked_eyes,
            eyes_button,
            down_arrow,
            right_arrow,
            eyes_block,
            radios: [(TrackedEyes::Both, both), (TrackedEyes::Left, left), (TrackedEyes::Right, right)],
            display_setup,
            set_up_display,
            user_profile,
            create,
            list,
            rows_dirty: Cell::new(true),
            updating: Cell::new(false),
        };
        view.connect(controller);
        view
    }

    fn connect(&self, controller: &Controller) {
        let on = |action: fn(&Controller)| {
            let weak = controller.downgrade();
            move || {
                if let Some(controller) = weak.upgrade() {
                    action(&controller);
                }
            }
        };
        let weak = controller.downgrade();
        self.switch.button.connect_toggled(move |button| {
            let Some(controller) = weak.upgrade().filter(|controller| !view_updating(controller)) else { return };
            if controller.availability().enable_disable {
                set_enabled(&controller, button.is_active());
            } else {
                refresh(&controller);
            }
        });
        let weak = controller.downgrade();
        self.preview.button.connect_toggled(move |button| {
            if let Some(controller) = weak.upgrade().filter(|controller| !view_updating(controller)) {
                controller.set_preview_requested(button.is_active());
            }
        });
        let improve = on(|controller| start_setup(controller, SetupFlow::Calibrate));
        self.improve.connect_clicked(move |_| improve());
        let display = on(|controller| start_setup(controller, SetupFlow::display(controller.page_state())));
        self.set_up_display.connect_clicked(move |_| display());
        let create = on(|controller| start_setup(controller, SetupFlow::CreateNewProfile));
        self.create.connect_clicked(move |_| create());
        let fold = on(|controller| {
            let open = &controller.page_model().eyes_open;
            open.set(!open.get());
            refresh(controller);
        });
        self.eyes_button.connect_clicked(move |_| fold());
        for (eyes, radio) in &self.radios {
            let (eyes, weak) = (*eyes, controller.downgrade());
            radio.connect_toggled(move |radio| {
                let Some(controller) = weak.upgrade() else { return };
                if radio.is_active() && !view_updating(&controller) {
                    set_tracked_eyes(&controller, eyes);
                }
            });
        }
    }

    fn refresh(&self, controller: &Controller) {
        let model = controller.page_model();
        let current = controller.current_profile();
        let capability = controller.gaze_capability();
        let shown = visibility(capability);
        let flags = availability(controller.page_state(), capability, current.as_deref(), controller.system_enabled());
        self.updating.set(true);
        self.root.set_sensitive(!controller.page_busy());
        if let Some(header) = self.header.borrow().as_ref() {
            header.set_label(current.as_deref().unwrap_or_default());
            header.set_visible(shown.profiles_section);
        }
        self.switch.button.set_active(model.pending_enabled.get().unwrap_or_else(|| controller.system_enabled()));
        self.switch.button.set_sensitive(flags.enable_disable);
        self.profiles_section.set_visible(shown.profiles_section);
        self.calibration.set_visible(shown.calibration);
        self.improve.set_sensitive(flags.improve_calibration);
        self.gaze_trace.set_visible(shown.gaze_trace);
        self.preview.button.set_active(controller.preview_requested());
        self.preview.button.set_sensitive(flags.preview);
        self.tracked_eyes.set_visible(shown.tracked_eyes);
        self.eyes_button.set_sensitive(flags.select_eyes);
        let open = model.eyes_open.get();
        self.down_arrow.set_visible(open);
        self.right_arrow.set_visible(!open);
        self.eyes_block.set_visible(open);
        let eyes = model.pending_eyes.get().unwrap_or_else(|| tracked_eyes(controller));
        for (value, radio) in &self.radios {
            radio.set_active(*value == eyes);
        }
        self.display_setup.set_visible(shown.display_setup);
        self.set_up_display.set_sensitive(flags.setup_display);
        self.user_profile.set_visible(shown.user_profile);
        self.create.set_sensitive(flags.create_new_profile);
        self.list.set_sensitive(flags.change_profile);
        if self.rows_dirty.replace(false) {
            self.rebuild_rows(controller);
        }
        self.updating.set(false);
    }

    fn rebuild_rows(&self, controller: &Controller) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let Some(list) = controller.profile_list() else { return };
        for item in list.items() {
            self.list.append(&profile_row(
                controller,
                &item.name,
                item.is_enabled(),
                item.can_undo_delete(),
                item.can_delete(),
            ));
        }
    }
}

fn view_updating(controller: &Controller) -> bool {
    controller.page_model().view.borrow().as_ref().is_some_and(|view| view.updating.get())
}

/// One list row, `CalibrationProfileItemStyle`: the profile button in a 200 px column, then
/// Undo or the trash.
fn profile_row(controller: &Controller, name: &str, enabled: bool, undo: bool, delete: bool) -> gtk::Grid {
    let row = gtk::Grid::builder().css_classes(["tobii-profile-row"]).build();
    let on_click = |action: fn(&Controller, &str)| {
        let (weak, name): (WeakController, String) = (controller.downgrade(), name.to_string());
        move |_: &gtk::Button| {
            if let Some(controller) = weak.upgrade() {
                action(&controller, &name);
            }
        }
    };
    let profile_icon = icon(style::PROFILE, Some(30.0), None);
    let label = single_line(name, "body");
    label.set_valign(gtk::Align::Center);
    label.set_margin_start(5);
    let profile =
        accent_button(&horizontal(&[profile_icon.upcast_ref(), label.upcast_ref()]), vec![profile_icon.clone()]);
    profile.set_size_request(200, -1);
    profile.set_valign(gtk::Align::Center);
    profile.set_sensitive(enabled);
    profile.connect_clicked(on_click(switch_profile));
    row.attach(&profile, 0, 0, 1, 1);
    if undo {
        let undo = accent_button(&single_line(UNDO_TEXT, "body"), Vec::new());
        undo.add_css_class("tobii-undo");
        undo.set_halign(gtk::Align::Start);
        undo.set_valign(gtk::Align::Center);
        undo.connect_clicked(on_click(undo_delete));
        row.attach(&undo, 1, 0, 1, 1);
    }
    if delete {
        let trash_icon = icon(style::TRASH, Some(30.0), None);
        let trash = accent_button(&trash_icon, vec![trash_icon.clone()]);
        trash.add_css_class("tobii-trash");
        trash.set_halign(gtk::Align::Start);
        trash.set_valign(gtk::Align::Center);
        trash.connect_clicked(on_click(mark_deleted));
        row.attach(&trash, 1, 0, 1, 1);
    }
    row
}
