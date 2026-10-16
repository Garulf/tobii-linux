//! Deterministic float conversions shared by every decoder.
//!
//! The runtime keeps most values in float32 and converts fixed-point wire values with
//! plain scalar SSE instructions. These helpers reproduce those conversions exactly:
//! round-to-nearest-even narrowing, truncating float-to-integer conversion and power of
//! two scaling, which is exact in both precisions. See docs/rust.md for the rules.

pub const FIXED16_ONE: f64 = 65536.0;
pub const FIXED42_ONE: f64 = 4_398_046_511_104.0;
pub const FIXED16_SCALE: f64 = 1.0 / FIXED16_ONE;
pub const FIXED42_SCALE: f64 = 1.0 / FIXED42_ONE;

/// Narrows to float32 with round-to-nearest-even and widens back, as `cvtsd2ss` then `cvtss2sd`.
pub fn round_f32(value: f64) -> f64 {
    f64::from(value as f32)
}

/// A 16.16 wire value: the integer is first rounded to float32, then scaled.
pub fn fixed16(raw: i32) -> f64 {
    f64::from(raw as f32) * FIXED16_SCALE
}

/// A 22.42 wire value scaled in double precision.
pub fn fixed42(raw: i64) -> f64 {
    raw as f64 * FIXED42_SCALE
}

/// A 22.42 wire value as the runtime stores it in a float32 field.
pub fn fixed42_f32(raw: i64) -> f64 {
    round_f32(fixed42(raw))
}

/// A 16.16 wire value as the runtime stores it in a float32 field.
pub fn fixed16_f32(raw: i32) -> f64 {
    round_f32(fixed16(raw))
}

/// Truncates toward zero when the result fits `[min, max)`; `None` otherwise (including NaN).
pub fn truncate_in_range(value: f64, min: f64, max_exclusive: f64) -> Option<f64> {
    let truncated = value.trunc();
    (truncated >= min && truncated < max_exclusive).then_some(truncated)
}

/// `cvttss2si`: truncation of a float32, with the integer indefinite 0x80000000 out of range.
pub fn cvttss2si(value: f32) -> i32 {
    match truncate_in_range(f64::from(value), -2_147_483_648.0, 2_147_483_648.0) {
        Some(truncated) => truncated as i32,
        None => i32::MIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed16_rounds_through_float32() {
        assert_eq!(fixed16(0x0100_0001), 256.0);
        assert_eq!(fixed16(0x0001_8000), 1.5);
        assert_eq!(fixed16(-0x4000), -0.25);
    }

    #[test]
    fn fixed42_scales_exactly() {
        assert_eq!(fixed42(1 << 42), 1.0);
        assert_eq!(fixed42(-1), -(2.0f64.powi(-42)));
    }

    #[test]
    fn cvttss2si_truncates_and_saturates_to_indefinite() {
        assert_eq!(cvttss2si(-1.5), -1);
        assert_eq!(cvttss2si(2_621_440_000.0), i32::MIN);
        assert_eq!(cvttss2si(f32::NAN), i32::MIN);
        assert_eq!(cvttss2si(f32::NEG_INFINITY), i32::MIN);
    }
}
