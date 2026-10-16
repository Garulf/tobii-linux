//! Recovered landmark smoothing of the per-object tracker step 0x14004a530.
//!
//! After VNNTrackerCore (0x140049450) writes frame-normalised FDP records with Y pointing
//! up, 0x14007f5a0 smooths every coordinate listed in a group table with the history filter
//! 0x14004c480 and then every present, non-negative quality with 0x14004bf60. Both keep a
//! 31-sample ring per channel; channels are assigned in call order through the filter
//! cursor, which restarts each frame.
//!
//! 0x14004c480 averages three polynomial fits of the samples inside a 310 ms window
//! (degrees 0, 1 and 4). Each fit predicts the newest sample from the first row of the
//! pseudo-inverse of an index Vandermonde matrix and is weighted by
//! `exp(-k * gain) / (channels * residual + 1e-8)`, where the residual comes from a
//! least-squares fit in time (milliseconds / 30) and k is the fit's position.
//!
//! The original solves both systems with OpenCV's float32 SVD. These are solved to
//! (nearly) exact float32 results instead, with double-double arithmetic, so coordinates
//! match the original within a few float32 ulps; the scalar quality filter is exact.

use std::collections::BTreeMap;

use crate::error::{Result, invalid};
use crate::model::FdpId;

pub const HISTORY: usize = 31;
pub const WINDOW_MS: i64 = 310;
pub const MINIMUM_SAMPLES: usize = 5;
pub const CLEARED_TIME: i64 = -10_000_000;
pub const DEGREES: [usize; 3] = [0, 1, 4];
pub const TIME_SCALE: f32 = 0.03333333507180214;
pub const RESIDUAL_REGULARIZER: f32 = 1e-8;
pub const QUALITY_GAIN: f32 = 0.2;
pub const SHARPNESS_FLOOR: f32 = 0.2;
pub const SCALAR_DEFAULT_LOW: f32 = 1_000_000.0;
pub const SCALAR_GAIN_FLOOR: f32 = 0.1;
pub const SCALAR_DECAY: f32 = 30.0;
/// `(gain, low_floor, high_cap, initial_high)` of the quality filter.
pub const QUALITY_SMOOTHING: [f32; 4] = [0.5, 0.05, 1.0, 0.5];
pub const UNSCALED_GROUP: u32 = 15;
pub const CHANNELS: usize = 2000;

/// `(group, record count)` of every FDP group the runtime allocates.
pub const GROUP_SIZES: [(u32, u32); 14] = [
    (2, 14),
    (3, 14),
    (4, 6),
    (5, 4),
    (6, 4),
    (7, 1),
    (8, 10),
    (9, 15),
    (10, 24),
    (11, 6),
    (12, 47),
    (13, 17),
    (14, 26),
    (15, 17),
];

/// The 0x14004a530 table: `(FDP group, first index, last index, smoothing_factors column)`.
pub const TRACKER_TABLE: [(u32, u32, u32, usize); 13] = [
    (4, 1, 6, 0),
    (14, 1, 4, 0),
    (8, 1, 10, 1),
    (2, 2, 9, 1),
    (3, 5, 6, 2),
    (3, 1, 4, 3),
    (3, 7, 14, 3),
    (12, 5, 12, 3),
    (9, 3, 5, 4),
    (9, 15, 15, 4),
    (14, 22, 25, 4),
    (2, 1, 1, 5),
    (15, 1, 17, 5),
];
pub const EAR_TABLE: [(u32, u32, u32, usize); 1] = [(10, 1, 24, 7)];

/// A table entry with its configured factor: `(group, first index, last index, factor)`.
pub type TableEntry = (u32, u32, u32, f32);

/// The fields of an FDP record the smoothing reads and writes (x +0, y +4, present +0xc,
/// quality +0x14).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LandmarkRecord {
    pub x: f32,
    pub y: f32,
    pub present: bool,
    pub quality: f32,
}

impl Default for LandmarkRecord {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, present: false, quality: -1.0 }
    }
}

pub type Records = BTreeMap<FdpId, LandmarkRecord>;

/// MAXSS first, second: the second operand when either is NaN.
fn maxss(first: f32, second: f32) -> f32 {
    if first > second { first } else { second }
}

fn minss(first: f32, second: f32) -> f32 {
    if first < second { first } else { second }
}

fn expf(value: f32) -> f32 {
    f64::from(value).exp() as f32
}

/// The history filter object built by 0x14004bcc0 (0x48 bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct LandmarkFilter {
    pub channels: usize,
    pub history: usize,
    pub enabled: bool,
    pub cursor: usize,
    pub frames: i64,
    /// `channels x history` ring samples.
    pub values: Vec<f32>,
    /// `channels x history` sample times in milliseconds.
    pub times: Vec<i64>,
    pub position: Vec<usize>,
    pub initialised: Vec<bool>,
    pub high: Vec<f32>,
    pub low: Vec<f32>,
}

impl Default for LandmarkFilter {
    fn default() -> Self {
        Self::new(CHANNELS, HISTORY)
    }
}

impl LandmarkFilter {
    pub fn new(channels: usize, history: usize) -> Self {
        let mut filter = Self {
            channels,
            history,
            enabled: true,
            cursor: 0,
            frames: 0,
            values: vec![0.0; channels * history],
            times: vec![0; channels * history],
            position: vec![0; channels],
            initialised: vec![false; channels],
            high: vec![0.0; channels],
            low: vec![0.0; channels],
        };
        filter.reset();
        filter
    }

    /// 0x14004bed0: rewinds every ring, forgets initialisation and clears all sample times.
    pub fn reset(&mut self) {
        self.position.fill(0);
        self.initialised.fill(false);
        self.frames = 0;
        self.enabled = true;
        self.times.fill(CLEARED_TIME);
    }

    /// 0x14004a530 bookkeeping before any channel is used.
    pub fn begin_frame(&mut self) {
        self.frames += 1;
        self.cursor = 0;
    }

    pub fn window(&self) -> i64 {
        WINDOW_MS.max(self.history as i64)
    }

    fn channel(&self) -> Result<usize> {
        if self.cursor >= self.channels {
            return invalid("Landmark filter channels exhausted");
        }
        Ok(self.cursor)
    }

    fn ring(&self, channel: usize, slot: usize) -> usize {
        channel * self.history + slot
    }

    fn store(&mut self, channel: usize, value: f32, time: i64) -> usize {
        let slot = self.position[channel];
        let at = self.ring(channel, slot);
        self.values[at] = value;
        self.times[at] = time;
        slot
    }

    fn advance(&mut self, channel: usize, slot: usize) {
        self.position[channel] = (slot + 1) % self.history;
        self.cursor += 1;
    }

    /// 0x14004c480: smooths one coordinate per channel and returns the new values.
    ///
    /// Coordinates with a negative gain are skipped and use no channel. Fewer than
    /// [`MINIMUM_SAMPLES`] samples inside the window leave every value unchanged.
    pub fn smooth_coordinates(&mut self, values: &[f32], gains: &[f32], time: i64) -> Result<Vec<f32>> {
        let mut result = values.to_vec();
        let kept: Vec<usize> = (0..values.len()).filter(|&i| !(gains[i] < 0.0)).collect();
        if !self.enabled || kept.is_empty() {
            return Ok(result);
        }
        let (length, window) = (self.history, self.window());
        let mut samples = vec![0.0f32; length * kept.len()];
        let mut sample_times = vec![0.0f32; length];
        let mut count = 0;
        for (column, &index) in kept.iter().enumerate() {
            let channel = self.channel()?;
            let slot = self.store(channel, result[index], time);
            for age in 0..length {
                let ring = self.ring(channel, (length - age + slot) % length);
                if time - self.times[ring] < window {
                    samples[(length - age - 1) * kept.len() + column] = self.values[ring];
                    if column == 0 {
                        sample_times[length - age - 1] = self.times[ring] as f64 as f32;
                        count += 1;
                    }
                }
            }
            self.advance(channel, slot);
        }
        if count < MINIMUM_SAMPLES {
            return Ok(result);
        }
        let recent = &samples[(length - count) * kept.len()..];
        let gains: Vec<f32> = kept.iter().map(|&i| gains[i]).collect();
        let smoothed = polynomial_blend(recent, kept.len(), &sample_times[length - count..], &gains);
        for (column, &index) in kept.iter().enumerate() {
            result[index] = smoothed[column];
        }
        Ok(result)
    }

    /// 0x14004bf60: adaptive exponential average of one value; returns the new value.
    pub fn smooth_scalar(
        &mut self,
        value: f32,
        time: i64,
        gain: f32,
        low_floor: f32,
        high_cap: f32,
        initial_high: f32,
    ) -> Result<f32> {
        if !self.enabled || !(gain >= 0.0) {
            return Ok(value);
        }
        let channel = self.channel()?;
        let (length, window) = (self.history, self.window());
        let slot = self.position[channel];
        if !self.initialised[channel] {
            if time > window {
                self.initialised[channel] = true;
            } else {
                self.high[channel] = initial_high;
                self.low[channel] = SCALAR_DEFAULT_LOW;
            }
        }
        self.store(channel, value, time);
        let initialised = self.initialised[channel];
        let base = channel * self.history;
        let sample = |age: usize| base + (length + slot - age) % length;
        let count =
            if initialised { (0..length).filter(|&age| time - self.times[sample(age)] < window).count() } else { 0 };
        let mut value = value;
        if count > 0 {
            let mut largest = 0.0f32;
            for age in 1..count {
                let difference = (value - self.values[sample(age)]).abs();
                if difference < self.low[channel] {
                    self.low[channel] = if difference >= low_floor { difference } else { low_floor };
                }
                if difference > self.high[channel] {
                    self.high[channel] = if high_cap >= difference { difference } else { high_cap };
                }
                largest = maxss(difference, largest);
            }
            let floor = maxss(gain, SCALAR_GAIN_FLOOR);
            let (high, low) = (self.high[channel], self.low[channel]);
            let spread = maxss(minss(floor * largest, high), low);
            let rate = 1000 / (window / count as i64);
            let sharpness = ((spread - low) / (high - low)) * 2.0 + SHARPNESS_FLOOR;
            let step = SCALAR_DECAY / rate as f64 as f32;
            let (mut weight_sum, mut total) = (0.0f32, 0.0f32);
            for offset in -(count as i64 - 1)..=0 {
                let weight = expf((offset as f64 as f32 * step) * sharpness);
                weight_sum += weight;
                let at = self.ring(channel, ((length + slot) as i64 + offset) as usize % length);
                total += weight * self.values[at];
            }
            value = total / weight_sum;
        }
        self.advance(channel, slot);
        Ok(value)
    }
}

/// Double-double arithmetic (Dekker), enough to solve the small least-squares systems to
/// a correctly rounded float32 result.
mod dd {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Dd {
        pub hi: f64,
        pub lo: f64,
    }

    fn two_sum(a: f64, b: f64) -> (f64, f64) {
        let s = a + b;
        let v = s - a;
        (s, (a - (s - v)) + (b - v))
    }

    fn quick_two_sum(a: f64, b: f64) -> Dd {
        let s = a + b;
        Dd { hi: s, lo: b - (s - a) }
    }

    fn split(a: f64) -> (f64, f64) {
        let c = 134_217_729.0 * a;
        let hi = c - (c - a);
        (hi, a - hi)
    }

    fn two_product(a: f64, b: f64) -> (f64, f64) {
        let p = a * b;
        let ((ah, al), (bh, bl)) = (split(a), split(b));
        (p, ((ah * bh - p) + ah * bl + al * bh) + al * bl)
    }

    impl Dd {
        pub const ZERO: Self = Self { hi: 0.0, lo: 0.0 };

        pub fn from(value: f64) -> Self {
            Self { hi: value, lo: 0.0 }
        }

        pub fn add(self, other: Self) -> Self {
            let (s, e) = two_sum(self.hi, other.hi);
            let (t, f) = two_sum(self.lo, other.lo);
            let r = quick_two_sum(s, e + t);
            quick_two_sum(r.hi, r.lo + f)
        }

        pub fn neg(self) -> Self {
            Self { hi: -self.hi, lo: -self.lo }
        }

        pub fn sub(self, other: Self) -> Self {
            self.add(other.neg())
        }

        pub fn mul(self, other: Self) -> Self {
            let (p, e) = two_product(self.hi, other.hi);
            quick_two_sum(p, e + (self.hi * other.lo + self.lo * other.hi))
        }

        pub fn div(self, other: Self) -> Self {
            let q1 = self.hi / other.hi;
            let r = self.sub(other.mul(Self::from(q1)));
            let q2 = r.hi / other.hi;
            let r = r.sub(other.mul(Self::from(q2)));
            let q3 = r.hi / other.hi;
            quick_two_sum(q1, q2).add(Self::from(q3))
        }

        /// The sum `hi + lo` rounded once to float32.
        pub fn to_f32(self) -> f32 {
            let rounded = self.hi as f32;
            if !rounded.is_finite() || self.hi == 0.0 {
                return rounded;
            }
            let remainder = (self.hi - f64::from(rounded)) + self.lo;
            let (up, down) = (rounded.next_up(), rounded.next_down());
            let (half_up, half_down) =
                ((f64::from(up) - f64::from(rounded)) / 2.0, (f64::from(rounded) - f64::from(down)) / 2.0);
            let even = |a: f32, b: f32| if a.to_bits().is_multiple_of(2) { a } else { b };
            if remainder > half_up {
                up
            } else if remainder < -half_down {
                down
            } else if remainder == half_up {
                even(rounded, up)
            } else if remainder == -half_down {
                even(rounded, down)
            } else {
                rounded
            }
        }
    }
}

use dd::Dd;

/// Solves the symmetric normal equations `matrix @ x = rhs` by Gaussian elimination with
/// partial pivoting; a column without a nonzero pivot gets a zero coefficient.
fn solve(mut matrix: Vec<Vec<Dd>>, mut rhs: Vec<Dd>) -> Vec<Dd> {
    let n = rhs.len();
    let mut pivots = vec![None; n];
    let mut row = 0;
    for column in 0..n {
        let Some(best) = (row..n)
            .filter(|&r| matrix[r][column].hi != 0.0)
            .max_by(|&a, &b| matrix[a][column].hi.abs().total_cmp(&matrix[b][column].hi.abs()))
        else {
            continue;
        };
        matrix.swap(row, best);
        rhs.swap(row, best);
        let (pivot_row, pivot_rhs) = (matrix[row].clone(), rhs[row]);
        for r in row + 1..n {
            let factor = matrix[r][column].div(pivot_row[column]);
            for (value, pivot) in matrix[r].iter_mut().zip(&pivot_row).skip(column) {
                *value = value.sub(factor.mul(*pivot));
            }
            rhs[r] = rhs[r].sub(factor.mul(pivot_rhs));
        }
        pivots[column] = Some(row);
        row += 1;
    }
    let mut solution = vec![Dd::ZERO; n];
    for column in (0..n).rev() {
        let Some(r) = pivots[column] else { continue };
        let mut value = rhs[r];
        for c in column + 1..n {
            value = value.sub(matrix[r][c].mul(solution[c]));
        }
        solution[column] = value.div(matrix[r][column]);
    }
    solution
}

/// Least-squares coefficients of `design` (`rows x n`, row-major) for each column of
/// `samples` (`rows x channels`), rounded to float32: `n x channels`.
fn least_squares(design: &[f64], n: usize, samples: &[f32], channels: usize) -> Vec<f32> {
    let rows = design.len() / n;
    let product = |a: f64, b: f64| Dd::from(a).mul(Dd::from(b));
    let normal: Vec<Vec<Dd>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| (0..rows).fold(Dd::ZERO, |t, k| t.add(product(design[k * n + i], design[k * n + j]))))
                .collect()
        })
        .collect();
    let mut result = vec![0.0; n * channels];
    for c in 0..channels {
        let rhs = (0..n)
            .map(|i| {
                (0..rows).fold(Dd::ZERO, |t, k| t.add(product(design[k * n + i], f64::from(samples[k * channels + c]))))
            })
            .collect();
        for (i, value) in solve(normal.clone(), rhs).iter().enumerate() {
            result[i * channels + c] = value.to_f32();
        }
    }
    result
}

/// The first row of `pinv(V)` for `V[i, j] = (i - count + 1) ** j`: the newest-sample
/// predictor of a degree-`degree` index fit.
pub fn index_prediction_row(count: usize, degree: usize) -> Vec<f32> {
    let n = degree + 1;
    let design: Vec<f64> =
        (0..count).flat_map(|i| (0..n).map(move |j| (i as f64 - count as f64 + 1.0).powi(j as i32))).collect();
    let mut row = vec![0.0; count];
    for (i, value) in row.iter_mut().enumerate() {
        let unit: Vec<f32> = (0..count).map(|k| if k == i { 1.0 } else { 0.0 }).collect();
        *value = least_squares(&design, n, &unit, 1)[0];
    }
    row
}

/// Per-column squared residual of a least-squares polynomial in `(t - t0) / 30`.
pub fn time_residual(samples: &[f32], channels: usize, times: &[f32], degree: usize) -> Vec<f32> {
    let scaled: Vec<f32> = times.iter().map(|&t| (t - times[0]) * TIME_SCALE).collect();
    let n = degree + 1;
    let design: Vec<f32> = scaled
        .iter()
        .flat_map(|&t| {
            (0..n).rev().map(move |power| if power == 0 { 1.0 } else { f64::from(t).powf(power as f64) as f32 })
        })
        .collect();
    let wide: Vec<f64> = design.iter().map(|&d| f64::from(d)).collect();
    let coefficients = least_squares(&wide, n, samples, channels);
    let rows = times.len();
    (0..channels)
        .map(|c| {
            let total = (0..rows).fold(0.0f64, |total, k| {
                let fitted = (0..n).fold(0.0f64, |sum, i| {
                    sum + f64::from(design[k * n + i]) * f64::from(coefficients[i * channels + c])
                }) as f32;
                let residual = fitted - samples[k * channels + c];
                total + f64::from(residual * residual)
            });
            total as f32
        })
        .collect()
}

/// 1 / value in float32, zero for zero as `cv::divide` does.
pub fn reciprocal(value: f32) -> f32 {
    if value != 0.0 { 1.0 / value } else { 0.0 }
}

/// 0x14004c480 model average over [`DEGREES`] for `count x channels` samples, oldest first.
pub fn polynomial_blend(samples: &[f32], channels: usize, times: &[f32], gains: &[f32]) -> Vec<f32> {
    let count = times.len();
    let mut accumulated = vec![0.0f32; channels];
    let mut weight_sum = vec![0.0f32; channels];
    for (position, degree) in DEGREES.into_iter().enumerate() {
        let row = index_prediction_row(count, degree);
        let residual = time_residual(samples, channels, times, degree);
        for c in 0..channels {
            let prediction =
                (0..count).fold(0.0f64, |t, k| t + f64::from(row[k]) * f64::from(samples[k * channels + c])) as f32;
            let prior = expf(-(position as i32) as f32 * gains[c]);
            let scaled = residual[c] * channels as f32;
            let weight = prior * reciprocal(scaled + RESIDUAL_REGULARIZER);
            accumulated[c] += weight * prediction;
            weight_sum[c] += weight;
        }
    }
    accumulated.iter().zip(&weight_sum).map(|(&a, &w)| a * reciprocal(w + RESIDUAL_REGULARIZER)).collect()
}

/// 0x14007ac80: `y = 1 - y` for present records.
pub fn flip_y(records: &mut Records) {
    for record in records.values_mut().filter(|r| r.present) {
        record.y = 1.0 - record.y;
    }
}

/// 0x14004a530 entries with the configured `smoothing_factors` (Head Tracker.cfg).
pub fn table_entries(factors: &[f32], table: &[(u32, u32, u32, usize)]) -> Result<Vec<TableEntry>> {
    table
        .iter()
        .map(|&(group, first, last, column)| match factors.get(column) {
            Some(&factor) => Ok((group, first, last, factor)),
            None => invalid("Missing smoothing factor"),
        })
        .collect()
}

/// 0x14007f5a0: `factor + (1 - quality) * 0.2`, except for FDP group 15.
pub fn coordinate_gain(group: u32, quality: f32, factor: f32) -> f32 {
    if group == UNSCALED_GROUP {
        return 0.0 + factor;
    }
    (1.0 - quality) * QUALITY_GAIN + factor
}

/// The records [`smooth_records`] wrote: coordinates of every table cell (in table
/// order, created when absent) and the smoothed qualities.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmoothedRecords {
    pub coordinates: Vec<FdpId>,
    pub qualities: Vec<FdpId>,
}

/// 0x14007f5a0: smooths the listed coordinates, then the present non-negative qualities.
///
/// Absent table cells behave as cleared records and are created.
pub fn smooth_records(
    filter: &mut LandmarkFilter,
    records: &mut Records,
    time: i64,
    entries: &[TableEntry],
) -> Result<SmoothedRecords> {
    let mut cells: Vec<(FdpId, f32)> = vec![];
    for &(group, first, last, factor) in entries {
        for index in first..=last {
            let record = records.entry((group, index)).or_default();
            cells.push(((group, index), coordinate_gain(group, record.quality, factor)));
        }
    }
    let values: Vec<f32> = cells.iter().flat_map(|(id, _)| [records[id].x, records[id].y]).collect();
    let gains: Vec<f32> = cells.iter().flat_map(|&(_, gain)| [gain, gain]).collect();
    let smoothed = filter.smooth_coordinates(&values, &gains, time)?;
    for ((id, _), &[x, y]) in cells.iter().zip(smoothed.as_chunks::<2>().0) {
        let record = records.get_mut(id).unwrap();
        (record.x, record.y) = (x, y);
    }
    let [gain, low, high, initial_high] = QUALITY_SMOOTHING;
    let mut qualities = vec![];
    for (group, size) in GROUP_SIZES {
        for index in 1..=size {
            let Some(record) = records.get_mut(&(group, index)) else { continue };
            if !record.present || 0.0 > record.quality {
                continue;
            }
            record.quality = filter.smooth_scalar(record.quality, time, gain, low, high, initial_high)?;
            qualities.push((group, index));
        }
    }
    let mut coordinates: Vec<FdpId> = vec![];
    for (id, _) in cells {
        if !coordinates.contains(&id) {
            coordinates.push(id);
        }
    }
    Ok(SmoothedRecords { coordinates, qualities })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(filter: &mut LandmarkFilter, frames: &[Vec<f32>], gains: &[f32]) -> Vec<Vec<f32>> {
        frames
            .iter()
            .enumerate()
            .map(|(frame, values)| {
                filter.begin_frame();
                filter.smooth_coordinates(values, gains, 30 * frame as i64).unwrap()
            })
            .collect()
    }

    #[test]
    fn reset_clears_history_and_rewinds_rings() {
        let mut filter = LandmarkFilter::new(4, HISTORY);
        run(&mut filter, &vec![vec![0.1, 0.2]; 3], &[1.0, 1.0]);
        filter.reset();
        assert!(filter.times.iter().all(|&t| t == CLEARED_TIME));
        assert!(filter.position.iter().all(|&p| p == 0));
    }

    #[test]
    fn fewer_than_five_samples_pass_through() {
        let frames: Vec<Vec<f32>> = (0..4).map(|f| vec![0.1 * f as f32, 0.5 + 0.01 * (f * f) as f32]).collect();
        let outputs = run(&mut LandmarkFilter::new(4, HISTORY), &frames, &[1.0, 1.0]);
        assert_eq!(outputs, frames);
    }

    #[test]
    fn constant_signal_and_ramp() {
        let outputs = run(&mut LandmarkFilter::new(4, HISTORY), &vec![vec![0.375, 0.625]; 12], &[1.1, 0.9]);
        for output in outputs {
            assert!((output[0] - 0.375).abs() < 1e-6 && (output[1] - 0.625).abs() < 1e-6);
        }
        let frames: Vec<Vec<f32>> = (0..12).map(|f| vec![0.2 + 0.01 * f as f32]).collect();
        let outputs = run(&mut LandmarkFilter::new(2, HISTORY), &frames, &[1.0]);
        assert!((outputs[11][0] - 0.31).abs() < 1e-5);
    }

    #[test]
    fn negative_gain_skips_value_and_channel() {
        let mut filter = LandmarkFilter::new(4, HISTORY);
        filter.begin_frame();
        let output = filter.smooth_coordinates(&[0.1, 0.7, 0.3], &[1.0, -1.0, 1.0], 0).unwrap();
        assert_eq!(filter.cursor, 2);
        assert_eq!(output[1], 0.7);
    }

    #[test]
    fn prediction_rows_are_exact() {
        assert_eq!(index_prediction_row(5, 0), [0.2; 5]);
        assert_eq!(index_prediction_row(5, 1), [-0.2, 0.0, 0.2, 0.4, 0.6]);
        let square = index_prediction_row(5, 4);
        assert_eq!(square[4], 1.0);
        assert!(square[..4].iter().all(|w| w.abs() < 1e-25));
        let line = index_prediction_row(31, 1);
        assert_eq!(line[30], 2.0 * (3.0 * 30.0 - 31.0 + 2.0) / (31.0 * 32.0));
    }

    #[test]
    fn scalar_filter_passes_first_310_ms_then_smooths() {
        let mut filter = LandmarkFilter::new(2, HISTORY);
        let outputs: Vec<f32> = (0..16)
            .map(|frame| {
                filter.begin_frame();
                let value = if frame % 2 == 1 { 0.8 } else { 0.6 };
                filter.smooth_scalar(value, 30 * frame, 0.5, 0.05, 1.0, 0.5).unwrap()
            })
            .collect();
        assert_eq!(outputs[10], 0.6);
        assert!(0.6 < outputs[11] && outputs[11] < 0.8);
        assert!(filter.initialised[0]);
    }

    #[test]
    fn gains_and_channels_cover_table_then_present_qualities() {
        assert!((coordinate_gain(4, 0.5, 1.1) - 1.2).abs() < 1e-6);
        assert!((coordinate_gain(4, -1.0, 1.1) - 1.5).abs() < 1e-6);
        assert!((coordinate_gain(15, 0.5, 1.2) - 1.2).abs() < 1e-6);
        let mut records: Records = (1..=6)
            .map(|i| ((4, i), LandmarkRecord { x: 0.1 * i as f32, y: 0.5, present: true, quality: 0.8 }))
            .collect();
        records.insert((9, 3), LandmarkRecord { x: 0.4, y: 0.4, present: true, quality: -1.0 });
        let mut filter = LandmarkFilter::default();
        filter.begin_frame();
        smooth_records(&mut filter, &mut records, 0, &[(4, 1, 6, 1.1), (14, 1, 2, 1.1)]).unwrap();
        assert_eq!(filter.cursor, 2 * 8 + 6);
        assert!(!records[&(14, 1)].present);
    }

    #[test]
    fn flip_only_touches_present_records() {
        let mut records = Records::new();
        records.insert((3, 1), LandmarkRecord { x: 0.2, y: 0.25, present: true, quality: 0.9 });
        records.insert((3, 2), LandmarkRecord { x: 0.2, y: 0.25, present: false, quality: -1.0 });
        flip_y(&mut records);
        assert_eq!((records[&(3, 1)].y, records[&(3, 2)].y), (0.75, 0.25));
    }
}
