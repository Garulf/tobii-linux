//! The CPU side of Tobii's "Preview my gaze" bubble: `GazeNative_Update` (docs/gaze-preview.md,
//! "Update: the CPU trail") and the bounding box its renderer scissors to. Points are in
//! output pixels relative to the output's top-left corner, single precision like Tobii's.

use std::time::{Duration, Instant};

use tobii_control::GazeSample;

/// Texels in the colour gradient the composite looks the accumulation up in.
pub const GRADIENT_WIDTH: usize = 1536;
/// Entries in the history ring.
pub const RING: usize = 200;
/// A sample older than this at an update adds no point.
pub const STALE_AFTER: Duration = Duration::from_millis(100);

/// One trail point in output pixels; [`Point::ABSENT`] marks no point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ABSENT: Point = Point { x: -100_000.0, y: -100_000.0 };
}

/// The parameters Tobii's managed side sets (`GazeTraceVisualization`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub size: f32,
    pub strength: f32,
    pub decay: f32,
    pub filter: f32,
    pub trail: f32,
    /// Straight RGBA, 0 to 1.
    pub color: [f32; 4],
    pub background: [f32; 4],
}

impl Default for Params {
    fn default() -> Self {
        let silver = 192.0 / 255.0;
        Params {
            size: 0.075,
            strength: 0.95,
            decay: 0.95,
            filter: 0.75,
            trail: 0.7,
            color: [silver, silver, silver, 1.0],
            background: [0.0; 4],
        }
    }
}

/// The trail state of `GazeNative.dll`: the last raw sample, the smoothed previous point,
/// this frame's three new points and the 200-entry history ring.
#[derive(Debug, Clone)]
pub struct Trail {
    pub params: Params,
    bounds: [i32; 4],
    raw: Option<([f64; 2], Instant)>,
    shown: bool,
    previous: Point,
    frame: [Point; 3],
    ring: [Point; RING],
    index: usize,
    pad: f32,
}

impl Default for Trail {
    fn default() -> Self {
        Trail::new(Params::default())
    }
}

impl Trail {
    pub fn new(params: Params) -> Self {
        Trail {
            params,
            bounds: [0; 4],
            raw: None,
            shown: false,
            previous: Point::ABSENT,
            frame: [Point::ABSENT; 3],
            ring: [Point::ABSENT; RING],
            index: 0,
            pad: 0.0,
        }
    }

    /// The output's left, top, width and height in pixels (`GazeNative_SetScreenBounds`).
    pub fn set_bounds(&mut self, bounds: [i32; 4]) {
        self.bounds = bounds;
    }

    pub fn bounds(&self) -> [i32; 4] {
        self.bounds
    }

    /// `GazeNative_Show`: the first show after a hide starts a fresh trail.
    pub fn show(&mut self) {
        if !self.shown {
            self.ring = [Point::ABSENT; RING];
            self.previous = Point::ABSENT;
            self.shown = true;
        }
    }

    /// `GazeNative_Hide`.
    pub fn hide(&mut self) {
        self.shown = false;
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// `GazeNative_AddGazePoint` for a sample in normalized output coordinates; invalid
    /// samples are dropped.
    pub fn add(&mut self, sample: GazeSample, now: Instant) {
        if !sample.valid {
            return;
        }
        let [left, top, width, height] = self.bounds.map(f64::from);
        let x = (left + width * sample.x) as i32;
        let y = (top + height * sample.y) as i32;
        self.raw = Some(([f64::from(x), f64::from(y)], now));
    }

    /// One `GazeNative_Update` tick: smooths towards the last sample when it is fresher than
    /// [`STALE_AFTER`] and returns the history ring.
    pub fn update(&mut self, now: Instant) -> &[Point; RING] {
        self.frame = [Point::ABSENT; 3];
        let fresh = |at: Instant| now.saturating_duration_since(at).as_secs_f64() < STALE_AFTER.as_secs_f64();
        if let Some((raw, _)) = self.raw.filter(|&(_, at)| self.shown && fresh(at)) {
            self.step(Point { x: raw[0] as f32 - self.bounds[0] as f32, y: raw[1] as f32 - self.bounds[1] as f32 });
        }
        &self.ring
    }

    fn step(&mut self, mut p: Point) {
        let prev = self.previous;
        if prev.x > Point::ABSENT.x {
            let t = 1.0 - self.params.filter * 0.9;
            p = Point { x: (p.x - prev.x) * t + prev.x, y: (p.y - prev.y) * t + prev.y };
        }
        self.frame[0] = p;
        if prev.x > -100.0 {
            let d = Point { x: p.x - prev.x, y: p.y - prev.y };
            self.frame = [
                Point { x: d.x / 5.0 + prev.x, y: d.y / 5.0 + prev.y },
                Point { x: (d.x + d.x) / 5.0 + prev.x, y: (d.y + d.y) / 5.0 + prev.y },
                p,
            ];
        }
        self.previous = p;
        self.ring[self.index] = p;
        self.index = (self.index + 1) % RING;
    }

    /// This frame's `gaze`, `gaze1` and `gaze2`, the points the trail shader splats.
    pub fn frame(&self) -> [Point; 3] {
        self.frame
    }

    /// The renderer's scissor box `[left, top, right, bottom]` in pixels of a `width` x
    /// `height` target; its padding shrinks slowly across calls.
    pub fn bounding_box(&mut self, width: i32, height: i32) -> [i32; 4] {
        let [mut left, mut top, mut right, mut bottom] = [width, height, 0, 0];
        let aspect = if height == 0 { 1.0 } else { width as f32 / height as f32 };
        let count = (60 - (self.params.trail * -140.0) as i32).min(RING as i32);
        let mut r = width as f32 * self.params.size * 1.1;
        if r <= self.pad {
            r = (r - self.pad) * 0.02 + self.pad;
        }
        self.pad = r;
        let mut i = self.index as i32 - 1;
        for _ in 0..count.max(0) {
            if i < 0 {
                i = RING as i32 - 1;
            }
            let Point { x, y } = self.ring[i as usize];
            if x > -10_000.0 {
                let (half_width, half_height) = (r / aspect, r);
                if x - half_width < left as f32 {
                    left = (x - half_width) as i32;
                }
                if x + half_width > right as f32 {
                    right = (x + half_width) as i32;
                }
                if y - half_height < top as f32 {
                    top = (y - half_height) as i32;
                }
                if y + half_height > bottom as f32 {
                    bottom = (y + half_height) as i32;
                }
            }
            i -= 1;
        }
        [left.max(0), top.max(0), right.min(width), bottom.min(height)]
    }
}

/// Tobii's constant buffer (eleven `vec4`s, std140) for GL's bottom-left origin: `gaze.y`
/// is `(height - y) / height`. `width` and `height` are the output size the points are in.
pub fn uniforms(params: &Params, frame: [Point; 3], width: f32, height: f32) -> [[f32; 4]; 11] {
    let aspect = if height == 0.0 { 1.0 } else { width / height };
    let point = |p: Point| [p.x / width, (height - p.y) / height, 0.0, 0.0];
    let scalar = |value: f32| [value, 0.0, 0.0, 0.0];
    [
        point(frame[0]),
        point(frame[1]),
        point(frame[2]),
        scalar(aspect),
        scalar(params.decay),
        scalar(params.strength),
        scalar(params.size),
        params.color,
        scalar(params.trail),
        scalar(0.0),
        params.background,
    ]
}

/// A GL rectangle: x and y from the bottom-left corner, width and height.
pub type GlRect = [i32; 4];

/// The rectangles of one rendered frame (docs/gaze-preview.md, "Passes while shown"), in GL
/// coordinates of the device-pixel surface and of the quarter-resolution accumulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scissors {
    pub quarter_size: [i32; 2],
    /// The trail pass.
    pub quarter_box: GlRect,
    /// Cleared after the trail pass: left, right, top and bottom of the box.
    pub outside: [GlRect; 4],
    /// The composite.
    pub full_box: GlRect,
}

/// [`Scissors`] for a bounding box in output pixels on a `device_size` surface at `scale`
/// device pixels per output pixel (truncated); quarters truncate like Tobii's `n / 4`.
pub fn scissors(bounds: [i32; 4], device_size: [i32; 2], scale: f64) -> Scissors {
    let [width, height] = device_size;
    let [left, top, right, bottom] = bounds.map(|edge| (f64::from(edge) * scale) as i32);
    let rect = |x: i32, y: i32, w: i32, h: i32| [x, y, w.max(0), h.max(0)];
    let [qw, qh] = [(width / 4).max(1), (height / 4).max(1)];
    let [ql, qt, qr, qb] = [left / 4, top / 4, right / 4, bottom / 4];
    Scissors {
        quarter_size: [qw, qh],
        quarter_box: rect(ql, qh - qb, qr - ql, qb - qt),
        outside: [
            rect(0, 0, ql, qh),
            rect(qr, 0, qw - qr, qh),
            rect(ql, qh - qt, qr - ql, qt),
            rect(ql, 0, qr - ql, qh - qb),
        ],
        full_box: rect(left, height - bottom, right - left, bottom - top),
    }
}

/// Gradient 0 of `GazeNative.dll` as the piecewise-linear fit of docs/gaze-preview.md
/// ("Gradient"): white, alpha rising to 255 at texel 256, opaque to 320, back to 0 at 369.
pub fn gradient() -> Vec<[u8; 4]> {
    let alpha = |i: usize| match i {
        0..256 => 255 * i / 256,
        256..=320 => 255,
        321..369 => 255 * (369 - i) / 49,
        _ => 0,
    };
    (0..GRADIENT_WIDTH).map(|i| [255, 255, 255, alpha(i) as u8]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: [i32; 4] = [0, 0, 1920, 1080];

    fn sample(x: f64, y: f64) -> GazeSample {
        GazeSample { timestamp_us: 0, x, y, valid: true }
    }

    fn shown_trail() -> Trail {
        let mut trail = Trail::default();
        trail.set_bounds(BOUNDS);
        trail.show();
        trail
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn defaults_are_the_managed_values() {
        let params = Params::default();
        assert_eq!(
            (params.size, params.strength, params.decay, params.filter, params.trail),
            (0.075, 0.95, 0.95, 0.75, 0.7)
        );
        assert_eq!(params.color.map(|c| (c * 255.0).round() as u8), [192, 192, 192, 255]);
        assert_eq!(params.background, [0.0; 4]);
    }

    #[test]
    fn stale_points_are_ignored() {
        let t0 = Instant::now();
        let mut trail = shown_trail();
        trail.add(sample(0.5, 0.5), t0);
        let ring = *trail.update(t0 + ms(100));
        assert_eq!(ring, [Point::ABSENT; RING], "a sample 0.1 s old adds nothing");
        assert_eq!(trail.frame(), [Point::ABSENT; 3]);

        trail.add(GazeSample { valid: false, ..sample(0.25, 0.25) }, t0 + ms(100));
        assert_eq!(trail.update(t0 + ms(101))[0], Point::ABSENT, "invalid samples are dropped");

        trail.add(sample(0.5, 0.5), t0 + ms(200));
        assert_eq!(trail.update(t0 + ms(299))[0], Point { x: 960.0, y: 540.0 }, "99 ms is still fresh");

        let mut hidden = Trail::default();
        hidden.set_bounds(BOUNDS);
        hidden.add(sample(0.5, 0.5), t0);
        assert_eq!(hidden.update(t0)[0], Point::ABSENT, "nothing is added while hidden");
    }

    #[test]
    fn smoothing_matches_gaze_native() {
        let t = 1.0f32 - 0.75f32 * 0.9f32;
        assert_eq!(t, 0.325_000_05);
        let t0 = Instant::now();
        let mut trail = shown_trail();
        trail.set_bounds([100, 50, 1920, 1080]);
        trail.add(sample(0.5, 0.5), t0);
        assert_eq!(trail.update(t0)[0], Point { x: 960.0, y: 540.0 }, "the first point is not smoothed");

        trail.add(sample(0.75, 0.25), t0 + ms(12));
        let first = Point { x: (1440.0f32 - 960.0) * t + 960.0, y: (270.0f32 - 540.0) * t + 540.0 };
        assert_eq!(trail.update(t0 + ms(12))[1], first);

        let second = Point { x: (1440.0f32 - first.x) * t + first.x, y: (270.0f32 - first.y) * t + first.y };
        assert_eq!(trail.update(t0 + ms(24))[2], second, "every fresh tick smooths towards the same sample");
    }

    #[test]
    fn raw_points_truncate_like_the_managed_side() {
        let t0 = Instant::now();
        let mut trail = shown_trail();
        trail.set_bounds([-1920, 0, 1920, 1080]);
        trail.add(sample(0.0001, 0.9999), t0);
        let point = trail.update(t0)[0];
        assert_eq!(point, Point { x: 1.0, y: 1079.0 }, "(int)(-1920 + 0.192) is -1919, so x is 1");
    }

    #[test]
    fn intermediates_at_one_and_two_fifths() {
        let t0 = Instant::now();
        let mut trail = shown_trail();
        trail.add(sample(0.5, 0.5), t0);
        trail.update(t0);
        assert_eq!(trail.frame(), [Point { x: 960.0, y: 540.0 }, Point::ABSENT, Point::ABSENT]);

        trail.add(sample(0.75, 0.25), t0 + ms(12));
        trail.update(t0 + ms(12));
        let prev = Point { x: 960.0, y: 540.0 };
        let p = trail.ring[1];
        let d = Point { x: p.x - prev.x, y: p.y - prev.y };
        assert_eq!(
            trail.frame(),
            [
                Point { x: d.x / 5.0 + prev.x, y: d.y / 5.0 + prev.y },
                Point { x: (d.x + d.x) / 5.0 + prev.x, y: (d.y + d.y) / 5.0 + prev.y },
                p,
            ]
        );

        trail.update(t0 + ms(200));
        assert_eq!(trail.frame(), [Point::ABSENT; 3], "a tick without a fresh sample adds no point");
    }

    #[test]
    fn ring_wraps_at_200() {
        let t0 = Instant::now();
        let mut trail = shown_trail();
        trail.params.filter = 0.0;
        for tick in 0..=RING as u64 {
            trail.add(sample((tick as f64 + 1.5) / 1920.0, 0.5), t0 + ms(tick));
            trail.update(t0 + ms(tick));
        }
        assert_eq!(trail.ring[0].x, 201.0, "the 201st point overwrites the first");
        assert_eq!(trail.ring[1].x, 2.0);
        assert_eq!(trail.ring[RING - 1].x, 200.0);
        assert_eq!(trail.index, 1);
    }

    #[test]
    fn showing_again_starts_a_fresh_trail() {
        let t0 = Instant::now();
        let mut trail = shown_trail();
        trail.add(sample(0.5, 0.5), t0);
        trail.update(t0);
        trail.show();
        assert_eq!(trail.ring[0].x, 960.0, "showing while shown keeps the trail");
        trail.hide();
        trail.show();
        assert_eq!((trail.ring, trail.previous), ([Point::ABSENT; RING], Point::ABSENT), "Show keeps the ring index");
    }

    #[test]
    fn uniforms_flip_y_for_gl() {
        let params = Params::default();
        let frame = [Point { x: 480.0, y: 270.0 }, Point::ABSENT, Point { x: 1920.0, y: 0.0 }];
        let block = uniforms(&params, frame, 1920.0, 1080.0);
        assert_eq!(block[0], [0.25, 0.75, 0.0, 0.0]);
        assert_eq!(block[2], [1.0, 1.0, 0.0, 0.0]);
        assert!(block[1][0] < -50.0 && block[1][1] > 50.0, "an absent point stays far away: {:?}", block[1]);
        assert_eq!(block[3], [1920.0 / 1080.0, 0.0, 0.0, 0.0]);
        assert_eq!(&block[4..7], &[[0.95, 0.0, 0.0, 0.0], [0.95, 0.0, 0.0, 0.0], [0.075, 0.0, 0.0, 0.0]]);
        assert_eq!(block[7], params.color);
        assert_eq!(&block[8..], &[[0.7, 0.0, 0.0, 0.0], [0.0; 4], [0.0; 4]]);
        assert_eq!(uniforms(&params, frame, 1920.0, 0.0)[3][0], 1.0, "aspect 1 without a height");
    }

    #[test]
    fn scissors_flip_and_quarter_for_gl() {
        let plan = scissors([100, 200, 501, 403], [1920, 1080], 1.0);
        assert_eq!(plan.quarter_size, [480, 270]);
        assert_eq!(plan.quarter_box, [25, 270 - 100, 125 - 25, 100 - 50]);
        assert_eq!(
            plan.outside,
            [[0, 0, 25, 270], [125, 0, 480 - 125, 270], [25, 270 - 50, 100, 50], [25, 0, 100, 270 - 100]]
        );
        assert_eq!(plan.full_box, [100, 1080 - 403, 401, 203]);

        let hidpi = scissors([100, 200, 501, 403], [3840, 2160], 2.0);
        assert_eq!(hidpi.quarter_size, [960, 540]);
        assert_eq!(hidpi.quarter_box, [50, 540 - 201, 250 - 50, 201 - 100]);
        assert_eq!(hidpi.full_box, [200, 2160 - 806, 802, 406]);

        let empty = scissors([1920, 1080, 0, 0], [1920, 1080], 1.0);
        assert_eq!((empty.quarter_box[2], empty.quarter_box[3], empty.full_box[2]), (0, 0, 0), "nothing to draw");
        assert_eq!(empty.outside[0], [0, 0, 480, 270], "the left strip clears everything");
        assert!(empty.outside.iter().all(|rect| rect[2] >= 0 && rect[3] >= 0));
        assert_eq!(scissors([0, 0, 0, 0], [2, 3], 1.0).quarter_size, [1, 1], "at least one texel");
    }

    #[test]
    fn gradient_fits_tobii_table() {
        let table = gradient();
        assert_eq!(table.len(), GRADIENT_WIDTH);
        assert!(table.iter().all(|texel| texel[..3] == [255, 255, 255]));
        let alpha = |i: usize| table[i][3];
        assert_eq!((alpha(0), alpha(128), alpha(256), alpha(320), alpha(369), alpha(1535)), (0, 127, 255, 255, 0, 0));
        assert!((alpha(345) as i32 - 125).abs() <= 1, "falling edge: {}", alpha(345));
    }

    #[test]
    fn bounding_box_covers_the_last_158_points_padded() {
        let t0 = Instant::now();
        let mut trail = shown_trail();
        assert_eq!(trail.bounding_box(1920, 1080), [1920, 1080, 0, 0], "no points, an empty box");

        trail.add(sample(0.5, 0.5), t0);
        trail.update(t0);
        let r = 1920.0f32 * 0.075 * 1.1;
        let aspect = 1920.0f32 / 1080.0;
        let expected =
            [(960.0 - r / aspect) as i32, (540.0 - r) as i32, (960.0 + r / aspect) as i32, (540.0 + r) as i32];
        assert_eq!(trail.bounding_box(1920, 1080), expected);

        let shrunk = (r * 0.5 - r) * 0.02 + r;
        let small = trail.bounding_box(960, 1080);
        assert_eq!(small[1], (540.0 - shrunk) as i32, "the pad shrinks by 2 % of the difference");
        assert_eq!(small[2], 960, "clamped at the right edge");

        trail.ring = [Point::ABSENT; RING];
        trail.ring[RING - 1] = Point { x: 100.0, y: 100.0 };
        trail.ring[RING - 1 - 158] = Point { x: 1800.0, y: 900.0 };
        trail.index = 0;
        let edge = trail.bounding_box(1920, 1080);
        assert!(edge[2] < 1800, "only 158 entries back from the index count: {edge:?}");
    }
}
