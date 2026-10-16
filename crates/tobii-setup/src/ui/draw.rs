//! Cairo drawing of the artwork, the lines, the eyes, the particles and the spinner.

use gtk4::cairo::{Context, LineCap, LineJoin};
use gtk4::gdk::RGBA;

use crate::alignment::{Alignment, Phase, Rect, Side};
use crate::artwork::{
    Artwork, FLAT_TRACKER, MONITOR_BODY, MONITOR_SCREEN, MONITOR_STAND, MONITOR_WIDTH, SWITCHER_CONTENT, Segment,
    TRACKER, parse_path, switcher_parts,
};
use crate::motion::keyframes;
use crate::particles::{CalibrationView, DOT_SIZE};

pub const FOREGROUND: [f64; 3] = rgb(0xaf, 0xc4, 0xcd);
pub const ACCENT: [f64; 3] = rgb(0x00, 0x8d, 0xbf);
pub const WHITE: [f64; 3] = [1.0, 1.0, 1.0];
pub const BLACK: [f64; 3] = [0.0, 0.0, 0.0];

const fn rgb(r: u8, g: u8, b: u8) -> [f64; 3] {
    [r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0]
}

fn colour(cr: &Context, [r, g, b]: [f64; 3], alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}

pub fn rgba_colour(cr: &Context, rgba: &RGBA) {
    cr.set_source_rgba(rgba.red().into(), rgba.green().into(), rgba.blue().into(), rgba.alpha().into());
}

pub fn path(cr: &Context, data: &str) {
    for segment in parse_path(data) {
        match segment {
            Segment::Move([x, y]) => cr.move_to(x, y),
            Segment::Line([x, y]) => cr.line_to(x, y),
            Segment::Cubic([x1, y1], [x2, y2], [x, y]) => cr.curve_to(x1, y1, x2, y2, x, y),
            Segment::Close => cr.close_path(),
        }
    }
}

/// Black inside where the drawing is filled, white strokes, scaled into `place`.
pub fn artwork(cr: &Context, art: &Artwork, place: Rect, alpha: f64) {
    if alpha <= 0.0 || place.width <= 0.0 {
        return;
    }
    cr.save().ok();
    cr.translate(place.x, place.y);
    cr.scale(place.width / art.width, place.height / art.height);
    cr.set_line_join(LineJoin::Miter);
    cr.set_miter_limit(1.0);
    for (data, stroke, filled) in art.paths {
        cr.new_path();
        path(cr, data);
        if *filled {
            colour(cr, BLACK, alpha);
            cr.fill_preserve().ok();
        }
        cr.set_line_width(*stroke);
        colour(cr, WHITE, alpha);
        cr.stroke().ok();
    }
    cr.restore().ok();
}

/// Screen.xaml: body and stand in `fill`, a black screen; in a box `width` wide at `x`, `y`.
pub fn monitor(cr: &Context, x: f64, y: f64, width: f64, fill: [f64; 4]) {
    let scale = width / MONITOR_WIDTH;
    cr.save().ok();
    cr.translate(x, y);
    cr.scale(scale, scale);
    cr.set_source_rgba(fill[0], fill[1], fill[2], fill[3]);
    cr.new_path();
    path(cr, MONITOR_BODY);
    cr.fill().ok();
    path(cr, MONITOR_STAND);
    cr.fill().ok();
    colour(cr, BLACK, fill[3]);
    let [sx, sy, sw, sh] = MONITOR_SCREEN;
    cr.rectangle(sx, sy, sw, sh);
    cr.fill().ok();
    cr.restore().ok();
}

/// The monitor switcher's icon in a `width` x `height` area, in the widget's colour.
pub fn switcher(cr: &Context, previous: bool, width: f64, height: f64, rgba: &RGBA) {
    let scale = (width / SWITCHER_CONTENT[0]).min(height / SWITCHER_CONTENT[1]);
    let fill = [rgba.red().into(), rgba.green().into(), rgba.blue().into(), rgba.alpha().into()];
    let (arrow, monitor_x) = switcher_parts(previous);
    cr.save().ok();
    cr.translate((width - SWITCHER_CONTENT[0] * scale) / 2.0, (height - SWITCHER_CONTENT[1] * scale) / 2.0);
    cr.scale(scale, scale);
    rgba_colour(cr, rgba);
    cr.move_to(arrow[0][0], arrow[0][1]);
    cr.line_to(arrow[1][0], arrow[1][1]);
    cr.line_to(arrow[2][0], arrow[2][1]);
    cr.close_path();
    cr.fill_preserve().ok();
    cr.set_line_width(1.0);
    cr.stroke().ok();
    monitor(cr, monitor_x, 0.0, MONITOR_WIDTH, fill);
    cr.restore().ok();
}

/// The close button's cross: a 60-unit view box drawn into `size`.
pub fn cross(cr: &Context, size: f64, rgba: &RGBA) {
    cr.save().ok();
    cr.scale(size / 60.0, size / 60.0);
    rgba_colour(cr, rgba);
    cr.set_line_width(1.1);
    cr.set_line_cap(LineCap::Round);
    cr.move_to(15.0, 15.0);
    cr.line_to(45.0, 45.0);
    cr.move_to(15.0, 45.0);
    cr.line_to(45.0, 15.0);
    cr.stroke().ok();
    cr.restore().ok();
}

/// The page's spinner: twelve dots fading in turn every 1.2 s, in a 48 px box at `x`, `y`.
pub fn spinner(cr: &Context, x: f64, y: f64, scale: f64, now: f64) {
    cr.save().ok();
    cr.translate(x, y);
    cr.scale(scale, scale);
    for i in 0..12 {
        let delay = f64::from(i) * 0.1 - 1.2;
        let phase = ((now - delay) / 1.2).rem_euclid(1.0);
        let opacity = keyframes(phase, &[(0.0, 1.0), (0.75, 0.1), (1.0, 0.1)]);
        cr.save().ok();
        cr.translate(24.0, 24.0);
        cr.rotate(f64::from(i) * 30.0_f64.to_radians());
        colour(cr, FOREGROUND, opacity);
        cr.arc(0.0, -19.0, 3.0, 0.0, std::f64::consts::TAU);
        cr.fill().ok();
        cr.restore().ok();
    }
    cr.restore().ok();
}

fn handle(cr: &Context, alignment: &Alignment, side: Side, height: f64) {
    let handle = alignment.handle(side);
    let x = handle.x;
    colour(cr, if handle.active { WHITE } else { FOREGROUND }, 1.0);
    cr.set_line_width(1.0);
    cr.set_dash(&[5.0, 5.0], 0.0);
    cr.move_to(x, handle.top);
    cr.line_to(x, height);
    cr.stroke().ok();
    cr.set_dash(&[], 0.0);
    cr.move_to(x - 6.0, height - 10.0);
    cr.line_to(x, height);
    cr.line_to(x + 6.0, height - 10.0);
    cr.stroke().ok();
    colour(cr, WHITE, 1.0);
    let middle = handle.middle;
    for direction in [-1.0, 1.0] {
        cr.move_to(x + 24.0 * direction, middle);
        cr.line_to(x + 10.0 * direction, middle - 14.0);
        cr.line_to(x + 10.0 * direction, middle + 14.0);
        cr.close_path();
        cr.fill_preserve().ok();
        cr.stroke().ok();
    }
}

/// The alignment canvas: the intro pictures, or the tracker and its two lines.
pub fn alignment(cr: &Context, alignment: &Alignment, canvas_top: f64) {
    let (width, height) = (alignment.width, alignment.height);
    cr.save().ok();
    cr.rectangle(0.0, canvas_top, width, height - canvas_top);
    cr.clip();
    match alignment.phase {
        Phase::Idle => {}
        Phase::Intro | Phase::Moving => {
            let frame = alignment.intro_frame();
            let [r, g, b] = FOREGROUND;
            monitor(cr, frame.monitor.x, frame.monitor.y, frame.monitor.width, [r, g, b, frame.monitor_alpha]);
            artwork(cr, &FLAT_TRACKER, frame.flat, frame.flat_alpha);
            if let Some((place, alpha)) = frame.detailed {
                artwork(cr, &TRACKER, place, alpha);
            }
        }
        _ => {
            artwork(cr, &TRACKER, alignment.tracker_box(), 1.0);
            handle(cr, alignment, Side::Left, height);
            handle(cr, alignment, Side::Right, height);
        }
    }
    cr.restore().ok();
}

/// The calibration screen: black, the particles, the centre dot and the spinner.
pub fn calibration(cr: &Context, view: &CalibrationView, now: f64, width: f64, height: f64) {
    colour(cr, BLACK, 1.0);
    cr.paint().ok();
    let overlays = view.overlays(now);
    for particle in view.particles(now, width, height) {
        let opacity = particle.opacity * overlays.targets;
        if opacity <= 0.0 || particle.radii[0] <= 0.0 {
            continue;
        }
        cr.save().ok();
        cr.translate(particle.centre[0], particle.centre[1]);
        cr.rotate(particle.degrees.to_radians());
        cr.scale(particle.radii[0], particle.radii[1]);
        cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
        cr.restore().ok();
        colour(cr, ACCENT, opacity);
        cr.fill().ok();
    }
    if overlays.dot_scale > 0.0 {
        colour(cr, ACCENT, 1.0);
        cr.arc(width / 2.0, height / 2.0, DOT_SIZE / 2.0 * overlays.dot_scale, 0.0, std::f64::consts::TAU);
        cr.fill().ok();
    }
    if overlays.spinner {
        spinner(cr, width / 2.0 - 36.0, height / 2.0 - 36.0, 1.5, now);
    }
}

/// A filled circle, for the eyes.
pub fn disc(cr: &Context, centre: [f64; 2], diameter: f64, grey: f64, alpha: f64, vertical_scale: f64) {
    if alpha <= 0.0 || vertical_scale <= 0.0 {
        return;
    }
    cr.save().ok();
    cr.translate(centre[0], centre[1]);
    cr.scale(diameter / 2.0, diameter / 2.0 * vertical_scale);
    cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
    cr.restore().ok();
    cr.set_source_rgba(grey, grey, grey, alpha);
    cr.fill().ok();
}

pub fn fill_rect(cr: &Context, rect: [f64; 4], rgba: [f64; 4]) {
    cr.set_source_rgba(rgba[0], rgba[1], rgba[2], rgba[3]);
    cr.rectangle(rect[0], rect[1], rect[2], rect[3]);
    cr.fill().ok();
}
