//! The gaze preview's renderer in a real GTK window against simulated gaze. GTK needs a
//! display: `make experience-test` runs this under Xvfb; without one it is skipped. Under X11
//! there is no layer-shell, so the renderer runs in [`Overlay::windowed`]. With
//! `TOBII_EXPERIENCE_BUBBLE_FRAMES=<directory>` it writes the frames it checks as PNGs, over a
//! dark backdrop, for comparing with Tobii's bubble on Windows.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib, graphene};
use gtk4 as gtk;
use tobii_control::GazeSample;
use tobii_experience::overlay::Overlay;

const REQUIRE_DISPLAY: &str = "TOBII_EXPERIENCE_REQUIRE_DISPLAY";
const FRAMES: &str = "TOBII_EXPERIENCE_BUBBLE_FRAMES";
const SIZE: f64 = 0.075;

/// Runs the main loop for `duration`.
fn pump(duration: Duration) {
    let context = glib::MainContext::default();
    let end = Instant::now() + duration;
    while Instant::now() < end {
        context.iteration(true);
    }
}

/// Renders the window through its own renderer, optionally over a backdrop colour. The
/// GLArea first runs every trail pass queued so far: software rendering draws the window
/// less often than the trail ticks.
fn render(overlay: &Overlay, backdrop: Option<gdk::RGBA>) -> Result<gdk::Texture, String> {
    let window = overlay.window();
    let clock = window.frame_clock().ok_or("the window has no frame clock")?;
    let painted = Rc::new(Cell::new(false));
    let handler = clock.connect_after_paint({
        let painted = painted.clone();
        move |_| painted.set(true)
    });
    overlay.area().queue_render();
    let (context, deadline) = (glib::MainContext::default(), Instant::now() + Duration::from_secs(5));
    while !painted.get() && Instant::now() < deadline {
        context.iteration(true);
    }
    clock.disconnect(handler);
    if !painted.get() {
        return Err("the window was not painted within 5 s".into());
    }
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    let (width, height) = (window.width() as f32, window.height() as f32);
    let bounds = graphene::Rect::new(0.0, 0.0, width, height);
    if let Some(color) = backdrop {
        snapshot.append_color(&color, &bounds);
    }
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot.to_node().ok_or("the window drew nothing")?;
    let renderer = window.renderer().ok_or("the window has no renderer")?;
    Ok(renderer.render_texture(&node, Some(&bounds)))
}

/// Premultiplied alpha per pixel, row by row.
fn alpha(texture: &gdk::Texture) -> (Vec<u8>, usize) {
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let mut bytes = vec![0; width * height * 4];
    texture.download(&mut bytes, width * 4);
    let (pixels, _) = bytes.as_chunks::<4>();
    let alpha = pixels.iter().map(|pixel| pixel[if cfg!(target_endian = "little") { 3 } else { 0 }]).collect();
    (alpha, width)
}

/// The alpha-weighted mean distance of drawn pixels from `centre`, the largest distance of
/// any drawn pixel and the alpha at the centre.
fn ring(alpha: &[u8], width: usize, centre: (f64, f64)) -> (f64, f64, u8) {
    let (mut weight, mut sum, mut farthest) = (0.0, 0.0, 0.0f64);
    for (index, &value) in alpha.iter().enumerate().filter(|(_, value)| **value > 0) {
        let (x, y) = ((index % width) as f64 + 0.5, (index / width) as f64 + 0.5);
        let distance = (x - centre.0).hypot(y - centre.1);
        weight += f64::from(value);
        sum += f64::from(value) * distance;
        farthest = farthest.max(distance);
    }
    let at_centre = alpha[centre.1 as usize * width + centre.0 as usize];
    (if weight > 0.0 { sum / weight } else { 0.0 }, farthest, at_centre)
}

/// The leftmost and rightmost drawn columns.
fn extent(alpha: &[u8], width: usize) -> (usize, usize) {
    let columns = alpha.iter().enumerate().filter(|(_, value)| **value > 0).map(|(index, _)| index % width);
    columns.fold((usize::MAX, 0), |(left, right), x| (left.min(x), right.max(x)))
}

fn save(overlay: &Overlay, directory: Option<&Path>, name: &str) -> Result<(), String> {
    let Some(directory) = directory else { return Ok(()) };
    let texture = render(overlay, Some(gdk::RGBA::new(0.16, 0.18, 0.22, 1.0)))?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("bubble-{name}.png"));
    texture.save_to_png(&path).map_err(|error| error.to_string())?;
    println!("wrote {} ({}x{})", path.display(), texture.width(), texture.height());
    Ok(())
}

fn no_overlay_without_layer_shell() -> Result<(), String> {
    if gdk::Display::default().is_some_and(|display| display.backend().is_wayland()) {
        println!("on Wayland, layer-shell depends on the compositor: not checked");
        return Ok(());
    }
    match Overlay::new() {
        None => Ok(()),
        Some(overlay) => Err(format!("a layer-shell overlay outside Wayland: {overlay:?}")),
    }
}

fn bubble_follows_simulated_gaze(frames: Option<&Path>) -> Result<(), String> {
    let monitor = gdk::Display::default()
        .and_then(|display| display.monitors().item(0))
        .and_downcast::<gdk::Monitor>()
        .ok_or("no monitor")?;
    let geometry = monitor.geometry();
    let (width, height) = (f64::from(geometry.width()), f64::from(geometry.height()));
    let overlay = Overlay::windowed();
    let gaze = Rc::new(Cell::new((0.3, 0.5)));
    let feeder = {
        let (overlay, gaze) = (overlay.clone(), gaze.clone());
        glib::timeout_add_local(Duration::from_millis(8), move || {
            let (x, y) = gaze.get();
            overlay.add(GazeSample { timestamp_us: 0, x, y, valid: true });
            glib::ControlFlow::Continue
        })
    };
    overlay.show_on(Some(&monitor));
    if !overlay.is_shown() {
        return Err("not shown".into());
    }
    pump(Duration::from_millis(700));
    if let Some(error) = overlay.area().error() {
        return Err(format!("GLArea: {error}"));
    }
    let description = overlay.renderer_description().ok_or("nothing was rendered")?;
    println!("renderer: {description}");

    let window = overlay.window().clone();
    let still = render(&overlay, None)?;
    save(&overlay, frames, "1-still")?;
    let (values, stride) = alpha(&still);
    let radius = SIZE * height;
    let (mean, farthest, centre) = ring(&values, stride, (0.3 * width, 0.5 * height));
    println!("still gaze: mean drawn distance {mean:.1} px, farthest {farthest:.1} px, radius {radius:.1} px");
    if !(0.8 * radius..=1.05 * radius).contains(&mean) || farthest > 1.15 * radius || centre != 0 {
        return Err(format!(
            "not a ring of radius {radius:.1}: mean {mean:.1}, farthest {farthest:.1}, centre {centre}"
        ));
    }

    let start = Instant::now();
    let travel = Duration::from_millis(500);
    let mut moving = None;
    while start.elapsed() < travel {
        let t = start.elapsed().as_secs_f64() / travel.as_secs_f64();
        gaze.set((0.3 + 0.4 * t, 0.5 - 0.1 * (t * std::f64::consts::PI).sin()));
        pump(Duration::from_millis(8));
        if moving.is_none() && t >= 0.5 {
            moving = Some(render(&overlay, None)?);
            save(&overlay, frames, "2-moving")?;
        }
    }
    let (values, stride) = alpha(&moving.ok_or("no frame while moving")?);
    let (left, right) = extent(&values, stride);
    println!("moving: drawn from x {left} to {right}");
    if (right.saturating_sub(left) as f64) < 2.5 * radius {
        return Err(format!("no trail behind the moving gaze: drawn from x {left} to {right}"));
    }
    gaze.set((0.7, 0.5));
    pump(Duration::from_millis(120));
    save(&overlay, frames, "3-stopped")?;

    // Just outside the ring the trail keeps about 0.95 per tick, so falling from the steady
    // 57 to the gradient's first visible texel takes about 172 ticks (2.1 s).
    pump(Duration::from_millis(2500));
    let settled = render(&overlay, None)?;
    save(&overlay, frames, "4-settled")?;
    let (values, stride) = alpha(&settled);
    let (mean, farthest, _) = ring(&values, stride, (0.7 * width, 0.5 * height));
    println!("settled: mean drawn distance {mean:.1} px, farthest {farthest:.1} px");
    if farthest > 1.15 * radius {
        return Err(format!("the trail did not fade: farthest {farthest:.1} px"));
    }

    gaze.set((0.45, 0.5));
    pump(Duration::from_millis(30));
    overlay.show_on(None);
    if overlay.is_shown() || window.is_visible() {
        return Err("still shown after show_on(None)".into());
    }
    overlay.show_on(Some(&monitor));
    pump(Duration::from_millis(50));
    let again = render(&overlay, None)?;
    let (values, stride) = alpha(&again);
    let (_, farthest, _) = ring(&values, stride, (0.45 * width, 0.5 * height));
    println!("shown again: farthest drawn pixel {farthest:.1} px");
    if farthest > 1.15 * radius {
        return Err(format!("the old trail survived hiding: farthest {farthest:.1} px"));
    }
    overlay.show_on(None);
    feeder.remove();
    window.destroy();
    pump(Duration::from_millis(50));
    Ok(())
}

fn main() {
    if std::env::var_os("GDK_BACKEND").is_none() || gtk::init().is_err() {
        if std::env::var_os(REQUIRE_DISPLAY).is_some() {
            println!("test overlay ... FAILED: no display, and {REQUIRE_DISPLAY} is set");
            std::process::exit(1);
        }
        println!("test overlay ... skipped (no display; run make experience-test)");
        return;
    }
    let frames = std::env::var_os(FRAMES).map(PathBuf::from);
    let results = [
        ("no_overlay_without_layer_shell", no_overlay_without_layer_shell()),
        ("bubble_follows_simulated_gaze", bubble_follows_simulated_gaze(frames.as_deref())),
    ];
    let mut failed = false;
    for (name, result) in results {
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
