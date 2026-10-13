"""Float32 inversion arithmetic recovered from the fitting runtime."""


def inverse_float32(matrix):
    """Partial-pivot elimination and sequential back substitution (order >= 4).

Matches the observed scalar float32 path at 0x140085b70. Unlike a LAPACK
inverse, each multiply, addition, subtraction and reciprocal rounds to float32.
The runtime's small closed-form inverse paths are not implemented here.
"""
    import numpy as np
    a = np.asarray(matrix, dtype=np.float32).copy()
    if a.ndim != 2 or a.shape[0] != a.shape[1] or a.shape[0] < 4 or not np.isfinite(a).all():
        raise ValueError('Expected a finite square matrix of order at least four')
    count = len(a)
    b = np.eye(count, dtype=np.float32)
    for i in range(count):
        pivot = i + int(np.argmax(np.abs(a[i:, i])))
        if abs(a[pivot, i]) < np.finfo(np.float32).eps:
            # The wrapper zeros its output when the LU helper reports failure.
            return np.zeros_like(a)
        if pivot != i:
            a[[i, pivot], i:] = a[[pivot, i], i:]
            b[[i, pivot]] = b[[pivot, i]]
        reciprocal = np.float32(1) / a[i, i]
        negative_reciprocal = np.float32(-1) / a[i, i]
        alpha = negative_reciprocal * a[i+1:, i, None]
        a[i+1:, i+1:] = a[i+1:, i+1:] + alpha * a[i, i+1:]
        b[i+1:] = b[i+1:] + alpha * b[i]
        a[i, i] = reciprocal
    for i in range(count - 1, -1, -1):
        terms = np.concatenate((b[i, None], a[i, i+1:, None] * b[i+1:]))
        b[i] = np.subtract.reduce(terms, axis=0) * a[i, i]
    return b
