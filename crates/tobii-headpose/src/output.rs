//! Head-pose output conversions recovered from the platform algorithm module.

use crate::array::Vec3;

/// float32 pi, the runtime's constant.
pub const PI: f32 = std::f32::consts::PI;
/// float32 2 pi.
pub const TWO_PI: f32 = std::f32::consts::TAU;
pub const MILLIMETRES_PER_METRE: f32 = 1000.0;

/// An order-3 matrix in the original's storage: row-major, applied to row vectors.
pub type Matrix3 = [[f32; 3]; 3];

fn sincos(angle: f32) -> (f32, f32) {
    (f64::from(angle).cos() as f32, f64::from(angle).sin() as f32)
}

/// `Rx`, `Ry` and `Rz` for row vectors.
pub(crate) fn rotations(angles: Vec3) -> (Matrix3, Matrix3, Matrix3) {
    let (ca, sa) = sincos(angles[0]);
    let (cb, sb) = sincos(angles[1]);
    let (cc, sc) = sincos(angles[2]);
    (
        [[1.0, 0.0, 0.0], [0.0, ca, sa], [0.0, -sa, ca]],
        [[cb, 0.0, -sb], [0.0, 1.0, 0.0], [sb, 0.0, cb]],
        [[cc, sc, 0.0], [-sc, cc, 0.0], [0.0, 0.0, 1.0]],
    )
}

/// 3x3 float32 product with each entry summed as `(0 + 1) + 2`.
fn product(left: &Matrix3, right: &Matrix3) -> Matrix3 {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| left[i][0] * right[0][j] + left[i][1] * right[1][j] + left[i][2] * right[2][j])
    })
}

/// Order-3 matrix of 0x1402b1c00: the transpose of `Ry(b) @ Rx(a) @ Rz(c)`.
pub fn euler_matrix(angles: Vec3) -> Matrix3 {
    let (x, y, z) = rotations(angles);
    product(&product(&z, &x), &y)
}

/// 0x1402b0dc0: shifts by float32 2 pi into `[-pi, pi]`; NaN passes through.
pub fn wrap_angle(angle: f32) -> f32 {
    let mut angle = angle;
    while angle < -PI {
        angle += TWO_PI;
    }
    while angle > PI {
        angle -= TWO_PI;
    }
    angle
}

/// 0x1402b1020: angles `(a, b, c)` from an order-3 matrix, each wrapped.
pub fn matrix_euler(matrix: &Matrix3) -> Vec3 {
    let m = |i: usize| f64::from(matrix[i / 3][i % 3]);
    let b = m(6).atan2(m(8)) as f32;
    let sine = -m(7);
    let a = if (-1.0..=1.0).contains(&sine) { sine.asin() as f32 } else { f32::NAN };
    let c = m(1).atan2(m(4)) as f32;
    [wrap_angle(a), wrap_angle(b), wrap_angle(c)]
}

/// Face adapter slot 5 (0x140291830): order-3 matrix and millimetre position.
///
/// `fitted_rotation` is the 0x140055470 output `(rx, ry - pi, rz)` in radians and
/// `fitted_translation` the fitted translation in model units (metres).
pub fn adapter_head_pose(fitted_rotation: Vec3, fitted_translation: Vec3) -> (Matrix3, Vec3) {
    (euler_matrix(fitted_rotation), fitted_translation.map(|t| t * MILLIMETRES_PER_METRE))
}

/// One Euro filter settings: `(fallback_rate_hz, min_cutoff, beta, derivative_cutoff)`.
pub type FilterConfig = [f32; 4];
pub const ROTATION_FILTER: FilterConfig = [120.0, 1.0, 2.5, 2.5];
pub const POSITION_FILTER: FilterConfig = [120.0, 0.04, 0.04, 0.04];
pub const MICROSECONDS_PER_SECOND: f32 = 1_000_000.0;

/// Sample rate from microsecond timestamps; unsigned difference, as 0x140295460.
fn rate(timestamp_us: i64, previous_us: i64, fallback: f32) -> f32 {
    if timestamp_us == previous_us {
        return fallback;
    }
    let difference = timestamp_us.wrapping_sub(previous_us) as u64;
    MICROSECONDS_PER_SECOND / (difference as f64 as f32)
}

fn smoothing(cutoff: f32, rate: f32) -> f32 {
    let weight = cutoff * TWO_PI;
    weight / (weight + rate)
}

/// Scalar One Euro filter of 0x140295460 with low-pass 0x1402958e0.
///
/// The derivative is taken against the previous filtered value.
#[derive(Debug, Clone, PartialEq)]
pub struct OneEuroFilter {
    pub config: FilterConfig,
    pub previous_us: i64,
    pub filtered: Option<f32>,
    pub derivative: Option<f32>,
}

impl OneEuroFilter {
    pub fn new(config: FilterConfig) -> Self {
        Self { config, previous_us: 0, filtered: None, derivative: None }
    }

    pub fn filter(&mut self, value: f32, timestamp_us: i64) -> f32 {
        let [fallback, min_cutoff, beta, derivative_cutoff] = self.config;
        let rate = rate(timestamp_us, self.previous_us, fallback);
        self.previous_us = timestamp_us;
        let derivative = self.filtered.map_or(0.0, |filtered| (value - filtered) * rate);
        let alpha = smoothing(derivative_cutoff, rate);
        let previous_derivative = self.derivative.unwrap_or(derivative);
        let smoothed = (1.0 - alpha) * previous_derivative + derivative * alpha;
        self.derivative = Some(smoothed);
        let cutoff = (smoothed.abs() * beta + min_cutoff) * TWO_PI;
        let alpha = cutoff / (cutoff + rate);
        let previous = self.filtered.unwrap_or(value);
        let filtered = (1.0 - alpha) * previous + alpha * value;
        self.filtered = Some(filtered);
        filtered
    }
}

/// Three-component One Euro filter of 0x1402955d0 with low-pass 0x140295940.
///
/// The cutoff uses the Euclidean norm of the filtered derivative.
#[derive(Debug, Clone, PartialEq)]
pub struct OneEuroVectorFilter {
    pub config: FilterConfig,
    pub previous_us: i64,
    pub filtered: Option<Vec3>,
    pub derivative: Option<Vec3>,
}

impl OneEuroVectorFilter {
    pub fn new(config: FilterConfig) -> Self {
        Self { config, previous_us: 0, filtered: None, derivative: None }
    }

    pub fn filter(&mut self, value: Vec3, timestamp_us: i64) -> Vec3 {
        let [fallback, min_cutoff, beta, derivative_cutoff] = self.config;
        let rate = rate(timestamp_us, self.previous_us, fallback);
        self.previous_us = timestamp_us;
        let derivative = match self.filtered {
            Some(filtered) => std::array::from_fn(|i| (value[i] - filtered[i]) * rate),
            None => [0.0; 3],
        };
        let alpha = smoothing(derivative_cutoff, rate);
        let previous_derivative = self.derivative.unwrap_or(derivative);
        let smoothed: Vec3 = std::array::from_fn(|i| derivative[i] * alpha + (1.0 - alpha) * previous_derivative[i]);
        self.derivative = Some(smoothed);
        let [x, y, z] = smoothed;
        let speed = ((z * z + y * y) + x * x).sqrt();
        let cutoff = (speed * beta + min_cutoff) * TWO_PI;
        let alpha = cutoff / (cutoff + rate);
        let previous = self.filtered.unwrap_or(value);
        let filtered = std::array::from_fn(|i| (1.0 - alpha) * previous[i] + alpha * value[i]);
        self.filtered = Some(filtered);
        filtered
    }
}

/// Filter stage 0x140295790: per-angle filters and the position filter.
///
/// Its first filter object smooths angle 1 and the second angle 0, so with the identical
/// installed configurations the routing changes only state ownership.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadPoseFilter {
    pub angles: [OneEuroFilter; 3],
    pub position: OneEuroVectorFilter,
}

impl Default for HeadPoseFilter {
    fn default() -> Self {
        Self::new(ROTATION_FILTER, POSITION_FILTER)
    }
}

impl HeadPoseFilter {
    pub fn new(rotation: FilterConfig, position: FilterConfig) -> Self {
        Self {
            angles: std::array::from_fn(|_| OneEuroFilter::new(rotation)),
            position: OneEuroVectorFilter::new(position),
        }
    }

    pub fn filter(&mut self, rotation: Vec3, position: Vec3, timestamp_us: i64) -> (Vec3, Vec3) {
        let mut smoothed = [0.0; 3];
        smoothed[1] = self.angles[0].filter(rotation[1], timestamp_us);
        smoothed[0] = self.angles[1].filter(rotation[0], timestamp_us);
        smoothed[2] = self.angles[2].filter(rotation[2], timestamp_us);
        (smoothed, self.position.filter(position, timestamp_us))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f32], b: &[f32], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
    }

    #[test]
    fn round_trip_recovers_angles() {
        for angles in [[0.15, -0.15, 0.005], [-0.4, 0.7, -0.2], [0.2, -2.9, 1.1]] {
            assert!(close(&matrix_euler(&euler_matrix(angles)), &angles, 2e-6));
        }
    }

    #[test]
    fn wrap_shifts_into_pi_interval_and_keeps_nan() {
        assert!((wrap_angle(4.0) - (4.0 - 2.0 * std::f32::consts::PI)).abs() < 1e-5);
        assert!((wrap_angle(-7.0) - (-7.0 + 2.0 * std::f32::consts::PI)).abs() < 1e-5);
        assert!(wrap_angle(f32::NAN).is_nan());
    }

    #[test]
    fn adapter_scales_translation_to_millimetres() {
        let (matrix, position) = adapter_head_pose([0.0; 3], [0.01, -0.02, 0.5]);
        assert!(close(&position, &[10.0, -20.0, 500.0], 1e-4));
        assert_eq!(matrix, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }

    #[test]
    fn filter_passes_first_sample_and_holds_steady_input() {
        let mut stage = HeadPoseFilter::default();
        for step in 0..5 {
            let (rotation, position) = stage.filter([0.1, -0.2, 0.3], [-50.0, -15.0, 530.0], 1_000_000 + 8333 * step);
            assert!(close(&rotation, &[0.1, -0.2, 0.3], 1e-6));
            assert!(close(&position, &[-50.0, -15.0, 530.0], 1e-3));
        }
    }

    #[test]
    fn filter_lags_a_position_step_and_repeats_use_fallback() {
        let mut smoother = OneEuroVectorFilter::new(POSITION_FILTER);
        smoother.filter([0.0, 0.0, 500.0], 1_000_000);
        let moved = smoother.filter([0.0, 0.0, 600.0], 1_008_333);
        assert!(500.0 < moved[2] && moved[2] < 600.0);
        let mut scalar = OneEuroFilter::new(ROTATION_FILTER);
        scalar.filter(0.0, 5);
        let repeated = scalar.filter(1.0, 5);
        assert!(0.0 < repeated && repeated < 1.0);
    }
}
