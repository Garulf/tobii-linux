"""Float32 inversion arithmetic recovered from the fitting runtime.

The arithmetic is crates/tobii-headpose/src/linalg.rs.
"""
from ._arrays import array, floats
from ._native import pose_linalg as _linalg


def inverse_float32(matrix):
    """Partial-pivot elimination and sequential back substitution (order >= 4).

Matches the observed scalar float32 path at 0x140085b70. Unlike a LAPACK
inverse, each multiply, addition, subtraction and reciprocal rounds to float32.
The runtime's small closed-form inverse paths are not implemented here.
"""
    a = floats(matrix)
    if a.ndim != 2 or a.shape[0] != a.shape[1]:
        raise ValueError('Expected a finite square matrix of order at least four')
    count = len(a)
    return array(_linalg.inverse_float32(a.tobytes(), count), count, count)
