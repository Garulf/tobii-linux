//! The tracker alignment step of Tobii's display setup (ScreenPlaneSetupControl): layout,
//! line dragging, keyboard nudging and the intro and slide-in animations, without drawing.
//!
//! Layout: a 60 px header, then rows 0.35 : 1 : 1 : 2; text at the bottom of row 1, buttons
//! in row 2, the lines in row 3. The tracker sits 150 px above the bottom. Times are
//! seconds on any monotonic clock.

use tobii_gaze::display::{MAX_LINE, MIN_LINE, MIN_LINE_DISTANCE};

use crate::artwork::{FLAT_TRACKER, MONITOR_WIDTH, SWITCHER_CONTENT, TRACKER};
use crate::motion::{ease_back_in_out, ease_quadratic_in_out};
use crate::screen::DisplayLines;

pub const HEADER: f64 = 60.0;
pub const ROWS: [f64; 4] = [0.35, 1.0, 1.0, 2.0];
pub const BOTTOM_MARGIN: f64 = 150.0;
pub const HANDLE_HALF: f64 = 30.0;
pub const INTRO_MONITOR_WIDTH: f64 = 400.0;
pub const INTRO_TRACKER_TOP: f64 = 222.0;
pub const INTRO_TRACKER_WIDTH: f64 = 250.0;
pub const SWITCHER_WIDTH: f64 = 134.0;
pub const SWITCHER_MARGIN: f64 = 15.0;
pub const MOVE_S: f64 = 1.0;
pub const CROSS_FADE_S: f64 = 0.5;
pub const SLIDE_S: f64 = 1.5;
pub const SLIDE_OFFSET: f64 = 0.01;

pub const WHERE: &str = "Where is your eye tracker?";
pub const ATTACH: &str = "If you haven't yet attached it to the display, now is the time.";
pub const THIS_DISPLAY: &str = "This display";
pub const DONE: &str = "Done";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// Top and height of each of the four rows below the header.
pub fn rows(height: f64) -> [(f64, f64); 4] {
    let total: f64 = ROWS.iter().sum();
    let content = height - HEADER;
    let mut top = HEADER;
    ROWS.map(|weight| {
        let row = (top, content * weight / total);
        top += row.1;
        row
    })
}

/// Where the text, the buttons and the switchers go in a window of `width` x `height`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub text: Rect,
    pub buttons: Rect,
    pub previous: Rect,
    pub next: Rect,
    pub canvas_top: f64,
}

pub fn layout(width: f64, height: f64) -> Layout {
    let rows = rows(height);
    let switcher_height = SWITCHER_WIDTH * SWITCHER_CONTENT[1] / SWITCHER_CONTENT[0];
    let switcher_top = rows[2].0 + (rows[2].1 - switcher_height) / 2.0;
    Layout {
        text: Rect { x: 0.0, y: rows[1].0, width, height: rows[1].1 },
        buttons: Rect { x: width / 5.0, y: rows[2].0, width: width * 3.0 / 5.0, height: rows[2].1 },
        previous: Rect { x: SWITCHER_MARGIN, y: switcher_top, width: SWITCHER_WIDTH, height: switcher_height },
        next: Rect {
            x: width - SWITCHER_MARGIN - SWITCHER_WIDTH,
            y: switcher_top,
            width: SWITCHER_WIDTH,
            height: switcher_height,
        },
        canvas_top: rows[3].0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// "Where is your eye tracker?" with the monitor drawing and "This display".
    Intro,
    /// The tracker picture moves into place, then cross-fades to the detailed drawing.
    Moving,
    /// The lines slide in from 0.01 outside.
    Sliding,
    Lines,
    /// Done was pressed; the flow decides what comes next.
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    Line(Side),
    Tracker,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    grab: Grab,
    to_left: f64,
    to_right: f64,
}

/// Keys the lines step understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKey {
    Enter,
    Tab,
    L,
    R,
    Left,
    Right,
}

/// What a key did on the lines step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyOutcome {
    Ignored,
    Redraw,
    ThisDisplay,
    Done(f64, f64),
}

/// The intro picture at one moment: the monitor's opacity, the flat tracker's box and
/// opacity, and the detailed tracker's box and opacity once the cross-fade started.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntroFrame {
    pub monitor: Rect,
    pub monitor_alpha: f64,
    pub flat: Rect,
    pub flat_alpha: f64,
    pub detailed: Option<(Rect, f64)>,
}

/// One line as drawn: x, whether it is highlighted, and the middle of its side triangles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    pub x: f64,
    pub active: bool,
    pub top: f64,
    pub middle: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    pub config: Option<DisplayLines>,
    pub phase: Phase,
    pub left: f64,
    pub right: f64,
    pub width: f64,
    pub height: f64,
    /// Device pixels per window pixel, for keyboard nudges of one device pixel.
    pub scale: f64,
    pub tracker_top: Option<f64>,
    pub hover: Option<Grab>,
    pub keyboard_line: Option<Side>,
    pub intro_progress: f64,
    pub cross_fade: f64,
    /// When the text and buttons of the current phase started fading in.
    pub shown_at: f64,
    drag: Option<Drag>,
    started_at: f64,
}

impl Default for Alignment {
    fn default() -> Self {
        Self {
            config: None,
            phase: Phase::Idle,
            left: 0.0,
            right: 1.0,
            width: 1280.0,
            height: 720.0,
            scale: 1.0,
            tracker_top: None,
            hover: None,
            keyboard_line: None,
            intro_progress: 0.0,
            cross_fade: 0.0,
            shown_at: 0.0,
            drag: None,
            started_at: 0.0,
        }
    }
}

impl Alignment {
    pub fn resize(&mut self, width: f64, height: f64, scale: f64) {
        (self.width, self.height, self.scale) = (width, height, scale);
    }

    pub fn show(&mut self, config: DisplayLines, now: f64) {
        (self.left, self.right) = (config.left, config.right);
        self.hover = None;
        self.drag = None;
        self.keyboard_line = None;
        self.tracker_top = None;
        let intro = config.intro;
        self.config = Some(config);
        if intro {
            self.phase = Phase::Intro;
            self.intro_progress = 0.0;
            self.cross_fade = 0.0;
            self.shown_at = now;
        } else {
            self.show_lines(now);
        }
    }

    fn show_lines(&mut self, now: f64) {
        self.phase = Phase::Sliding;
        self.started_at = now;
        self.shown_at = now;
    }

    /// Whether the text shows the instructions and Done (rather than the intro and "This display").
    pub fn shows_lines(&self) -> bool {
        matches!(self.phase, Phase::Sliding | Phase::Lines | Phase::Finished)
    }

    pub fn animating(&self) -> bool {
        matches!(self.phase, Phase::Moving | Phase::Sliding)
    }

    /// "This display": the tracker moves into place, then the lines slide in.
    pub fn choose_this_display(&mut self, now: f64) -> bool {
        if self.phase != Phase::Intro {
            return false;
        }
        self.phase = Phase::Moving;
        self.started_at = now;
        true
    }

    /// Advances the animations to `now`.
    pub fn tick(&mut self, now: f64) {
        let elapsed = now - self.started_at;
        match self.phase {
            Phase::Moving => {
                self.intro_progress = (elapsed / MOVE_S).min(1.0);
                self.cross_fade = ((elapsed - MOVE_S) / CROSS_FADE_S).clamp(0.0, 1.0);
                if elapsed >= MOVE_S + CROSS_FADE_S {
                    self.tracker_top = Some(self.intro_frame().detailed.map_or(0.0, |(rect, _)| rect.y));
                    self.show_lines(now);
                }
            }
            Phase::Sliding => {
                let Some(config) = &self.config else { return };
                let t = (elapsed / SLIDE_S).min(1.0);
                let eased = ease_back_in_out(t);
                self.left = config.left - SLIDE_OFFSET * (1.0 - eased);
                self.right = config.right + SLIDE_OFFSET * (1.0 - eased);
                if t >= 1.0 {
                    (self.left, self.right) = (config.left, config.right);
                    self.phase = Phase::Lines;
                }
            }
            _ => {}
        }
    }

    pub fn done_enabled(&self, can_finish: bool) -> bool {
        self.phase == Phase::Lines && can_finish
    }

    /// Done: the line positions when the lines are up and the window fills the screen.
    pub fn finish(&mut self, can_finish: bool) -> Option<(f64, f64)> {
        if !self.done_enabled(can_finish) {
            return None;
        }
        self.phase = Phase::Finished;
        Some((self.left, self.right))
    }

    // Geometry ---------------------------------------------------------------------------

    fn proportion(&self) -> f64 {
        self.config.as_ref().map_or(1.0, |c| c.picture_proportion)
    }

    /// The detailed tracker's box for lines at `left` and `right`.
    pub fn tracker_box_at(&self, left: f64, right: f64) -> Rect {
        let width = (right - left) * self.proportion() * self.width;
        let height = width * TRACKER.height / TRACKER.width;
        let top = self.tracker_top.unwrap_or(self.height - height - BOTTOM_MARGIN);
        Rect { x: (left + right) / 2.0 * self.width - width / 2.0, y: top, width, height }
    }

    pub fn tracker_box(&self) -> Rect {
        self.tracker_box_at(self.left, self.right)
    }

    /// Where the lines start: a quarter down the tracker drawing, at the end of its marks.
    pub fn line_top(&self) -> f64 {
        let tracker = self.tracker_box();
        tracker.y + tracker.height / 4.0
    }

    fn side(&self, side: Side) -> f64 {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }

    pub fn grab_at(&self, x: f64, y: f64) -> Option<Grab> {
        if !matches!(self.phase, Phase::Lines | Phase::Sliding) {
            return None;
        }
        let line_top = self.line_top();
        let near = [Side::Left, Side::Right]
            .into_iter()
            .map(|side| (side, (x - self.side(side) * self.width).abs()))
            .filter(|(_, distance)| *distance <= HANDLE_HALF && y >= line_top)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((side, _)) = near {
            return Some(Grab::Line(side));
        }
        self.tracker_box().contains(x, y).then_some(Grab::Tracker)
    }

    fn set_left(&mut self, value: f64) {
        self.left = value.min(self.right - MIN_LINE_DISTANCE).max(MIN_LINE);
    }

    fn set_right(&mut self, value: f64) {
        self.right = value.max(self.left + MIN_LINE_DISTANCE).min(MAX_LINE);
    }

    // Pointer and keyboard -----------------------------------------------------------------

    pub fn press(&mut self, x: f64, y: f64) {
        if self.phase != Phase::Lines {
            return;
        }
        let Some(grab) = self.grab_at(x, y) else { return };
        self.keyboard_line = None;
        self.drag = Some(Drag { grab, to_left: self.left * self.width - x, to_right: self.right * self.width - x });
    }

    /// Pointer motion; returns whether the pointer is over (or dragging) something movable.
    pub fn motion(&mut self, x: f64, y: f64) -> bool {
        if let Some(drag) = self.drag.filter(|_| self.phase == Phase::Lines) {
            if drag.grab != Grab::Line(Side::Right) {
                self.set_left((x + drag.to_left) / self.width);
            }
            if drag.grab != Grab::Line(Side::Left) {
                self.set_right((x + drag.to_right) / self.width);
            }
        }
        self.keyboard_line = None;
        self.hover = self.grab_at(x, y);
        self.drag.is_some() || self.hover.is_some()
    }

    pub fn release(&mut self) {
        self.drag = None;
    }

    pub fn leave(&mut self) {
        self.hover = None;
        self.drag = None;
    }

    /// Enter is the default button; Tab, L and R pick a line; the arrows nudge the last used
    /// line by a device pixel (`shift`: ten).
    pub fn key(&mut self, key: LineKey, shift: bool, can_finish: bool, now: f64) -> KeyOutcome {
        if key == LineKey::Enter {
            if self.phase == Phase::Intro {
                return if self.choose_this_display(now) { KeyOutcome::ThisDisplay } else { KeyOutcome::Ignored };
            }
            return self.finish(can_finish).map_or(KeyOutcome::Ignored, |(l, r)| KeyOutcome::Done(l, r));
        }
        if self.phase != Phase::Lines {
            return KeyOutcome::Ignored;
        }
        match key {
            LineKey::Tab => {
                self.keyboard_line =
                    Some(if self.keyboard_line == Some(Side::Right) { Side::Left } else { Side::Right });
            }
            LineKey::L => self.keyboard_line = Some(Side::Left),
            LineKey::R => self.keyboard_line = Some(Side::Right),
            LineKey::Left | LineKey::Right => {
                let hovered = if self.hover == Some(Grab::Line(Side::Right)) { Side::Right } else { Side::Left };
                let line = self.keyboard_line.unwrap_or(hovered);
                self.keyboard_line = Some(line);
                let step = if shift { 10.0 } else { 1.0 } / (self.scale * self.width);
                let delta = if key == LineKey::Left { -step } else { step };
                match line {
                    Side::Left => self.set_left(self.left + delta),
                    Side::Right => self.set_right(self.right + delta),
                }
            }
            LineKey::Enter => unreachable!("handled above"),
        }
        KeyOutcome::Redraw
    }

    // Drawing ----------------------------------------------------------------------------

    pub fn handle(&self, side: Side) -> Handle {
        let line_top = self.line_top();
        let active = self.hover == Some(Grab::Line(side)) || self.keyboard_line == Some(side);
        Handle {
            x: (self.side(side) * self.width).round() + 0.5,
            active,
            top: line_top,
            middle: (line_top + self.height) / 2.0 + 7.5,
        }
    }

    /// The intro picture: the monitor, the flat tracker moving to where the detailed one
    /// will sit, and the cross-fade to the detailed tracker.
    pub fn intro_frame(&self) -> IntroFrame {
        let canvas_top = rows(self.height)[3].0;
        let progress = ease_quadratic_in_out(self.intro_progress);
        let scale = INTRO_MONITOR_WIDTH / MONITOR_WIDTH;
        let monitor = Rect {
            x: (self.width - INTRO_MONITOR_WIDTH) / 2.0,
            y: canvas_top,
            width: INTRO_MONITOR_WIDTH,
            height: crate::artwork::MONITOR_HEIGHT * scale,
        };
        let flat_height = INTRO_TRACKER_WIDTH * FLAT_TRACKER.height / FLAT_TRACKER.width;
        let flat_top = canvas_top + INTRO_TRACKER_TOP;
        let (left, right) = self.config.as_ref().map_or((self.left, self.right), |c| (c.left, c.right));
        let mut placed = self.clone();
        placed.tracker_top = None;
        let target = placed.tracker_box_at(left, right);
        let detail_top = (flat_top - target.height / 4.0).min(target.y);
        let flat_target_top = detail_top + target.height / 4.0;
        let mix = |a: f64, b: f64| a + (b - a) * progress;
        let flat = Rect {
            x: mix((self.width - INTRO_TRACKER_WIDTH) / 2.0, target.x),
            y: mix(flat_top, flat_target_top),
            width: mix(INTRO_TRACKER_WIDTH, target.width),
            height: mix(flat_height, target.width * FLAT_TRACKER.height / FLAT_TRACKER.width),
        };
        let detailed = (self.cross_fade > 0.0).then_some((Rect { y: detail_top, ..target }, self.cross_fade));
        IntroFrame {
            monitor,
            monitor_alpha: 1.0 - self.intro_progress,
            flat,
            flat_alpha: 1.0 - self.cross_fade,
            detailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(intro: bool) -> DisplayLines {
        DisplayLines {
            intro,
            left: 0.4,
            right: 0.6,
            aspect: 16.0 / 9.0,
            tracker_width_mm: 184.0,
            picture_proportion: 286.0 / 184.0,
            instructions: "Move the lines to the marks on top of your eye tracker.",
            previous: false,
            next: false,
        }
    }

    fn ready(intro: bool) -> Alignment {
        let mut alignment = Alignment::default();
        alignment.resize(1280.0, 720.0, 1.0);
        alignment.show(lines(intro), 0.0);
        alignment.tick(10.0);
        alignment
    }

    #[test]
    fn rows_follow_the_grid() {
        let rows = rows(720.0);
        assert_eq!(rows[0].0, 60.0);
        assert!((rows[3].0 + rows[3].1 - 720.0).abs() < 1e-9);
        assert!((rows[3].1 / rows[1].1 - 2.0).abs() < 1e-9);
        let layout = layout(2560.0, 1440.0);
        assert_eq!((layout.previous.x, layout.previous.width), (15.0, 134.0));
        assert!((layout.previous.height - 68.25).abs() < 0.01);
    }

    #[test]
    fn the_lines_slide_in_and_then_take_done() {
        let mut alignment = Alignment::default();
        alignment.show(lines(false), 0.0);
        assert_eq!(alignment.finish(true), None);
        alignment.tick(0.75);
        assert!((alignment.left - 0.4).abs() < 0.011 && alignment.phase == Phase::Sliding);
        alignment.tick(1.5);
        assert_eq!((alignment.phase, alignment.left, alignment.right), (Phase::Lines, 0.4, 0.6));
        assert_eq!(alignment.finish(false), None);
        assert_eq!(alignment.finish(true), Some((0.4, 0.6)));
        assert_eq!(alignment.phase, Phase::Finished);
    }

    #[test]
    fn this_display_moves_the_tracker_then_shows_the_lines() {
        let mut alignment = Alignment::default();
        alignment.show(lines(true), 0.0);
        assert!(!alignment.shows_lines());
        assert_eq!(alignment.key(LineKey::Enter, false, true, 1.0), KeyOutcome::ThisDisplay);
        alignment.tick(2.25);
        assert!(alignment.intro_frame().detailed.is_some());
        assert_eq!(alignment.phase, Phase::Moving);
        alignment.tick(2.6);
        assert!(alignment.shows_lines());
        let frame_top = alignment.tracker_top.unwrap();
        alignment.tick(10.0);
        assert_eq!(alignment.tracker_box().y, frame_top);
    }

    #[test]
    fn pressing_near_a_line_drags_only_it_and_the_tracker_drags_both() {
        let mut alignment = ready(false);
        let below = alignment.line_top() + 40.0;
        alignment.press(0.4 * 1280.0 + 10.0, below);
        alignment.motion(0.4 * 1280.0 + 40.0, below);
        alignment.release();
        assert!((alignment.left - (0.4 + 30.0 / 1280.0)).abs() < 1e-9 && alignment.right == 0.6);
        let tracker = alignment.tracker_box();
        let (left, right) = (alignment.left, alignment.right);
        alignment.press(tracker.x + tracker.width / 2.0 + 50.0, tracker.y + tracker.height / 2.0);
        alignment.motion(tracker.x + tracker.width / 2.0 + 114.0, tracker.y + tracker.height / 2.0);
        assert!((alignment.left - left - 0.05).abs() < 1e-9 && (alignment.right - right - 0.05).abs() < 1e-9);
        alignment.release();
        let above = alignment.tracker_box().y - 5.0;
        assert_eq!(alignment.grab_at(alignment.left * 1280.0, above), None);
        let inside = alignment.line_top() - 2.0;
        assert_eq!(alignment.grab_at(alignment.left * 1280.0, inside), Some(Grab::Tracker));
    }

    #[test]
    fn keys_nudge_the_chosen_line_by_device_pixels() {
        let mut alignment = ready(false);
        alignment.key(LineKey::Right, true, true, 11.0);
        assert!((alignment.left - (0.4 + 10.0 / 1280.0)).abs() < 1e-12);
        assert_eq!(alignment.keyboard_line, Some(Side::Left));
        alignment.key(LineKey::Tab, false, true, 11.0);
        alignment.key(LineKey::Left, false, true, 11.0);
        assert!((alignment.right - (0.6 - 1.0 / 1280.0)).abs() < 1e-12);
        assert!(alignment.handle(Side::Right).active && !alignment.handle(Side::Left).active);
        assert_eq!(alignment.key(LineKey::Enter, false, true, 11.0), KeyOutcome::Done(alignment.left, alignment.right));
    }

    #[test]
    fn lines_keep_their_limits() {
        let mut alignment = ready(false);
        for _ in 0..200 {
            alignment.key(LineKey::Right, true, true, 11.0);
        }
        assert!((alignment.left - (0.6 - MIN_LINE_DISTANCE)).abs() < 1e-12);
        alignment.key(LineKey::R, false, true, 11.0);
        for _ in 0..200 {
            alignment.key(LineKey::Right, true, true, 11.0);
        }
        assert_eq!(alignment.right, MAX_LINE);
    }
}
