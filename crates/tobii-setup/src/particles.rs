//! The calibration screen's targets and overlays as timelines (CalibrationProcessView,
//! docs/setup-flow.md): targets of 20 accent-coloured particles sitting on one point that
//! fly out from the centre, squash and spin while focused, and explode when collected.
//! Times are seconds on any monotonic clock; positions are window pixels.

use crate::motion::{cubic_bezier, keyframes, progress, timeline};
use crate::screen::CalibrationPhase;

pub const PARTICLES: usize = 20;
pub const LARGEST_PARTICLE: f64 = 18.0;
pub const FOCUS_S: f64 = 1.0;
pub const SPIN_S: f64 = 2.0;
pub const SPIN_DEGREES: f64 = 3240.0;
pub const UNFOCUS_S: f64 = 0.2;
pub const FADE_S: f64 = 0.4;
pub const WELCOME_S: f64 = 2.5;
pub const DOT_S: f64 = 2.5;
pub const DOT_SIZE: f64 = 15.0;
pub const SPINNER_AFTER_S: f64 = 0.5;
pub const RESTART_FADE_S: f64 = 0.2;
pub const RESTART_PAUSE_S: f64 = 0.9;

/// How long the window takes to draw a calibration phase before acknowledging it.
pub fn ack_delay_s(phase: &CalibrationPhase) -> f64 {
    match phase {
        CalibrationPhase::Intro { welcome, again } => {
            (if *welcome { WELCOME_S } else { 0.0 }) + instruction_hold_s(*again) - 1.25
        }
        CalibrationPhase::Targets { targets, .. } => spawn_s(targets.len()),
        CalibrationPhase::Computed { stage } => {
            if *stage == 0 {
                0.5
            } else {
                0.7
            }
        }
        CalibrationPhase::Restart { .. } => RESTART_FADE_S + RESTART_PAUSE_S,
        _ => 0.0,
    }
}

/// How long "Look at each dot until it explodes!" stays: longer after a restart.
pub fn instruction_hold_s(again: bool) -> f64 {
    if again { 3.0 } else { 2.0 }
}

/// How long targets take to fly out: 1.5 s for one, 1 s for several.
pub fn spawn_s(count: usize) -> f64 {
    if count == 1 { 1.5 } else { 1.0 }
}

/// Uniform numbers in 0..1 for the particles' random sizes, spread and fade delays.
#[derive(Debug, Clone)]
pub struct Random(u64);

impl Random {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + self.uniform() * (high - low)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub size: f64,
    /// Where it flies when the target explodes, relative to the target.
    pub offset: [f64; 2],
    pub fade_delay_s: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    Resting,
    Focused { since: f64 },
    Unfocusing { since: f64, from: (f64, [f64; 2]) },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub index: usize,
    pub x: f64,
    pub y: f64,
    pub particles: Vec<Particle>,
    spawned_at: f64,
    spawn_s: f64,
    focus: Focus,
    exploded_at: Option<f64>,
}

/// One particle as drawn: centre, radii, rotation in degrees and opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleDraw {
    pub centre: [f64; 2],
    pub radii: [f64; 2],
    pub degrees: f64,
    pub opacity: f64,
}

impl Target {
    pub fn new(index: usize, x: f64, y: f64, random: &mut Random) -> Self {
        let particles = (0..PARTICLES)
            .map(|i| {
                let size = if i == 0 { LARGEST_PARTICLE } else { random.between(4.0, 18.0) };
                let radius = random.uniform().sqrt() * random.between(40.0, 70.0);
                let angle = random.uniform() * std::f64::consts::TAU;
                let fade_delay_s = (random.uniform() * 400.0).floor() / 1000.0;
                Particle { size, offset: [angle.cos() * radius, angle.sin() * radius], fade_delay_s }
            })
            .collect();
        Self { index, x, y, particles, spawned_at: 0.0, spawn_s: 0.0, focus: Focus::Resting, exploded_at: None }
    }

    /// Rotation and scale of the particles at `now` while not exploding.
    fn spin(&self, now: f64) -> (f64, [f64; 2]) {
        match self.focus {
            Focus::Resting => (0.0, [1.0, 1.0]),
            Focus::Focused { since } => {
                let elapsed = now - since;
                if elapsed < FOCUS_S {
                    let t = cubic_bezier(0.55, 0.085, 0.68, 0.53, elapsed / FOCUS_S);
                    (-1440.0 * (1.0 - t), [1.0 - 0.5 * t, 1.0 - 0.2 * t])
                } else {
                    let turn = ((elapsed - FOCUS_S) / SPIN_S).fract();
                    (SPIN_DEGREES * turn, [0.5, 0.8])
                }
            }
            Focus::Unfocusing { since, from: (degrees, scale) } => {
                let t = progress(now, since, UNFOCUS_S);
                let mix = |a: f64, b: f64| a + (b - a) * t;
                (mix(degrees, 0.0), [mix(scale[0], 1.0), mix(scale[1], 1.0)])
            }
        }
    }

    pub fn set_focus(&mut self, focused: bool, now: f64) {
        self.focus = if focused {
            Focus::Focused { since: now }
        } else {
            Focus::Unfocusing { since: now, from: self.spin(now) }
        };
    }

    pub fn explode(&mut self, now: f64) {
        self.exploded_at = Some(now);
    }

    fn explode_s(particle: &Particle) -> f64 {
        0.5 + 0.5 * particle.fade_delay_s
    }

    /// Whether the explosion is over and the target can go.
    pub fn gone(&self, now: f64) -> bool {
        self.exploded_at.is_some_and(|at| {
            let longest = self.particles.iter().map(|p| Self::explode_s(p).max(p.fade_delay_s + FADE_S));
            now - at > longest.fold(0.0, f64::max) + 0.05
        })
    }

    /// The particles at `now` in a window of `width` x `height`.
    pub fn draw(&self, now: f64, width: f64, height: f64) -> Vec<ParticleDraw> {
        let (px, py) = (self.x * width, self.y * height);
        let spawn = progress(now, self.spawned_at, self.spawn_s.max(1e-9));
        let travel = cubic_bezier(0.65, 0.0, 0.35, 1.0, spawn);
        let opacity = cubic_bezier(0.83, 0.0, 0.17, 1.0, spawn);
        let (dx, dy) = ((width / 2.0 - px) * (1.0 - travel), (height / 2.0 - py) * (1.0 - travel));
        let target_scale = 2.0 - travel;
        let (degrees, scale) = self.spin(now);
        self.particles
            .iter()
            .map(|particle| {
                let half = particle.size / 2.0;
                let Some(at) = self.exploded_at else {
                    return ParticleDraw {
                        centre: [px + dx, py + dy],
                        radii: [half * scale[0] * target_scale, half * scale[1] * target_scale],
                        degrees,
                        opacity,
                    };
                };
                let out = cubic_bezier(0.075, 0.82, 0.165, 1.0, progress(now, at, Self::explode_s(particle)));
                let fade = 1.0 - progress(now, at + particle.fade_delay_s, FADE_S);
                ParticleDraw {
                    centre: [px + 1.5 * particle.offset[0] * out, py + 1.5 * particle.offset[1] * out],
                    radii: [half * 0.5, half * 0.5],
                    degrees: 0.0,
                    opacity: fade,
                }
            })
            .collect()
    }
}

/// The calibration screen: its targets and overlays.
#[derive(Debug, Clone)]
pub struct CalibrationView {
    pub targets: Vec<Target>,
    random: Random,
    intro: Option<(f64, bool, bool)>,
    spinner_from: Option<f64>,
    fading_since: Option<f64>,
}

impl Default for CalibrationView {
    fn default() -> Self {
        Self { targets: Vec::new(), random: Random::new(0x5EED), intro: None, spinner_from: None, fading_since: None }
    }
}

/// The overlays at one moment: welcome and instruction text opacity, centre dot scale,
/// whether the spinner shows, and the opacity of the targets layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overlays {
    pub welcome: f64,
    pub instruction: f64,
    pub dot_scale: f64,
    pub spinner: bool,
    pub targets: f64,
}

impl CalibrationView {
    pub fn new(seed: u64) -> Self {
        Self { random: Random::new(seed), ..Self::default() }
    }

    pub fn apply(&mut self, phase: &CalibrationPhase, now: f64) {
        match phase {
            CalibrationPhase::Intro { welcome, again } => {
                self.targets.clear();
                self.spinner_from = None;
                self.fading_since = None;
                self.intro = Some((now, *welcome, *again));
            }
            CalibrationPhase::Targets { targets, .. } => {
                let duration = spawn_s(targets.len());
                for &(index, x, y) in targets {
                    let mut target = Target::new(index, x, y, &mut self.random);
                    (target.spawned_at, target.spawn_s) = (now, duration);
                    self.targets.push(target);
                }
            }
            CalibrationPhase::Focus { index, focused } => {
                if let Some(target) = self.target(*index) {
                    target.set_focus(*focused, now);
                }
            }
            CalibrationPhase::Missed { index } => {
                if let Some(target) = self.target(*index) {
                    target.set_focus(false, now);
                }
            }
            CalibrationPhase::Pop { index } => {
                if let Some(target) = self.target(*index) {
                    target.explode(now);
                }
            }
            CalibrationPhase::Computing { .. } => self.spinner_from = Some(now + SPINNER_AFTER_S),
            CalibrationPhase::Computed { .. } => self.spinner_from = None,
            CalibrationPhase::Restart { .. } => self.fading_since = Some(now),
        }
    }

    fn target(&mut self, index: usize) -> Option<&mut Target> {
        self.targets.iter_mut().find(|t| t.index == index && t.exploded_at.is_none())
    }

    /// Drops exploded targets and, after a restart's fade, every target.
    pub fn tick(&mut self, now: f64) {
        self.targets.retain(|t| !t.gone(now));
        if self.fading_since.is_some_and(|since| now - since >= RESTART_FADE_S) {
            self.targets.clear();
            self.fading_since = None;
        }
    }

    pub fn overlays(&self, now: f64) -> Overlays {
        let (mut welcome, mut instruction, mut dot_scale) = (0.0, 0.0, 0.0);
        if let Some((start, with_welcome, again)) = self.intro {
            let elapsed = now - start;
            let after_welcome = if with_welcome { WELCOME_S } else { 0.0 };
            if with_welcome {
                welcome = timeline(elapsed, 0.0, WELCOME_S, &[(0.0, 0.0), (0.2, 1.0), (0.8, 1.0), (1.0, 0.0)]);
            }
            let hold = instruction_hold_s(again);
            let total = hold + 0.5;
            let frames = [(0.0, 0.0), (0.5 / total, 1.0), (hold / total, 1.0), (1.0, 0.0)];
            if elapsed >= after_welcome {
                instruction = timeline(elapsed, after_welcome, total, &frames);
            }
            let dot_frames = [(0.0, 0.0), (0.6, 1.0), (0.99, 1.0), (1.0, 0.0)];
            dot_scale = timeline(elapsed, after_welcome + hold - 1.25, DOT_S, &dot_frames);
        }
        let targets =
            self.fading_since.map_or(1.0, |since| keyframes((now - since) / RESTART_FADE_S, &[(0.0, 1.0), (1.0, 0.0)]));
        Overlays {
            welcome,
            instruction,
            dot_scale,
            spinner: self.spinner_from.is_some_and(|from| now >= from),
            targets,
        }
    }

    pub fn particles(&self, now: f64, width: f64, height: f64) -> Vec<ParticleDraw> {
        self.targets.iter().flat_map(|t| t.draw(now, width, height)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(list: &[(usize, f64, f64)]) -> CalibrationPhase {
        CalibrationPhase::Targets { stage: 0, targets: list.to_vec() }
    }

    #[test]
    fn acknowledgements_wait_for_the_animations() {
        assert_eq!(ack_delay_s(&CalibrationPhase::Intro { welcome: true, again: false }), 3.25);
        assert_eq!(ack_delay_s(&CalibrationPhase::Intro { welcome: false, again: true }), 1.75);
        assert_eq!(ack_delay_s(&targets(&[(0, 0.5, 0.5)])), 1.5);
        assert_eq!(ack_delay_s(&targets(&[(1, 0.1, 0.9), (2, 0.5, 0.1)])), 1.0);
        assert_eq!(ack_delay_s(&CalibrationPhase::Computed { stage: 1 }), 0.7);
        assert_eq!(ack_delay_s(&CalibrationPhase::Pop { index: 0 }), 0.0);
    }

    #[test]
    fn targets_fly_out_from_the_centre_and_rest_on_their_point() {
        let mut view = CalibrationView::new(1);
        view.apply(&targets(&[(1, 0.1, 0.9)]), 0.0);
        let start = view.particles(0.0, 1000.0, 500.0);
        assert_eq!(start[0].centre, [500.0, 250.0]);
        assert_eq!(start[0].opacity, 0.0);
        let rest = view.particles(2.0, 1000.0, 500.0);
        assert!(rest.iter().all(|p| (p.centre[0] - 100.0).abs() < 1e-9 && (p.centre[1] - 450.0).abs() < 1e-9));
        assert_eq!(rest[0].radii, [9.0, 9.0]);
        assert_eq!(rest[0].opacity, 1.0);
    }

    #[test]
    fn focus_squashes_and_spins_then_the_target_explodes_and_goes() {
        let mut view = CalibrationView::new(2);
        view.apply(&targets(&[(0, 0.5, 0.5)]), 0.0);
        view.apply(&CalibrationPhase::Focus { index: 0, focused: true }, 2.0);
        let spinning = view.particles(3.5, 1000.0, 500.0)[0];
        assert_eq!(spinning.radii, [4.5, 7.2]);
        assert!((spinning.degrees - 0.25 * SPIN_DEGREES).abs() < 1e-6);
        view.apply(&CalibrationPhase::Pop { index: 0 }, 4.0);
        let flying = view.particles(4.3, 1000.0, 500.0);
        assert!(flying.iter().any(|p| (p.centre[0] - 500.0).abs() > 10.0));
        view.tick(5.0);
        assert!(view.targets.is_empty());
    }

    #[test]
    fn a_missed_target_returns_to_rest() {
        let mut view = CalibrationView::new(3);
        view.apply(&targets(&[(0, 0.5, 0.5)]), 0.0);
        view.apply(&CalibrationPhase::Focus { index: 0, focused: true }, 2.0);
        view.apply(&CalibrationPhase::Missed { index: 0 }, 3.2);
        let rest = view.particles(3.5, 1000.0, 500.0)[0];
        assert_eq!((rest.degrees, rest.radii), (0.0, [9.0, 9.0]));
    }

    #[test]
    fn intro_overlays_and_restart() {
        let mut view = CalibrationView::new(4);
        view.apply(&CalibrationPhase::Intro { welcome: true, again: false }, 0.0);
        assert_eq!(view.overlays(1.25).welcome, 1.0);
        assert_eq!(view.overlays(1.25).instruction, 0.0);
        assert_eq!(view.overlays(3.5).instruction, 1.0);
        assert!(view.overlays(3.25 + 1.0).dot_scale > 0.6);
        view.apply(&CalibrationPhase::Computing { stage: 0 }, 10.0);
        assert!(!view.overlays(10.4).spinner && view.overlays(10.6).spinner);
        view.apply(&targets(&[(0, 0.5, 0.5)]), 11.0);
        view.apply(&CalibrationPhase::Restart { attempt: 1 }, 12.0);
        assert!((view.overlays(12.1).targets - 0.5).abs() < 1e-9);
        view.tick(12.3);
        assert!(view.targets.is_empty() && view.overlays(12.3).targets == 1.0);
    }
}
