//! Python and NumPy float semantics the recovered routines were checked against.
//!
//! The Python reconstruction mixes Python floats (doubles rounded to float32 where it
//! packs them), NumPy float32 scalars and NumPy reductions. These helpers reproduce what
//! those produce: float32 packing with Python's overflow check, Python's `max`/`min` and
//! `round`, NumPy's int64 conversion and the summation orders of `mean` and `sum`.

use crate::error::{Error, Result};

/// 2^63, the first float64 beyond the int64 range.
const INT64_END: f64 = (1u64 << 63) as f64;

/// `struct.pack('<f', value)`: round to nearest float32; a finite value beyond the
/// float32 range raises OverflowError instead of becoming infinite.
pub fn pack_f32(value: f64) -> Result<f32> {
    let rounded = value as f32;
    if rounded.is_infinite() && value.is_finite() {
        return Err(Error::Overflow("float too large to pack with f format".into()));
    }
    Ok(rounded)
}

/// Python's `max(a, b)`: `b` only when `b > a`, so a NaN first argument is kept.
pub fn py_max(a: f32, b: f32) -> f32 {
    if b > a { b } else { a }
}

/// Python's `min(a, b)`: `b` only when `b < a`.
pub fn py_min(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}

/// Python's `round(float)` to an integer: ties to even, NaN and infinity refused.
pub fn py_round(value: f64) -> Result<i64> {
    if value.is_nan() {
        return Err(Error::Value("cannot convert float NaN to integer".into()));
    }
    let rounded = value.round_ties_even();
    if !(-INT64_END..INT64_END).contains(&rounded) {
        return Err(Error::Overflow("cannot convert float infinity to integer".into()));
    }
    Ok(rounded as i64)
}

/// `math.exp` of a Python float: OverflowError where the result is out of range.
pub fn py_exp(value: f64) -> Result<f64> {
    let result = value.exp();
    if result.is_infinite() && value.is_finite() {
        return Err(Error::Overflow("math range error".into()));
    }
    Ok(result)
}

/// NumPy's float64 to int64 cast on x86 (`cvttsd2si`): truncation, with
/// 0x8000000000000000 for NaN and out-of-range values.
pub fn cvttsd2si64(value: f64) -> i64 {
    if (-INT64_END..INT64_END).contains(&value) { value as i64 } else { i64::MIN }
}

/// NumPy's float64 pairwise summation (`DOUBLE_pairwise_sum`): eight accumulators per
/// block of at most 128 values, halves split on multiples of eight.
pub fn pairwise_sum(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 8 {
        values.iter().fold(0.0, |sum, &v| sum + v)
    } else if n <= 128 {
        let mut r: [f64; 8] = values[..8].try_into().expect("eight values");
        let blocks = n - n % 8;
        for block in values[8..blocks].as_chunks::<8>().0 {
            for (accumulator, value) in r.iter_mut().zip(block) {
                *accumulator += value;
            }
        }
        let mut sum = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for value in &values[blocks..] {
            sum += value;
        }
        sum
    } else {
        let half = n / 2;
        let half = half - half % 8;
        pairwise_sum(&values[..half]) + pairwise_sum(&values[half..])
    }
}

/// `np.sum` of a contiguous float64 array: the identity plus the pairwise sum.
pub fn numpy_sum(values: &[f64]) -> f64 {
    0.0 + pairwise_sum(values)
}

/// `points.mean(axis=0, dtype=np.float32)` of an N×2 float32 array: each column summed in
/// order from zero in float32, then divided by N.
pub fn mean_columns(points: &[[f32; 2]]) -> [f32; 2] {
    let mut sum = [0.0f32; 2];
    for point in points {
        sum[0] += point[0];
        sum[1] += point[1];
    }
    let count = points.len() as f32;
    [sum[0] / count, sum[1] / count]
}

/// Row-by-column products of float64 matrices, each accumulated from zero in order, as
/// NumPy's matmul computes them for these small shapes.
pub fn dot(row: &[f64], column: impl IntoIterator<Item = f64>) -> f64 {
    row.iter().zip(column).fold(0.0, |sum, (&a, b)| sum + a * b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_rounds_and_refuses_overflow() {
        assert_eq!(pack_f32(0.1).unwrap(), 0.1f32);
        assert_eq!(pack_f32(f64::INFINITY).unwrap(), f32::INFINITY);
        assert!(pack_f32(f64::NAN).unwrap().is_nan());
        assert!(matches!(pack_f32(1e39), Err(Error::Overflow(_))));
    }

    #[test]
    fn python_max_and_min_keep_the_first_argument_on_ties_and_nan() {
        assert!(py_max(f32::NAN, 1.0).is_nan());
        assert_eq!(py_max(1.0, f32::NAN), 1.0);
        assert_eq!(py_min(2.0, 1.0), 1.0);
        assert_eq!(py_max(0.0, -0.0).to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn python_round_is_half_even() {
        assert_eq!([0.5, 1.5, 2.5, -0.5, -1.5].map(|v| py_round(v).unwrap()), [0, 2, 2, 0, -2]);
        assert!(py_round(f64::NAN).is_err());
        assert!(py_round(f64::INFINITY).is_err());
    }

    #[test]
    fn sums_follow_numpy() {
        let values: Vec<f64> = (0..300).map(|i| 1.0 / (f64::from(i) + 0.37)).collect();
        // np.sum(values[:n]) and the float32 column means, as NumPy 2.4 computed them.
        for (n, bits) in [
            (5, 0x4011_8543_49b0_4423),
            (57, 0x401b_5825_f252_488d),
            (114, 0x401e_1f17_3c19_cf67),
            (129, 0x401e_9dce_811d_0000),
            (300, 0x4020_ff4f_2c71_4ee8),
        ] {
            assert_eq!(numpy_sum(&values[..n]).to_bits(), bits, "{n} values");
        }
        let points: Vec<[f32; 2]> = values[..114].chunks(2).map(|p| [p[0] as f32, p[1] as f32]).collect();
        assert_eq!(mean_columns(&points).map(f32::to_bits), [0x3daf_2db0, 0x3d3e_c5b1]);
    }
}
