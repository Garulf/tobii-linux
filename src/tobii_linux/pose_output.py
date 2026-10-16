"""Head-pose output conversions recovered from the platform algorithm module.

The arithmetic is crates/tobii-headpose/src/output.rs.
"""
from ._arrays import floats
from ._native import pose_output as _output

PI = 3.1415927410125732
TWO_PI = 6.2831854820251465
MILLIMETRES_PER_METRE = 1000.0
ROTATION_FILTER = (120.0, 1.0, 2.5, 2.5)
POSITION_FILTER = (120.0, 0.04, 0.04, 0.04)
MICROSECONDS_PER_SECOND = 1000000.0


def _matrix3(values):
    import numpy as np
    return np.array(values, dtype=np.float32).reshape(3, 3)


def euler_matrix(angles):
    """Order-3 matrix of 0x1402b1c00: transpose of Ry(b) @ Rx(a) @ Rz(c).

The result is laid out for row vectors, matching the original's storage.
"""
    return _matrix3(_output.euler_matrix(tuple(angles)))


def wrap_angle(angle):
    """0x1402b0dc0: shift by float32 2*pi into [-pi, pi]; NaN passes through."""
    import numpy as np
    return np.float32(_output.wrap_angle(angle))


def matrix_euler(matrix):
    """0x1402b1020: angles (a, b, c) from an order-3 matrix, each wrapped."""
    import numpy as np
    return np.array(_output.matrix_euler(floats(matrix).reshape(9).tolist()), dtype=np.float32)


def adapter_head_pose(fitted_rotation, fitted_translation):
    """Face adapter slot 5 (0x140291830): order-3 matrix and millimetre position.

`fitted_rotation` is the 0x140055470 output (rx, ry - pi, rz) in radians and
`fitted_translation` the fitted translation in model units (metres).
"""
    import numpy as np
    translation = floats(fitted_translation)
    if translation.shape != (3,):
        raise ValueError('Translation must have three values')
    matrix, position = _output.adapter_head_pose(tuple(fitted_rotation), translation.tolist())
    return _matrix3(matrix), np.array(position, dtype=np.float32)


class OneEuroFilter:
    """Scalar One Euro filter of 0x140295460 with low-pass 0x1402958e0.

`config` is (fallback_rate_hz, min_cutoff, beta, derivative_cutoff). The
derivative is taken against the previous filtered value.
"""
    def __init__(self, config=ROTATION_FILTER):
        self._native = _output.OneEuroFilter(tuple(config))

    def __call__(self, value, timestamp_us):
        import numpy as np
        return np.float32(self._native.filter(value, timestamp_us))


class OneEuroVectorFilter:
    """Three-component One Euro filter of 0x1402955d0 with low-pass 0x140295940.

The cutoff uses the Euclidean norm of the filtered derivative.
"""
    def __init__(self, config=POSITION_FILTER):
        self._native = _output.OneEuroVectorFilter(tuple(config))

    def __call__(self, value, timestamp_us):
        import numpy as np
        value = floats(value)
        if value.shape != (3,):
            raise ValueError('Vector filter expects three values')
        return np.array(self._native.filter(value.tolist(), timestamp_us), dtype=np.float32)


class HeadPoseFilter:
    """Filter stage 0x140295790: per-angle filters and the position filter.

Its first filter object smooths angle 1 and the second angle 0, so with the
identical installed configurations the routing changes only state ownership.
"""
    def __init__(self, rotation_config=ROTATION_FILTER, position_config=POSITION_FILTER):
        self._native = _output.HeadPoseFilter(tuple(rotation_config), tuple(position_config))

    def __call__(self, rotation, position, timestamp_us):
        import numpy as np
        rotation, position = floats(rotation).reshape(-1)[:3], floats(position)
        if rotation.shape != (3,) or position.shape != (3,):
            raise ValueError('Vector filter expects three values')
        smoothed, moved = self._native.filter(rotation.tolist(), position.tolist(), timestamp_us)
        return np.array(smoothed, dtype=np.float32), np.array(moved, dtype=np.float32)
