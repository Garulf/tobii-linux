//! Double-precision helpers that give the same results as the Python implementation.
//!
//! The display setup and calibration models run in double precision, as the configuration
//! app's .NET code does; only their final float32 rounding is a runtime value. Distances
//! use CPython's `vector_norm` (math.hypot, math.dist) so that values compared or
//! rounded downstream stay identical to what the Python modules produced.

pub use tobii_protocol::numeric::round_f32 as float32;

const T27: f64 = 134_217_729.0;

/// `frexp` exponent of a positive finite value: `value * 2^-e` lies in [0.5, 1).
fn frexp_exponent(value: f64) -> i32 {
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7FF) as i32;
    if biased != 0 {
        return biased - 1022;
    }
    let mantissa = bits & ((1 << 52) - 1);
    let highest = 63 - mantissa.leading_zeros() as i32;
    highest - 1073
}

/// `ldexp(1.0, exponent)` for exponents from -1074 to 1023.
fn power_of_two(exponent: i32) -> f64 {
    if exponent >= -1022 {
        f64::from_bits(((exponent + 1023) as u64) << 52)
    } else {
        f64::from_bits(1u64 << (exponent + 1074))
    }
}

/// CPython 3.11 `vector_norm`: the Euclidean norm of finite non-negative values whose
/// maximum is `max`, with the compensated summation that makes it correctly rounded in
/// practice.
fn vector_norm(values: &[f64], max: f64) -> f64 {
    if max.is_infinite() {
        return max;
    }
    if values.iter().any(|v| v.is_nan()) {
        return f64::NAN;
    }
    if max == 0.0 || values.len() <= 1 {
        return max;
    }
    let mut csum = 1.0;
    let (mut frac1, mut frac2, mut frac3) = (0.0, 0.0, 0.0);
    let max_e = frexp_exponent(max);
    if max_e < -1023 {
        for &value in values {
            let x = value / max;
            let x = x * x;
            let old = csum;
            csum += x;
            frac1 += (old - csum) + x;
        }
        return max * (csum - 1.0 + frac1).sqrt();
    }
    let scale = power_of_two(-max_e);
    let split = |x: f64| {
        let t = x * T27;
        let hi = t - (t - x);
        (hi, x - hi)
    };
    for &value in values {
        let (hi, lo) = split(value * scale);
        let x = hi * hi;
        let old = csum;
        csum += x;
        frac1 += (old - csum) + x;
        let x = 2.0 * hi * lo;
        let old = csum;
        csum += x;
        frac2 += (old - csum) + x;
        frac3 += lo * lo;
    }
    let h = (csum - 1.0 + (frac1 + frac2 + frac3)).sqrt();
    let (hi, lo) = split(h);
    let x = -hi * hi;
    let old = csum;
    csum += x;
    frac1 += (old - csum) + x;
    let x = -2.0 * hi * lo;
    let old = csum;
    csum += x;
    frac2 += (old - csum) + x;
    let x = -lo * lo;
    let old = csum;
    csum += x;
    frac3 += (old - csum) + x;
    let x = csum - 1.0 + (frac1 + frac2 + frac3);
    (h + x / (2.0 * h)) / scale
}

/// `math.hypot(*values)`.
pub fn hypot(values: &[f64]) -> f64 {
    let magnitudes: Vec<f64> = values.iter().map(|v| v.abs()).collect();
    let max = magnitudes.iter().copied().filter(|v| !v.is_nan()).fold(0.0, f64::max);
    vector_norm(&magnitudes, max)
}

/// `math.dist(p, q)`.
pub fn dist(p: &[f64], q: &[f64]) -> f64 {
    let differences: Vec<f64> = p.iter().zip(q).map(|(a, b)| (a - b).abs()).collect();
    hypot(&differences)
}

/// `round(value, digits)`: the double nearest to the decimal rounded half to even.
pub fn round_digits(value: f64, digits: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// Python's `min(a, b)`: `a` unless `b` is smaller.
pub fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Python's `max(a, b)`: `a` unless `b` is larger.
pub fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Python's `sum()` of floats: left to right from integer zero.
pub fn sum(values: impl IntoIterator<Item = f64>) -> f64 {
    values.into_iter().fold(0.0, |total, value| total + value)
}

/// `math.degrees`: CPython multiplies by the rounded constant 180 / pi.
pub fn degrees(radians: f64) -> f64 {
    radians * (180.0 / std::f64::consts::PI)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hypot_matches_cpython() {
        assert_eq!(hypot(&[3.0, 4.0]), 5.0);
        assert_eq!(hypot(&[597.0, 336.1]), 685.1074441282915);
        assert_eq!(hypot(&[0.0, 1.0, 0.0]), 1.0);
        assert_eq!(hypot(&[-0.1, 315.7367, 114.9187]), 335.9999722270524);
        assert_eq!(hypot(&[]), 0.0);
        assert!(hypot(&[f64::NAN, 1.0]).is_nan());
        assert_eq!(hypot(&[f64::INFINITY, f64::NAN]), f64::INFINITY);
        assert_eq!(hypot(&[5e-324, 5e-324]), 5e-324);
    }

    #[test]
    fn dist_matches_cpython() {
        assert_eq!(dist(&[256.0, 1296.0], &[1280.0, 144.0]), 1541.3241060854139);
        assert_eq!(dist(&[0.2, 0.3], &[0.2015, 0.3]), 0.0015000000000000013);
    }

    #[test]
    fn round_digits_is_half_even_on_the_exact_value() {
        assert_eq!(round_digits(0.03125, 4), 0.0312);
        assert_eq!(round_digits(0.0125, 4), 0.0125);
        assert_eq!(round_digits(1.4995, 3), 1.5);
        assert_eq!(round_digits(0.00015, 4), 0.0001);
        assert_eq!(round_digits(2.5, 0), 2.0);
    }

    #[test]
    fn degrees_uses_the_rounded_constant() {
        assert_eq!(degrees(0.3490658503988659), 20.0);
        assert_eq!(degrees(1.0), 57.29577951308232);
    }
}
