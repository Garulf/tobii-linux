"""Head-pose output conversions recovered from the platform algorithm module."""
import math

PI = 3.1415927410125732
TWO_PI = 6.2831854820251465
MILLIMETRES_PER_METRE = 1000.0


def _float32(value):
    import numpy as np
    return np.float32(value)


def _rotations(angles):
    import numpy as np
    a, b, c = (_float32(v) for v in angles)
    ca, sa = _float32(math.cos(a)), _float32(math.sin(a))
    cb, sb = _float32(math.cos(b)), _float32(math.sin(b))
    cc, sc = _float32(math.cos(c)), _float32(math.sin(c))
    x = np.array([[1, 0, 0], [0, ca, sa], [0, -sa, ca]], dtype=np.float32)
    y = np.array([[cb, 0, -sb], [0, 1, 0], [sb, 0, cb]], dtype=np.float32)
    z = np.array([[cc, sc, 0], [-sc, cc, 0], [0, 0, 1]], dtype=np.float32)
    return x, y, z


def euler_matrix(angles):
    """Order-3 matrix of 0x1402b1c00: transpose of Ry(b) @ Rx(a) @ Rz(c).

The result is laid out for row vectors, matching the original's storage.
"""
    x, y, z = _rotations(angles)
    return z @ x @ y


def wrap_angle(angle):
    """0x1402b0dc0: shift by float32 2*pi into [-pi, pi]; NaN passes through."""
    angle = _float32(angle)
    while angle < _float32(-PI):
        angle = _float32(angle + _float32(TWO_PI))
    while angle > _float32(PI):
        angle = _float32(angle - _float32(TWO_PI))
    return angle


def matrix_euler(matrix):
    """0x1402b1020: angles (a, b, c) from an order-3 matrix, each wrapped."""
    import numpy as np
    m = np.asarray(matrix, dtype=np.float32).reshape(9)
    b = _float32(math.atan2(m[6], m[8]))
    sine = -float(m[7])
    a = _float32(math.asin(sine)) if -1.0 <= sine <= 1.0 else _float32('nan')
    c = _float32(math.atan2(m[1], m[4]))
    return np.array([wrap_angle(a), wrap_angle(b), wrap_angle(c)], dtype=np.float32)


def adapter_head_pose(fitted_rotation, fitted_translation):
    """Face adapter slot 5 (0x140291830): order-3 matrix and millimetre position.

`fitted_rotation` is the 0x140055470 output (rx, ry - pi, rz) in radians and
`fitted_translation` the fitted translation in model units (metres).
"""
    import numpy as np
    translation = np.asarray(fitted_translation, dtype=np.float32)
    if translation.shape != (3,):
        raise ValueError('Translation must have three values')
    position = translation * np.float32(MILLIMETRES_PER_METRE)
    return euler_matrix(fitted_rotation), position
