"""Recovered landmark smoothing of the per-object tracker step 0x14004a530.

After VNNTrackerCore (0x140049450) writes frame-normalised FDP records with Y
pointing up, 0x14007f5a0 smooths every coordinate listed in a group table with
the history filter 0x14004c480 and then every present, non-negative quality
with 0x14004bf60. Both keep a 31-sample ring per channel; channels are
assigned in call order through the filter cursor, which restarts each frame.

0x14004c480 averages three polynomial fits of the samples inside a 310 ms
window (degrees 0, 1 and 4). Each fit predicts the newest sample from the
first row of the pseudo-inverse of an index Vandermonde matrix and is weighted
by exp(-k * gain) / (channels * residual + 1e-8), where the residual comes from
a least-squares fit in time (milliseconds / 30) and k is the fit's position.
The smoothing is crates/tobii-headpose/src/smoothing.rs.
"""
from dataclasses import dataclass

from ._arrays import array, floats, raw
from ._native import landmark_smoothing as _smoothing

HISTORY = 31
WINDOW_MS = 310
MINIMUM_SAMPLES = 5
CLEARED_TIME = -10000000
DEGREES = (0, 1, 4)
TIME_SCALE = 0.03333333507180214
RESIDUAL_REGULARIZER = 1e-08
QUALITY_GAIN = 0.20000000298023224
SHARPNESS_FLOOR = 0.20000000298023224
SCALAR_DEFAULT_LOW = 1000000.0
SCALAR_GAIN_FLOOR = 0.10000000149011612
SCALAR_DECAY = 30.0
QUALITY_SMOOTHING = (0.5, 0.05000000074505806, 1.0, 0.5)
UNSCALED_GROUP = 15
CHANNELS = 2000

GROUP_SIZES = dict(_smoothing.GROUP_SIZES)

# 0x14004a530 table: (FDP group, first index, last index, smoothing_factors column).
TRACKER_TABLE = tuple(tuple(entry) for entry in _smoothing.TRACKER_TABLE)
EAR_TABLE = tuple(tuple(entry) for entry in _smoothing.EAR_TABLE)


@dataclass
class LandmarkRecord:
    """FDP record fields the smoothing reads and writes (x +0, y +4, present +0xc, quality +0x14)."""
    x: float = 0.0
    y: float = 0.0
    present: bool = False
    quality: float = -1.0


class LandmarkFilter:
    """History filter object built by 0x14004bcc0 (0x48 bytes)."""

    def __init__(self, channels=CHANNELS, history=HISTORY):
        self._native = _smoothing.LandmarkFilter(channels, history)
        self.channels = channels
        self.history = history

    def reset(self):
        """0x14004bed0: rewind every ring, forget initialisation and clear all sample times."""
        self._native.reset()

    def begin_frame(self):
        """0x14004a530 bookkeeping before any channel is used."""
        self._native.begin_frame()

    @property
    def enabled(self):
        return self._native.enabled

    @enabled.setter
    def enabled(self, value):
        self._native.enabled = bool(value)

    @property
    def cursor(self):
        return self._native.cursor

    @cursor.setter
    def cursor(self, value):
        self._native.cursor = value

    @property
    def frames(self):
        return self._native.frames

    @property
    def window(self):
        return self._native.window

    @property
    def values(self):
        return array(self._native.values, self.channels, self.history)

    @property
    def times(self):
        import numpy as np
        return np.array(self._native.times, dtype=np.int64).reshape(self.channels, self.history)

    @property
    def position(self):
        import numpy as np
        return np.array(self._native.position, dtype=np.int64)

    @property
    def initialised(self):
        import numpy as np
        return np.array(self._native.initialised, dtype=bool)

    @property
    def high(self):
        return array(self._native.high, self.channels)

    @property
    def low(self):
        return array(self._native.low, self.channels)

    def smooth_coordinates(self, values, gains, time):
        """0x14004c480: smooth one coordinate per channel; returns the new values.

Coordinates with a negative gain are skipped and use no channel. Fewer than
MINIMUM_SAMPLES samples inside the window leave every value unchanged.
"""
        import numpy as np
        smoothed = self._native.smooth_coordinates([float(v) for v in values], [float(g) for g in gains], time)
        return [np.float32(v) for v in smoothed]

    def smooth_scalar(self, value, time, gain, low_floor, high_cap, initial_high):
        """0x14004bf60: adaptive exponential average of one value; returns the new value."""
        import numpy as np
        return np.float32(self._native.smooth_scalar(value, time, gain, low_floor, high_cap, initial_high))


def index_prediction_row(count, degree):
    """First row of pinv(V) for V[i, j] = (i - count + 1) ** j: the newest-sample predictor."""
    return array(_smoothing.index_prediction_row(count, degree), count)


def time_residual(samples, times, degree):
    """Per-column squared residual of a least-squares polynomial in (t - t0) / 30."""
    samples = floats(samples)
    return array(_smoothing.time_residual(samples.tobytes(), samples.shape[1], [float(t) for t in times], degree),
                 samples.shape[1])


def polynomial_blend(samples, times, gains):
    """0x14004c480 model average over DEGREES for count x channels samples, oldest first."""
    samples = floats(samples)
    return array(_smoothing.polynomial_blend(samples.tobytes(), samples.shape[1], [float(t) for t in times],
                                             [float(g) for g in gains]), samples.shape[1])


def reciprocal(values):
    """1 / values in float32, zero for zero as cv::divide does."""
    values = floats(values)
    return array(_smoothing.reciprocal(values.tobytes()), *values.shape)


def flip_y(records):
    """0x14007ac80: y = 1 - y for present records, marking them present."""
    present = [record for record in records.values() if record.present]
    for record, y in zip(present, _smoothing.flip_y([float(r.y) for r in present])):
        record.y = float(y)


def table_entries(factors, table=TRACKER_TABLE):
    """0x14004a530 entries with the configured smoothing_factors (Head Tracker.cfg)."""
    return tuple(_smoothing.table_entries([float(f) for f in factors], [tuple(entry) for entry in table]))


def coordinate_gain(group, quality, factor):
    """0x14007f5a0: factor + (1 - quality) * 0.2, except for FDP group 15."""
    import numpy as np
    return np.float32(_smoothing.coordinate_gain(group, quality, factor))


def smooth_records(landmark_filter, records, time, entries):
    """0x14007f5a0: smooth listed coordinates, then present non-negative qualities.

records maps (group, index) to LandmarkRecord and is updated in place; absent
keys behave as cleared records.
"""
    entries = [(int(group), int(first), int(last), float(factor)) for group, first, last, factor in entries]
    for group, first, last, _ in entries:
        for index in range(first, last + 1):
            records.setdefault((group, index), LandmarkRecord())
    fields = [(tuple(key), float(r.x), float(r.y), bool(r.present), float(r.quality)) for key, r in records.items()]
    lookup = {tuple(key): record for key, record in records.items()}
    coordinates, qualities = landmark_filter._native.smooth_records(fields, time, entries)
    for key, x, y in coordinates:
        lookup[key].x, lookup[key].y = float(x), float(y)
    for key, quality in qualities:
        lookup[key].quality = float(quality)
