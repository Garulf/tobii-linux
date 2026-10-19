//! The "Preview my gaze" bubble: a click-through layer-shell surface over the tracked monitor
//! drawing `GazeNative.dll`'s passes (docs/gaze-preview.md) in a `gtk::GLArea`.
//!
//! Every 12 ms while shown the trail ([`Trail`]) advances one `GazeNative_Update` tick and
//! queues one trail pass. Ticks follow a 12 ms wall-clock schedule: when the main loop was
//! busy, the missed ticks run at once, each at its scheduled time, because the trail decays
//! per tick (docs/gaze-preview.md, "Notes for the GL port"). A render runs the queued passes into two quarter-resolution float
//! targets (ping-pong), clears outside the bounding box and composites the latest one onto
//! the surface, premultiplied, as GTK 4 takes a GLArea's buffer (always with alpha). Hidden, the timer
//! stops and the targets are cleared before the next show.
//!
//! Layer-shell needs a Wayland compositor with `wlr-layer-shell`; elsewhere [`Overlay::new`]
//! logs that and returns `None`, and the preview stays off. [`Overlay::windowed`] puts the
//! same renderer in a plain undecorated window, for headless runs and tests.

mod gl;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Once;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tobii_control::GazeSample;

use crate::app::{Controller, WeakController};
use crate::link::LinkUpdate;
use crate::page::PageState;
use crate::preview::{OutputInfo, Preview, PreviewInputs, RecordedOutput, choose_output};
use crate::trail::{self, GRADIENT_WIDTH, Trail};
use gl::{Destination, Draw, FloatFormat, Gl, Program, Target, Texture, UniformBuffer};

/// `GazeTraceVisualization`'s update period.
pub const TICK: Duration = Duration::from_millis(12);
/// Trail passes kept for one render; older ones are dropped when rendering stalls.
const MAX_STEPS: usize = 64;
const TITLE: &str = "Tobii Streaming Gaze Overlay";
const NAMESPACE: &str = "tobii-gaze-preview";
const CSS_CLASS: &str = "tobii-gaze-preview";
const CSS: &str = "window.tobii-gaze-preview { background: none; box-shadow: none; }";
const UNIFORMS_SIZE: usize = 11 * 16;

const VERTEX: &str = include_str!("shaders/quad.vert.glsl");
const TRAIL: &str = include_str!("shaders/trail.frag.glsl");
const COMPOSITE: &str = include_str!("shaders/composite.frag.glsl");

/// One update tick's constant buffer and bounding box (output pixels).
#[derive(Debug, Clone, Copy)]
struct Step {
    block: [[f32; 4]; 11],
    bounds: [i32; 4],
}

impl Step {
    fn bytes(&self) -> Vec<u8> {
        self.block.iter().flatten().flat_map(|value| value.to_ne_bytes()).collect()
    }
}

#[derive(Debug)]
struct Targets {
    current: Target,
    previous: Target,
    size: [i32; 2],
}

/// The GL objects, made on realize and dropped (deleted) on unrealize.
#[derive(Debug)]
struct Renderer {
    gl: Gl,
    trail: Program,
    composite: Program,
    params: UniformBuffer,
    gradient: Texture,
    formats: Vec<FloatFormat>,
    format: Option<FloatFormat>,
    targets: Option<Targets>,
}

impl Renderer {
    fn new(context: &gdk::GLContext) -> Result<Renderer, String> {
        let gl = Gl::load(context)?;
        let formats = gl.float_formats();
        if formats.is_empty() {
            return Err(format!("{}: no float colour buffer (EXT_color_buffer_float/half_float)", gl.description()));
        }
        let gradient: Vec<u8> = trail::gradient().into_iter().flatten().collect();
        Ok(Renderer {
            trail: gl.program(VERTEX, TRAIL)?,
            composite: gl.program(VERTEX, COMPOSITE)?,
            params: gl.uniform_buffer(UNIFORMS_SIZE)?,
            gradient: gl.rgba_texture(GRADIENT_WIDTH as i32, 1, &gradient)?,
            gl,
            formats,
            format: None,
            targets: None,
        })
    }

    /// The quarter targets for a `size` surface, cleared when (re)made.
    fn ensure_targets(&mut self, size: [i32; 2]) -> Result<&mut Targets, String> {
        if self.targets.as_ref().is_none_or(|targets| targets.size != size) {
            self.targets = None;
            let [width, height] = trail::scissors([0; 4], size, 1.0).quarter_size;
            let mut errors = Vec::new();
            for &format in &self.formats {
                let pair = self.gl.float_target(width, height, format).and_then(|current| {
                    Ok(Targets { current, previous: self.gl.float_target(width, height, format)?, size })
                });
                match pair {
                    Ok(targets) => {
                        if self.format != Some(format) {
                            log::info!("gaze preview: {}, accumulation {format:?}", self.gl.description());
                        }
                        self.format = Some(format);
                        self.targets = Some(targets);
                        break;
                    }
                    Err(error) => errors.push(error),
                }
            }
            if self.targets.is_none() {
                return Err(errors.join("; "));
            }
        }
        Ok(self.targets.as_mut().expect("made above"))
    }

    fn clear_targets(&self) {
        if let Some(targets) = &self.targets {
            self.gl.clear(Destination::Target(&targets.current), None);
            self.gl.clear(Destination::Target(&targets.previous), None);
        }
    }

    /// The trail pass into the current target, the clear outside the box, then the swap.
    fn step(&mut self, step: &Step, scale: f64) {
        let Some(targets) = self.targets.as_mut() else { return };
        let plan = trail::scissors(step.bounds, targets.size, scale);
        let [width, height] = plan.quarter_size;
        self.params.write(&step.bytes());
        self.gl.draw(Draw {
            program: &self.trail,
            destination: Destination::Target(&targets.current),
            viewport: [0, 0, width, height],
            scissor: plan.quarter_box,
            uniforms: &self.params,
            textures: &[("previous", targets.previous.texture())],
        });
        for rect in plan.outside {
            self.gl.clear(Destination::Target(&targets.current), Some(rect));
        }
        std::mem::swap(&mut targets.current, &mut targets.previous);
    }

    /// Clears the bound framebuffer and composites the latest trail inside its box.
    fn composite(&self, step: Option<&Step>, scale: f64) {
        self.gl.clear(Destination::Bound, None);
        if let (Some(step), Some(targets)) = (step, &self.targets) {
            let plan = trail::scissors(step.bounds, targets.size, scale);
            let [width, height] = targets.size;
            self.params.write(&step.bytes());
            self.gl.draw(Draw {
                program: &self.composite,
                destination: Destination::Bound,
                viewport: [0, 0, width, height],
                scissor: plan.full_box,
                uniforms: &self.params,
                textures: &[("accumulation", targets.previous.texture()), ("gradient", &self.gradient)],
            });
        }
        self.gl.finish();
    }
}

struct State {
    window: gtk::Window,
    area: gtk::GLArea,
    layer_shell: bool,
    trail: RefCell<Trail>,
    steps: RefCell<VecDeque<Step>>,
    latest: Cell<Option<Step>>,
    renderer: RefCell<Option<Renderer>>,
    /// The buffer size from the GLArea's `resize`, device pixels.
    size: Cell<[i32; 2]>,
    timer: RefCell<Option<glib::SourceId>>,
    next_tick: Cell<Instant>,
    output: RefCell<Option<gdk::Monitor>>,
    clear: Cell<bool>,
    description: RefCell<Option<String>>,
}

impl Drop for State {
    /// Releases the GL objects, then the window, which GTK would otherwise keep.
    fn drop(&mut self) {
        self.renderer.take();
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
        self.window.destroy();
    }
}

/// The bubble's window and renderer.
#[derive(Clone)]
pub struct Overlay(Rc<State>);

impl fmt::Debug for Overlay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = &self.0;
        f.debug_struct("Overlay")
            .field("layer_shell", &state.layer_shell)
            .field("shown", &self.is_shown())
            .field("output", &state.output.borrow().as_ref().and_then(|monitor| monitor.connector()))
            .field("renderer", &state.description.borrow())
            .finish_non_exhaustive()
    }
}

impl Overlay {
    /// The layer-shell overlay; `None` (logged) where the compositor offers no layer-shell.
    pub fn new() -> Option<Overlay> {
        if !gtk4_layer_shell::is_supported() {
            static LOGGED: Once = Once::new();
            LOGGED.call_once(|| {
                log::info!(
                    "gaze preview: the display offers no layer-shell (Wayland wlr-layer-shell), so it stays off"
                );
            });
            return None;
        }
        Some(Overlay::build(true))
    }

    /// The same renderer in a plain undecorated, click-through window, for headless runs and
    /// tests where layer-shell is unavailable.
    pub fn windowed() -> Overlay {
        Overlay::build(false)
    }

    fn build(layer_shell: bool) -> Overlay {
        let area = gtk::GLArea::new();
        area.set_auto_render(false);
        let window = gtk::Window::builder().title(TITLE).decorated(false).focusable(false).child(&area).build();
        window.add_css_class(CSS_CLASS);
        if let Some(display) = gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_string(CSS);
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        if layer_shell {
            window.init_layer_shell();
            window.set_layer(Layer::Overlay);
            for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                window.set_anchor(edge, true);
            }
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
            window.set_namespace(Some(NAMESPACE));
        }
        window.connect_realize(click_through);
        window.connect_map(click_through);
        let state = Rc::new(State {
            window,
            area,
            layer_shell,
            trail: RefCell::new(Trail::default()),
            steps: RefCell::new(VecDeque::new()),
            latest: Cell::new(None),
            renderer: RefCell::new(None),
            size: Cell::new([0, 0]),
            timer: RefCell::new(None),
            next_tick: Cell::new(Instant::now()),
            output: RefCell::new(None),
            clear: Cell::new(true),
            description: RefCell::new(None),
        });
        connect_area(&state);
        Overlay(state)
    }

    /// Shows the bubble over `output`, or hides it for `None`.
    pub fn show_on(&self, output: Option<&gdk::Monitor>) {
        let Some(monitor) = output else {
            self.hide();
            return;
        };
        let state = &self.0;
        if state.output.borrow().as_ref() != Some(monitor) {
            let geometry = monitor.geometry();
            state.trail.borrow_mut().set_bounds([geometry.x(), geometry.y(), geometry.width(), geometry.height()]);
            if state.layer_shell {
                state.window.set_monitor(Some(monitor));
            } else {
                state.window.set_default_size(geometry.width(), geometry.height());
            }
            state.output.replace(Some(monitor.clone()));
        }
        if state.timer.borrow().is_some() {
            return;
        }
        state.trail.borrow_mut().show();
        state.clear.set(true);
        state.steps.borrow_mut().clear();
        state.latest.set(None);
        state.next_tick.set(Instant::now() + TICK);
        let weak = Rc::downgrade(state);
        let timer = glib::timeout_add_local(TICK, move || match weak.upgrade() {
            Some(state) => {
                run_due_ticks(&state);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        state.timer.replace(Some(timer));
        state.window.set_visible(true);
    }

    fn hide(&self) {
        let state = &self.0;
        if let Some(timer) = state.timer.take() {
            timer.remove();
        }
        state.trail.borrow_mut().hide();
        state.steps.borrow_mut().clear();
        state.latest.set(None);
        state.clear.set(true);
        state.output.replace(None);
        state.window.set_visible(false);
    }

    /// A gaze sample in normalized output coordinates; invalid ones are dropped.
    pub fn add(&self, sample: GazeSample) {
        self.0.trail.borrow_mut().add(sample, Instant::now());
    }

    pub fn is_shown(&self) -> bool {
        self.0.timer.borrow().is_some()
    }

    pub fn window(&self) -> &gtk::Window {
        &self.0.window
    }

    pub fn area(&self) -> &gtk::GLArea {
        &self.0.area
    }

    /// The GL version, renderer and accumulation format in use, once a frame was drawn.
    pub fn renderer_description(&self) -> Option<String> {
        self.0.description.borrow().clone()
    }
}

/// An empty input region: clicks go to the windows below.
fn click_through(window: &gtk::Window) {
    if let Some(surface) = window.surface() {
        surface.set_input_region(Some(&gtk::cairo::Region::create()));
    }
}

/// Runs every tick scheduled up to now, at most [`MAX_STEPS`]; further behind, the schedule
/// restarts from now.
fn run_due_ticks(state: &State) {
    let now = Instant::now();
    let mut due = state.next_tick.get();
    let mut ran = 0;
    while due <= now && ran < MAX_STEPS {
        tick(state, due);
        due += TICK;
        ran += 1;
    }
    state.next_tick.set(if due <= now { now + TICK } else { due });
}

fn tick(state: &State, at: Instant) {
    let step = {
        let mut trail = state.trail.borrow_mut();
        trail.update(at);
        let [_, _, width, height] = trail.bounds();
        let block = trail::uniforms(&trail.params, trail.frame(), width as f32, height as f32);
        Step { block, bounds: trail.bounding_box(width, height) }
    };
    let mut steps = state.steps.borrow_mut();
    if steps.len() == MAX_STEPS {
        steps.pop_front();
    }
    steps.push_back(step);
    state.area.queue_render();
}

fn connect_area(state: &Rc<State>) {
    let weak = Rc::downgrade(state);
    state.area.connect_realize(move |area| {
        let Some(state) = weak.upgrade() else { return };
        area.make_current();
        if let Some(error) = area.error() {
            log::warn!("gaze preview: no GL context: {error}");
            return;
        }
        let Some(context) = area.context() else { return };
        match Renderer::new(&context) {
            Ok(renderer) => {
                state.renderer.replace(Some(renderer));
            }
            Err(error) => log::warn!("gaze preview: {error}"),
        }
    });
    let weak = Rc::downgrade(state);
    state.area.connect_unrealize(move |area| {
        let Some(state) = weak.upgrade() else { return };
        area.make_current();
        state.renderer.replace(None);
    });
    let weak = Rc::downgrade(state);
    state.area.connect_resize(move |_, width, height| {
        if let Some(state) = weak.upgrade() {
            state.size.set([width, height]);
        }
    });
    let weak: Weak<State> = Rc::downgrade(state);
    state.area.connect_render(move |area, _| {
        if let Some(state) = weak.upgrade() {
            render(&state, area);
        }
        glib::Propagation::Stop
    });
}

fn render(state: &State, area: &gtk::GLArea) {
    let mut slot = state.renderer.borrow_mut();
    let Some(renderer) = slot.as_mut() else { return };
    let size = state.size.get();
    let scale = if area.width() > 0 { f64::from(size[0]) / f64::from(area.width()) } else { 1.0 };
    if let Err(error) = renderer.ensure_targets(size) {
        log::warn!("gaze preview: no float render target: {error}");
        *slot = None;
        return;
    }
    if state.description.borrow().is_none() {
        let format = renderer.format.map_or_else(String::new, |format| format!(", accumulation {format:?}"));
        state.description.replace(Some(format!("{}{format}", renderer.gl.description())));
    }
    if state.clear.replace(false) {
        renderer.clear_targets();
    }
    for step in state.steps.borrow_mut().drain(..) {
        renderer.step(&step, scale);
        state.latest.set(Some(step));
    }
    area.attach_buffers();
    renderer.composite(state.latest.get().as_ref(), scale);
}

/// The overlay and the visibility rules around it, owned by the controller's hooks.
struct Wiring {
    overlay: Overlay,
    preview: RefCell<Preview>,
    record: PathBuf,
}

impl Wiring {
    /// Re-reads every input; sends `SubscribeGaze` when the bubble shows or hides.
    fn refresh(&self, controller: &Controller) {
        let window = controller.page_window();
        let toplevel = window.as_ref().and_then(|window| window.surface()).and_downcast::<gdk::Toplevel>();
        let monitors = monitors();
        let outputs: Vec<OutputInfo> = monitors.iter().map(output_info).collect();
        let recorded = RecordedOutput::read(&self.record).unwrap_or_default();
        let index = choose_output(&recorded, &outputs);
        let inputs = PreviewInputs {
            requested: controller.preview_requested(),
            window_active: window.as_ref().is_some_and(|window| window.is_active()),
            window_minimized: toplevel.is_some_and(|toplevel| toplevel.state().contains(gdk::ToplevelState::MINIMIZED)),
            tracking: controller.page_state() == PageState::Tracking,
            output_found: index.is_some(),
        };
        let mut preview = self.preview.borrow_mut();
        if let Some(request) = preview.update(inputs) {
            controller.send(request);
        }
        let output = index.filter(|_| preview.shown()).map(|index| &monitors[index]);
        drop(preview);
        self.overlay.show_on(output);
    }
}

fn monitors() -> Vec<gdk::Monitor> {
    let Some(display) = gdk::Display::default() else { return Vec::new() };
    let model = display.monitors();
    (0..model.n_items()).filter_map(|index| model.item(index).and_downcast::<gdk::Monitor>()).collect()
}

fn output_info(monitor: &gdk::Monitor) -> OutputInfo {
    OutputInfo {
        connector: monitor.connector().map(Into::into),
        manufacturer: monitor.manufacturer().map(Into::into),
        model: monitor.model().map(Into::into),
    }
}

/// Refreshes on every change of a window's activity or minimized state.
fn watch_window(window: &gtk::Window, wiring: &Rc<Wiring>, controller: &WeakController) {
    let (hook_wiring, hook_controller) = (wiring.clone(), controller.clone());
    window.connect_is_active_notify(move |_| {
        if let Some(controller) = hook_controller.upgrade() {
            hook_wiring.refresh(&controller);
        }
    });
    let (wiring, controller) = (wiring.clone(), controller.clone());
    window.connect_realize(move |window| {
        let Some(toplevel) = window.surface().and_downcast::<gdk::Toplevel>() else { return };
        let (wiring, controller) = (wiring.clone(), controller.clone());
        toplevel.connect_state_notify(move |_| {
            if let Some(controller) = controller.upgrade() {
                wiring.refresh(&controller);
            }
        });
    });
}

/// Connects the preview to `controller`: the toggle, the page window, the device state, the
/// monitors and the gaze stream. Does nothing where layer-shell is unavailable.
pub(crate) fn attach(controller: &Controller) {
    let Some(overlay) = Overlay::new() else { return };
    let (data_home, home) = (std::env::var("XDG_DATA_HOME").ok(), std::env::var("HOME").ok());
    let record = tobii_gaze::profiles::default_display_record(data_home.as_deref(), home.as_deref());
    let wiring = Rc::new(Wiring { overlay, preview: RefCell::new(Preview::default()), record });
    let hook = wiring.clone();
    controller.connect_preview(move |controller, _| hook.refresh(controller));
    let hook = wiring.clone();
    controller.connect_update(move |controller, update| match update {
        LinkUpdate::Gaze(sample) => hook.overlay.add(*sample),
        LinkUpdate::Status(_) => hook.refresh(controller),
        LinkUpdate::Failed(_) => {}
    });
    let weak = controller.downgrade();
    let (hook, hook_weak) = (wiring.clone(), weak.clone());
    controller.application().connect_window_added(move |_, window| {
        watch_window(window, &hook, &hook_weak);
        if let Some(controller) = hook_weak.upgrade() {
            hook.refresh(&controller);
        }
    });
    let (hook, hook_weak) = (wiring.clone(), weak.clone());
    controller.application().connect_window_removed(move |_, _| {
        if let Some(controller) = hook_weak.upgrade() {
            hook.refresh(&controller);
        }
    });
    if let Some(display) = gdk::Display::default() {
        let (hook, hook_weak) = (wiring.clone(), weak);
        display.monitors().connect_items_changed(move |_, _, _, _| {
            if let Some(controller) = hook_weak.upgrade() {
                hook.refresh(&controller);
            }
        });
    }
    wiring.refresh(controller);
}
