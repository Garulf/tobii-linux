//! Where the user's eyes are in the track box, and the hint the positioning screen shows.
//!
//! A port of Tobii's EyesPositioningParametersCalculator and the hint and auto-advance
//! rules of EyesPositioningViewModel (Tobii.Configuration.Common, see docs/setup-flow.md).
//! Input is the device's user position guide (gaze columns 37 to 40): per eye a position
//! normalized to the track box, x and y in 0..1 across, z in 0..1 from near to far.
//!
//! On screen the picture is mirrored like a mirror: x becomes 1 - x. Each eye is drawn with
//! a size from the distance (50 at a good distance, up to 100 when too close, down to 25
//! when too far) and a grey level and opacity from that size and from how near the eye is
//! to the edge of the box.

use std::collections::VecDeque;

use tobii_protocol::gaze::DeviceGaze;

use crate::math::{py_max, py_min, sum};

pub const MIN_DISTANCE: f64 = 0.2;
pub const MAX_DISTANCE: f64 = 0.8;
pub const FAR_EDGE: f64 = 0.65;
pub const CLOSE_EDGE: f64 = 0.4;
pub const DEFAULT_EYE_SIZE: i64 = 50;
pub const SMALL_EYE_SIZE: i64 = 25;
pub const BIG_EYE_SIZE: i64 = 100;
pub const FAR_SLOPE: f64 = 50.0 / 0.3;
pub const CLOSE_SLOPE: f64 = 250.0;
pub const EDGE_MIN: f64 = 0.1;
pub const EDGE_MAX: f64 = 0.9;
pub const EXTRAPOLATION_SAMPLES: usize = 11;
pub const DISTANCE_SAMPLES: usize = 3;
pub const FADE_MARGIN: f64 = 0.2;
pub const DARK_GREY: i64 = 50;

pub const HINT_DELAY_FROM_VALID: u32 = 11;
pub const LOST_DELAY_FRAMES: u32 = 49;
pub const ARE_YOU_THERE_MS: f64 = 2000.0;
pub const ADJUST_SEATING_MS: f64 = 5000.0;
pub const AUTO_ADVANCE_AFTER_MS: f64 = 6000.0;
pub const PRESENCE_NEEDED_MS: f64 = 3000.0;
pub const ABSENCE_GRACE_MS: f64 = 500.0;

pub const ARE_YOU_THERE: &str = "Are you there?";
pub const ADJUST_SEATING: &str = "Adjust your seating position";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    CannotTrackEyes,
    MoveCloser,
    LeanBack,
    MoveRight,
    MoveLeft,
    MoveDown,
    MoveUp,
    Valid,
}

impl Status {
    pub const ALL: [Status; 8] = [
        Status::CannotTrackEyes,
        Status::MoveCloser,
        Status::LeanBack,
        Status::MoveRight,
        Status::MoveLeft,
        Status::MoveDown,
        Status::MoveUp,
        Status::Valid,
    ];

    /// The value the setup page receives.
    pub fn value(self) -> &'static str {
        match self {
            Status::CannotTrackEyes => "cannot-track-eyes",
            Status::MoveCloser => "move-closer",
            Status::LeanBack => "lean-back",
            Status::MoveRight => "move-right",
            Status::MoveLeft => "move-left",
            Status::MoveDown => "move-down",
            Status::MoveUp => "move-up",
            Status::Valid => "valid",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Status::MoveCloser => "Move closer",
            Status::LeanBack => "Lean back",
            Status::MoveRight => "Move right",
            Status::MoveLeft => "Move left",
            Status::MoveDown => "Move down",
            Status::MoveUp => "Move up",
            Status::Valid | Status::CannotTrackEyes => "",
        }
    }
}

/// One user position guide record: device time and each eye's normalized (x, y, z), `None`
/// when not valid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideSample {
    pub timestamp_us: i64,
    pub left: Option<[f64; 3]>,
    pub right: Option<[f64; 3]>,
}

/// GuideSample from a device gaze record (validity column 38/40 equal to 1 marks a valid eye).
pub fn guide_sample(g: &DeviceGaze) -> GuideSample {
    GuideSample {
        timestamp_us: g.timestamp,
        left: (g.left_position_guide_validity == 1).then_some(g.left_position_guide),
        right: (g.right_position_guide_validity == 1).then_some(g.right_position_guide),
    }
}

pub fn mirrored(position: [f64; 3]) -> [f64; 2] {
    [1.0 - position[0], position[1]]
}

pub fn eye_size(distance: f64) -> i64 {
    if distance <= MIN_DISTANCE {
        return BIG_EYE_SIZE;
    }
    if distance >= MAX_DISTANCE {
        return SMALL_EYE_SIZE;
    }
    if distance > FAR_EDGE {
        return (DEFAULT_EYE_SIZE as f64 - FAR_SLOPE * (distance - FAR_EDGE)).ceil() as i64;
    }
    if distance < CLOSE_EDGE {
        return (DEFAULT_EYE_SIZE as f64 + CLOSE_SLOPE * (CLOSE_EDGE - distance)).ceil() as i64;
    }
    DEFAULT_EYE_SIZE
}

/// EyeSizeToEyeColor before darkening: white at size 50, dark grey at 25, 100 or no size.
pub fn eye_grey(size: i64) -> i64 {
    if [0, BIG_EYE_SIZE, SMALL_EYE_SIZE].contains(&size) {
        return DARK_GREY;
    }
    if size == DEFAULT_EYE_SIZE {
        return 255;
    }
    let slope = 205.0 / if size < DEFAULT_EYE_SIZE { SMALL_EYE_SIZE as f64 } else { -(DEFAULT_EYE_SIZE as f64) };
    (size as f64 * slope + 255.0 - slope * DEFAULT_EYE_SIZE as f64).trunc() as i64 & 0xFF
}

/// DarkenEyeColor: opaque inside the box, fading out over the last 0.2 towards an edge.
pub fn eye_opacity(position: Option<[f64; 2]>) -> f64 {
    let Some([x, y]) = position else {
        return 1.0;
    };
    let x = if x < EDGE_MIN {
        0.0
    } else if x > EDGE_MAX {
        1.0
    } else {
        (x - EDGE_MIN) / (EDGE_MAX - EDGE_MIN)
    };
    let margin = py_min(py_min(py_min(x, 1.0 - x), y), 1.0 - y);
    if margin <= 0.0 || margin >= 1.0 {
        return 0.0;
    }
    if margin >= FADE_MARGIN { 1.0 } else { margin / FADE_MARGIN }
}

/// The newest position, or a straight-line guess from the last two seen when the newest is missing.
pub fn extrapolated(history: &[Option<[f64; 2]>]) -> Option<[f64; 2]> {
    let newest = history.last()?;
    if newest.is_some() {
        return *newest;
    }
    let seen: Vec<(usize, [f64; 2])> = history.iter().enumerate().filter_map(|(i, p)| Some((i, (*p)?))).collect();
    match seen.as_slice() {
        [] => None,
        [(_, only)] => Some(*only),
        [.., (i2, p2), (i1, p1)] => {
            let steps = (history.len() - 1 - i1) as f64 / (i1 - i2) as f64;
            Some(std::array::from_fn(|axis| {
                let guess = p1[axis] + (p1[axis] - p2[axis]) * steps;
                py_min(1.0, py_max(0.0, guess))
            }))
        }
    }
}

/// One eye as the positioning page draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeView {
    pub x: f64,
    pub y: f64,
    pub grey: i64,
    pub opacity: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyesView {
    pub left: Option<EyeView>,
    pub right: Option<EyeView>,
    /// Eye size in percent.
    pub size: i64,
}

/// Whether an eye's mirrored position is at an edge of the track box.
type EdgeTest = fn(&[f64; 2]) -> bool;

fn push_bounded<T>(history: &mut VecDeque<T>, item: T, bound: usize) {
    history.push_back(item);
    while history.len() > bound {
        history.pop_front();
    }
}

/// EyesPositioningParametersCalculator: eye points, size and status from guide samples.
#[derive(Debug, Clone, PartialEq)]
pub struct EyesPositioning {
    pub left_history: VecDeque<Option<[f64; 2]>>,
    pub right_history: VecDeque<Option<[f64; 2]>>,
    pub distances: VecDeque<f64>,
    pub left: Option<[f64; 2]>,
    pub right: Option<[f64; 2]>,
    pub size: i64,
    pub status: Status,
}

impl Default for EyesPositioning {
    fn default() -> Self {
        Self {
            left_history: VecDeque::new(),
            right_history: VecDeque::new(),
            distances: VecDeque::new(),
            left: None,
            right: None,
            size: 0,
            status: Status::CannotTrackEyes,
        }
    }
}

impl EyesPositioning {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, sample: &GuideSample) -> Status {
        push_bounded(&mut self.left_history, sample.left.map(mirrored), EXTRAPOLATION_SAMPLES);
        push_bounded(&mut self.right_history, sample.right.map(mirrored), EXTRAPOLATION_SAMPLES);
        self.left = extrapolated(self.left_history.make_contiguous());
        self.right = extrapolated(self.right_history.make_contiguous());
        let depths: Vec<f64> = [sample.left, sample.right].iter().flatten().map(|eye| eye[2]).collect();
        if !depths.is_empty() {
            push_bounded(&mut self.distances, sum(depths.iter().copied()) / depths.len() as f64, DISTANCE_SAMPLES);
            self.size = eye_size(sum(self.distances.iter().copied()) / self.distances.len() as f64);
        }
        self.status = self.current_status();
        self.status
    }

    fn current_status(&self) -> Status {
        let eyes: Vec<[f64; 2]> = [self.left, self.right].into_iter().flatten().collect();
        if eyes.is_empty() {
            return Status::CannotTrackEyes;
        }
        if self.size <= SMALL_EYE_SIZE {
            return Status::MoveCloser;
        }
        if self.size >= BIG_EYE_SIZE {
            return Status::LeanBack;
        }
        let rules: [(EdgeTest, Status); 4] = [
            (|e| e[0] <= EDGE_MIN, Status::MoveRight),
            (|e| e[0] >= EDGE_MAX, Status::MoveLeft),
            (|e| e[1] <= EDGE_MIN, Status::MoveDown),
            (|e| e[1] >= EDGE_MAX, Status::MoveUp),
        ];
        rules.iter().find(|(test, _)| eyes.iter().any(test)).map_or(Status::Valid, |(_, status)| *status)
    }

    /// What the page draws: per eye its mirrored point, grey level and opacity; size in percent.
    pub fn eyes(&self) -> EyesView {
        let eye = |point: Option<[f64; 2]>| {
            point.map(|[x, y]| EyeView { x, y, grey: eye_grey(self.size), opacity: eye_opacity(point) })
        };
        EyesView { left: eye(self.left), right: eye(self.right), size: self.size }
    }
}

/// EyesPositioningViewModel's hint and advance rules on top of [`EyesPositioning`].
///
/// A hint for a new problem appears at once, except that after a good position a problem
/// has to last 11 samples (49 for lost eyes) before it replaces the empty hint, so a blink
/// or a short glance does not flash a message. When the eyes stay lost, "Are you there?"
/// follows after 2 s and "Adjust your seating position" after 5 s. The screen advances by
/// itself once it has been up 6 s, the user has been present for more than 3 s in a row
/// (gaps under 0.5 s do not count as absence) and the position is good.
#[derive(Debug, Clone, PartialEq)]
pub struct PositioningGuide {
    pub calculator: EyesPositioning,
    pub hint: &'static str,
    pub previous: Status,
    pub pending: u32,
    pub lost_since_us: Option<i64>,
    pub start_us: Option<i64>,
    pub present_since_us: Option<i64>,
    pub absent_since_us: Option<i64>,
}

impl Default for PositioningGuide {
    fn default() -> Self {
        Self {
            calculator: EyesPositioning::new(),
            hint: "",
            previous: Status::CannotTrackEyes,
            pending: 0,
            lost_since_us: None,
            start_us: None,
            present_since_us: None,
            absent_since_us: None,
        }
    }
}

fn elapsed_ms(now: i64, since: i64) -> f64 {
    (now - since) as f64 / 1000.0
}

impl PositioningGuide {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn good(&self) -> bool {
        self.previous == Status::Valid
    }

    pub fn update(&mut self, sample: &GuideSample) -> &'static str {
        let status = self.calculator.update(sample);
        let now = sample.timestamp_us;
        self.start_us.get_or_insert(now);
        self.track_presence(sample, now);
        match status {
            Status::Valid => self.settle(status),
            Status::CannotTrackEyes => self.eyes_lost(now),
            _ => {
                self.lost_since_us = None;
                if self.previous == Status::Valid {
                    self.after_frames(status, HINT_DELAY_FROM_VALID);
                } else {
                    self.settle(status);
                }
            }
        }
        self.hint
    }

    fn settle(&mut self, status: Status) {
        self.previous = status;
        self.pending = 0;
        self.lost_since_us = None;
        self.hint = status.hint();
    }

    fn after_frames(&mut self, status: Status, frames: u32) {
        self.pending += 1;
        if self.pending >= frames {
            self.settle(status);
        }
    }

    fn eyes_lost(&mut self, now: i64) {
        if self.previous != Status::CannotTrackEyes {
            self.after_frames(Status::CannotTrackEyes, LOST_DELAY_FRAMES);
            if self.previous == Status::CannotTrackEyes {
                self.lost_since_us = Some(now);
            }
            return;
        }
        let since = *self.lost_since_us.get_or_insert(now);
        let lost_ms = elapsed_ms(now, since);
        self.hint = if lost_ms >= ADJUST_SEATING_MS {
            ADJUST_SEATING
        } else if lost_ms >= ARE_YOU_THERE_MS {
            ARE_YOU_THERE
        } else {
            ""
        };
    }

    fn track_presence(&mut self, sample: &GuideSample, now: i64) {
        if sample.left.is_some() || sample.right.is_some() {
            self.absent_since_us = None;
            self.present_since_us.get_or_insert(now);
        } else if self.present_since_us.is_some() {
            match self.absent_since_us {
                None => self.absent_since_us = Some(now),
                Some(absent) if elapsed_ms(now, absent) > ABSENCE_GRACE_MS => self.present_since_us = None,
                Some(_) => {}
            }
        }
    }

    pub fn presence_ms(&self, now: i64) -> f64 {
        self.present_since_us.map_or(0.0, |since| elapsed_ms(now, since))
    }

    /// Tobii's automatic advance: shown for 6 s, present for over 3 s, and in a good position.
    pub fn ready_to_advance(&self, now: i64) -> bool {
        let Some(start) = self.start_us else {
            return false;
        };
        elapsed_ms(now, start) >= AUTO_ADVANCE_AFTER_MS && self.presence_ms(now) > PRESENCE_NEEDED_MS && self.good()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_US: i64 = 30_000;

    fn samples(count: i64, left: Option<[f64; 3]>, right: Option<[f64; 3]>, start_us: i64) -> Vec<GuideSample> {
        (0..count).map(|i| GuideSample { timestamp_us: start_us + i * FRAME_US, left, right }).collect()
    }

    fn good(count: i64, start_us: i64) -> Vec<GuideSample> {
        samples(count, Some([0.55, 0.5, 0.5]), Some([0.45, 0.5, 0.5]), start_us)
    }

    fn feed(guide: &mut PositioningGuide, items: &[GuideSample]) {
        for item in items {
            guide.update(item);
        }
    }

    #[test]
    fn eye_size_follows_the_distance_bands() {
        assert_eq!([0.1, 0.3, 0.5, 0.7, 0.9].map(eye_size), [100, 75, 50, 42, 25]);
    }

    #[test]
    fn status_order_distance_before_sides() {
        let first = |left: [f64; 3]| {
            EyesPositioning::new().update(&GuideSample { timestamp_us: 0, left: Some(left), right: None })
        };
        assert_eq!(first([0.95, 0.5, 0.9]), Status::MoveCloser);
        assert_eq!(first([0.95, 0.5, 0.5]), Status::MoveRight);
        let mut calculator = EyesPositioning::new();
        calculator.update(&GuideSample { timestamp_us: 0, left: Some([0.5, 0.05, 0.5]), right: None });
        assert_eq!(calculator.update(&GuideSample { timestamp_us: 1, left: None, right: None }), Status::MoveDown);
    }

    #[test]
    fn extrapolation_grey_and_opacity() {
        let guess = extrapolated(&[Some([0.2, 0.5]), Some([0.3, 0.5]), None]).unwrap();
        assert!((guess[0] - 0.4).abs() < 1e-12 && (guess[1] - 0.5).abs() < 1e-12);
        assert_eq!(extrapolated(&[None, None]), None);
        assert_eq!(extrapolated(&[Some([0.9, 0.5]), Some([1.0, 0.5]), None]), Some([1.0, 0.5]));
        assert_eq!([0, 25, 50, 75, 100].map(eye_grey), [50, 50, 255, 152, 50]);
        assert_eq!(eye_opacity(Some([0.5, 0.5])), 1.0);
        assert!((eye_opacity(Some([0.5, 0.1])) - 0.5).abs() < 1e-12);
        assert_eq!(eye_opacity(Some([0.05, 0.5])), 0.0);
    }

    #[test]
    fn after_a_good_position_a_problem_needs_eleven_samples() {
        let mut guide = PositioningGuide::new();
        feed(&mut guide, &good(5, 0));
        assert!(guide.good());
        let too_close = samples(13, Some([0.5, 0.5, 0.1]), Some([0.4, 0.5, 0.1]), 1_000_000);
        feed(&mut guide, &too_close[..12]);
        assert_eq!((guide.hint, guide.calculator.status), ("", Status::LeanBack));
        feed(&mut guide, &too_close[12..]);
        assert_eq!(guide.hint, "Lean back");
    }

    #[test]
    fn lost_eyes_ask_are_you_there_then_to_adjust() {
        let mut guide = PositioningGuide::new();
        feed(&mut guide, &samples(1, None, None, 0));
        assert_eq!(guide.hint, "");
        feed(&mut guide, &samples(80, None, None, FRAME_US));
        assert_eq!(guide.hint, ARE_YOU_THERE);
        feed(&mut guide, &samples(110, None, None, 81 * FRAME_US));
        assert_eq!(guide.hint, ADJUST_SEATING);
    }

    #[test]
    fn advances_after_six_seconds_with_three_seconds_of_presence() {
        let mut guide = PositioningGuide::new();
        feed(&mut guide, &samples(101, None, None, 0));
        let present = good(120, 101 * FRAME_US);
        feed(&mut guide, &present[..100]);
        assert!(!guide.ready_to_advance(present[99].timestamp_us));
        feed(&mut guide, &present[100..]);
        assert!(guide.ready_to_advance(present[119].timestamp_us));
    }

    #[test]
    fn short_absence_keeps_presence() {
        let mut guide = PositioningGuide::new();
        feed(&mut guide, &good(150, 0));
        feed(&mut guide, &samples(10, None, None, 150 * FRAME_US));
        assert!(guide.presence_ms(160 * FRAME_US) > 3000.0);
        feed(&mut guide, &samples(20, None, None, 160 * FRAME_US));
        assert_eq!(guide.presence_ms(180 * FRAME_US), 0.0);
    }
}
