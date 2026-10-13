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


ROTATION_FILTER = (120.0, 1.0, 2.5, 2.5)
POSITION_FILTER = (120.0, 0.04, 0.04, 0.04)
MICROSECONDS_PER_SECOND = 1000000.0


def _rate(timestamp_us, previous_us, fallback):
    """Sample rate from microsecond timestamps; unsigned difference, as 0x140295460."""
    if timestamp_us == previous_us:
        return _float32(fallback)
    difference = (timestamp_us - previous_us) % (1 << 64)
    return _float32(_float32(MICROSECONDS_PER_SECOND) / _float32(difference))


def _smoothing(cutoff, rate):
    import numpy as np
    weight = _float32(_float32(cutoff) * _float32(TWO_PI))
    return np.float32(weight / np.float32(weight + rate))


class OneEuroFilter:
    """Scalar One Euro filter of 0x140295460 with low-pass 0x1402958e0.

`config` is (fallback_rate_hz, min_cutoff, beta, derivative_cutoff). The
derivative is taken against the previous filtered value.
"""
    def __init__(self, config=ROTATION_FILTER):
        self.fallback, self.min_cutoff, self.beta, self.derivative_cutoff = (_float32(v) for v in config)
        self.previous_us = 0
        self.filtered = None
        self.derivative = None

    def __call__(self, value, timestamp_us):
        import numpy as np
        value = _float32(value)
        rate = _rate(timestamp_us, self.previous_us, self.fallback)
        self.previous_us = timestamp_us
        derivative = np.float32((value - self.filtered) * rate) if self.filtered is not None else np.float32(0)
        alpha = _smoothing(self.derivative_cutoff, rate)
        if self.derivative is None:
            self.derivative = derivative
        self.derivative = np.float32(np.float32(np.float32(1) - alpha) * self.derivative
                                     + np.float32(derivative * alpha))
        cutoff = np.float32(np.float32(np.float32(abs(self.derivative)) * self.beta + self.min_cutoff)
                            * np.float32(TWO_PI))
        alpha = np.float32(cutoff / np.float32(cutoff + rate))
        previous = value if self.filtered is None else self.filtered
        self.filtered = np.float32(np.float32(np.float32(1) - alpha) * previous + np.float32(alpha * value))
        return self.filtered


class OneEuroVectorFilter:
    """Three-component One Euro filter of 0x1402955d0 with low-pass 0x140295940.

The cutoff uses the Euclidean norm of the filtered derivative.
"""
    def __init__(self, config=POSITION_FILTER):
        self.fallback, self.min_cutoff, self.beta, self.derivative_cutoff = (_float32(v) for v in config)
        self.previous_us = 0
        self.filtered = None
        self.derivative = None

    def __call__(self, value, timestamp_us):
        import numpy as np
        value = np.asarray(value, dtype=np.float32)
        if value.shape != (3,):
            raise ValueError('Vector filter expects three values')
        rate = _rate(timestamp_us, self.previous_us, self.fallback)
        self.previous_us = timestamp_us
        derivative = ((value - self.filtered) * rate).astype(np.float32) if self.filtered is not None \
            else np.zeros(3, dtype=np.float32)
        alpha = _smoothing(self.derivative_cutoff, rate)
        if self.derivative is None:
            self.derivative = derivative.copy()
        self.derivative = (derivative * alpha + (np.float32(1) - alpha) * self.derivative).astype(np.float32)
        x, y, z = self.derivative
        speed = np.float32(math.sqrt(np.float32(np.float32(z * z + y * y) + x * x)))
        cutoff = np.float32(np.float32(speed * self.beta + self.min_cutoff) * np.float32(TWO_PI))
        rate_sum = np.float32(cutoff + rate)
        alpha = np.float32(cutoff / rate_sum)
        previous = value if self.filtered is None else self.filtered
        self.filtered = ((np.float32(1) - alpha) * previous + alpha * value).astype(np.float32)
        return self.filtered.copy()


class HeadPoseFilter:
    """Filter stage 0x140295790: per-angle filters and the position filter.

Its first filter object smooths angle 1 and the second angle 0, so with the
identical installed configurations the routing changes only state ownership.
"""
    def __init__(self, rotation_config=ROTATION_FILTER, position_config=POSITION_FILTER):
        self.angles = [OneEuroFilter(rotation_config) for _ in range(3)]
        self.position = OneEuroVectorFilter(position_config)

    def __call__(self, rotation, position, timestamp_us):
        import numpy as np
        rotation = np.asarray(rotation, dtype=np.float32)
        smoothed = np.empty(3, dtype=np.float32)
        smoothed[1] = self.angles[0](rotation[1], timestamp_us)
        smoothed[0] = self.angles[1](rotation[0], timestamp_us)
        smoothed[2] = self.angles[2](rotation[2], timestamp_us)
        return smoothed, self.position(position, timestamp_us)
