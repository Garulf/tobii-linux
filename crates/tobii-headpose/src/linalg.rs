//! Float32 inversion arithmetic recovered from the fitting runtime.

use crate::array::Matrix;
use crate::error::{Result, invalid};

/// Partial-pivot elimination and sequential back substitution (order >= 4), 0x140085b70.
///
/// Unlike a LAPACK inverse, each multiply, addition, subtraction and reciprocal rounds to
/// float32. A pivot below float32 epsilon gives the zero matrix, as the wrapper zeros its
/// output when the LU helper reports failure. The runtime's small closed-form inverse
/// paths are not implemented here.
pub fn inverse_f32(matrix: &Matrix) -> Result<Matrix> {
    let count = matrix.rows;
    if matrix.cols != count || count < 4 || !matrix.values.iter().all(|v| v.is_finite()) {
        return invalid("Expected a finite square matrix of order at least four");
    }
    let mut a = matrix.clone();
    let mut b = Matrix::identity(count);
    for i in 0..count {
        let pivot = first_largest_magnitude((i..count).map(|r| a.get(r, i))) + i;
        if a.get(pivot, i).abs() < f32::EPSILON {
            return Ok(Matrix::zeros(count, count));
        }
        if pivot != i {
            for c in i..count {
                a.values.swap(i * count + c, pivot * count + c);
            }
            for c in 0..count {
                b.values.swap(i * count + c, pivot * count + c);
            }
        }
        let reciprocal = 1.0 / a.get(i, i);
        let negative_reciprocal = -1.0 / a.get(i, i);
        for r in i + 1..count {
            let alpha = negative_reciprocal * a.get(r, i);
            for c in i + 1..count {
                a.set(r, c, a.get(r, c) + alpha * a.get(i, c));
            }
            for c in 0..count {
                b.set(r, c, b.get(r, c) + alpha * b.get(i, c));
            }
        }
        a.set(i, i, reciprocal);
    }
    for i in (0..count).rev() {
        for c in 0..count {
            let mut value = b.get(i, c);
            for j in i + 1..count {
                value -= a.get(i, j) * b.get(j, c);
            }
            b.set(i, c, value * a.get(i, i));
        }
    }
    Ok(b)
}

/// numpy's argmax of the absolute values: the first maximum, or the first NaN.
fn first_largest_magnitude(values: impl Iterator<Item = f32>) -> usize {
    let mut best = (0, f32::NEG_INFINITY);
    for (index, value) in values.enumerate() {
        let magnitude = value.abs();
        if magnitude.is_nan() {
            return index;
        }
        if magnitude > best.1 {
            best = (index, magnitude);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permutation_inverts_to_its_transpose() {
        let mut permutation = Matrix::zeros(6, 6);
        for r in 0..6 {
            permutation.set((r + 2) % 6, r, 1.0);
        }
        let inverse = inverse_f32(&permutation).unwrap();
        for r in 0..6 {
            for c in 0..6 {
                assert_eq!(inverse.get(r, c), permutation.get(c, r));
            }
        }
    }

    #[test]
    fn singular_matrix_gives_zeros() {
        let ones = Matrix::from_values(6, 6, vec![1.0; 36]).unwrap();
        assert_eq!(inverse_f32(&ones).unwrap(), Matrix::zeros(6, 6));
    }

    #[test]
    fn rejects_small_or_nonfinite_matrices() {
        assert!(inverse_f32(&Matrix::identity(3)).is_err());
        let mut nan = Matrix::identity(4);
        nan.set(1, 2, f32::NAN);
        assert!(inverse_f32(&nan).is_err());
    }
}
