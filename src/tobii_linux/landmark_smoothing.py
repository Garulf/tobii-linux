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
"""
from dataclasses import dataclass
import math

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

GROUP_SIZES = {2: 14, 3: 14, 4: 6, 5: 4, 6: 4, 7: 1, 8: 10, 9: 15, 10: 24, 11: 6, 12: 47, 13: 17, 14: 26, 15: 17}

# 0x14004a530 table: (FDP group, first index, last index, smoothing_factors column).
TRACKER_TABLE = (
    (4, 1, 6, 0), (14, 1, 4, 0),
    (8, 1, 10, 1), (2, 2, 9, 1),
    (3, 5, 6, 2),
    (3, 1, 4, 3), (3, 7, 14, 3), (12, 5, 12, 3),
    (9, 3, 5, 4), (9, 15, 15, 4), (14, 22, 25, 4),
    (2, 1, 1, 5), (15, 1, 17, 5),
)
EAR_TABLE = ((10, 1, 24, 7),)


def _f32(value):
    import numpy as np
    return np.float32(value)


def _expf(value):
    return _f32(math.exp(float(value)))


def _powf(base, exponent):
    return _f32(math.pow(float(base), float(exponent)))


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
        import numpy as np
        self.channels = channels
        self.history = history
        self.enabled = True
        self.cursor = 0
        self.frames = 0
        self.values = np.zeros((channels, history), dtype=np.float32)
        self.times = np.zeros((channels, history), dtype=np.int64)
        self.position = np.zeros(channels, dtype=np.int64)
        self.initialised = np.zeros(channels, dtype=bool)
        self.high = np.zeros(channels, dtype=np.float32)
        self.low = np.zeros(channels, dtype=np.float32)
        self.reset()

    def reset(self):
        """0x14004bed0: rewind every ring, forget initialisation and clear all sample times."""
        self.position[:] = 0
        self.initialised[:] = False
        self.frames = 0
        self.enabled = True
        self.times[:] = CLEARED_TIME

    def begin_frame(self):
        """0x14004a530 bookkeeping before any channel is used."""
        self.frames += 1
        self.cursor = 0

    @property
    def window(self):
        return max(WINDOW_MS, self.history)

    def _store(self, channel, value, time):
        slot = int(self.position[channel])
        self.values[channel, slot] = _f32(value)
        self.times[channel, slot] = time
        return slot

    def _advance(self, channel, slot):
        self.position[channel] = (slot + 1) % self.history
        self.cursor += 1

    def smooth_coordinates(self, values, gains, time):
        """0x14004c480: smooth one coordinate per channel; returns the new values.

Coordinates with a negative gain are skipped and use no channel. Fewer than
MINIMUM_SAMPLES samples inside the window leave every value unchanged.
"""
        import numpy as np
        result = [_f32(v) for v in values]
        kept = [i for i, gain in enumerate(gains) if not _f32(gain) < 0]
        if not self.enabled or not kept or self.cursor < 0:
            return result
        length, window = self.history, self.window
        samples = np.zeros((length, len(kept)), dtype=np.float32)
        sample_times = np.zeros(length, dtype=np.float32)
        count = 0
        for column, index in enumerate(kept):
            channel = self.cursor
            slot = self._store(channel, result[index], time)
            for age in range(length):
                ring = (length - age + slot) % length
                if time - int(self.times[channel, ring]) < window:
                    samples[length - age - 1, column] = self.values[channel, ring]
                    if column == 0:
                        sample_times[length - age - 1] = _f32(int(self.times[channel, ring]))
                        count += 1
            self._advance(channel, slot)
        if count < MINIMUM_SAMPLES:
            return result
        recent = samples[length - count:]
        times = sample_times[length - count:]
        gain = np.array([_f32(gains[i]) for i in kept], dtype=np.float32)
        smoothed = polynomial_blend(recent, times, gain)
        for column, index in enumerate(kept):
            result[index] = smoothed[column]
        return result

    def smooth_scalar(self, value, time, gain, low_floor, high_cap, initial_high):
        """0x14004bf60: adaptive exponential average of one value; returns the new value."""
        value = _f32(value)
        gain = _f32(gain)
        if not self.enabled or not gain >= 0 or self.cursor < 0:
            return value
        channel, length, window = self.cursor, self.history, self.window
        slot = int(self.position[channel])
        if not self.initialised[channel]:
            if time > window:
                self.initialised[channel] = True
            else:
                self.high[channel] = _f32(initial_high)
                self.low[channel] = _f32(SCALAR_DEFAULT_LOW)
        self._store(channel, value, time)
        initialised = bool(self.initialised[channel])
        count = sum(1 for age in range(length)
                    if initialised and time - int(self.times[channel, (length + slot - age) % length]) < window)
        if initialised and count > 0:
            largest = _f32(0)
            for age in range(1, count):
                difference = abs(_f32(value - self.values[channel, (length - age + slot) % length]))
                if difference < self.low[channel]:
                    self.low[channel] = difference if difference >= _f32(low_floor) else _f32(low_floor)
                if difference > self.high[channel]:
                    self.high[channel] = difference if _f32(high_cap) >= difference else _f32(high_cap)
                largest = _maxss(difference, largest)
            floor = _maxss(gain, _f32(SCALAR_GAIN_FLOOR))
            high, low = self.high[channel], self.low[channel]
            spread = _maxss(_minss(_f32(floor * largest), high), low)
            rate = 1000 // (window // count)
            sharpness = _f32(_f32(_divide(spread - low, high - low)) * _f32(2)) + _f32(SHARPNESS_FLOOR)
            step = _f32(SCALAR_DECAY) / _f32(rate)
            weight_sum = total = _f32(0)
            for offset in range(-(count - 1), 1):
                weight = _expf(_f32(_f32(offset) * step) * sharpness)
                weight_sum = _f32(weight_sum + weight)
                total = _f32(total + _f32(weight * self.values[channel, (length + slot + offset) % length]))
            value = _f32(total / weight_sum)
        self._advance(channel, slot)
        return value


def _divide(numerator, denominator):
    """DIVSS, NaN or infinity for a zero denominator as on the original."""
    import numpy as np
    with np.errstate(divide='ignore', invalid='ignore'):
        return _f32(numerator) / _f32(denominator)


def _maxss(first, second):
    """MAXSS first, second: the second operand when either is NaN."""
    return first if first > second else second


def _minss(first, second):
    return first if first < second else second


def index_prediction_row(count, degree):
    """First row of pinv(V) for V[i, j] = (i - count + 1) ** j: the newest-sample predictor."""
    import numpy as np
    offsets = np.arange(1 - count, 1, dtype=np.float64)
    vandermonde = offsets[:, None] ** np.arange(degree + 1)[None, :]
    return np.linalg.pinv(vandermonde)[0].astype(np.float32)


def time_residual(samples, times, degree):
    """Per-column squared residual of a least-squares polynomial in (t - t0) / 30."""
    import numpy as np
    scaled = np.array([_f32(_f32(t - times[0]) * _f32(TIME_SCALE)) for t in times], dtype=np.float32)
    design = np.array([[_f32(1) if power == 0 else _powf(t, power) for power in range(degree, -1, -1)]
                       for t in scaled], dtype=np.float32)
    coefficients = np.linalg.lstsq(design.astype(np.float64), samples.astype(np.float64), rcond=None)[0]
    fitted = (design.astype(np.float64) @ coefficients.astype(np.float32).astype(np.float64)).astype(np.float32)
    residual = (fitted - samples).astype(np.float32)
    squared = (residual * residual).astype(np.float32)
    return squared.astype(np.float64).sum(axis=0).astype(np.float32)


def polynomial_blend(samples, times, gains):
    """0x14004c480 model average over DEGREES for count x channels samples, oldest first."""
    import numpy as np
    samples = np.asarray(samples, dtype=np.float32)
    count, channels = samples.shape
    regularizer = _f32(RESIDUAL_REGULARIZER)
    accumulated = np.zeros(channels, dtype=np.float32)
    weight_sum = np.zeros(channels, dtype=np.float32)
    for position, degree in enumerate(DEGREES):
        row = index_prediction_row(count, degree)
        prediction = (row.astype(np.float64) @ samples.astype(np.float64)).astype(np.float32)
        residual = time_residual(samples, times, degree)
        prior = np.array([_expf(_f32(-position) * g) for g in gains], dtype=np.float32)
        scaled = (residual * _f32(channels)).astype(np.float32)
        weight = (prior * reciprocal((scaled + regularizer).astype(np.float32))).astype(np.float32)
        accumulated = (accumulated + (weight * prediction).astype(np.float32)).astype(np.float32)
        weight_sum = (weight_sum + weight).astype(np.float32)
    return (accumulated * reciprocal((weight_sum + regularizer).astype(np.float32))).astype(np.float32)


def reciprocal(values):
    """1 / values in float32, zero for zero as cv::divide does."""
    import numpy as np
    values = np.asarray(values, dtype=np.float32)
    with np.errstate(divide='ignore'):
        return np.where(values != 0, (1.0 / values.astype(np.float64)).astype(np.float32), np.float32(0))


def flip_y(records):
    """0x14007ac80: y = 1 - y for present records, marking them present."""
    one = _f32(1)
    for record in records.values():
        if record.present:
            record.y = float(one - _f32(record.y))


def table_entries(factors, table=TRACKER_TABLE):
    """0x14004a530 entries with the configured smoothing_factors (Head Tracker.cfg)."""
    return tuple((group, first, last, float(_f32(factors[column]))) for group, first, last, column in table)


def coordinate_gain(group, quality, factor):
    """0x14007f5a0: factor + (1 - quality) * 0.2, except for FDP group 15."""
    if group == UNSCALED_GROUP:
        return _f32(_f32(0) + _f32(factor))
    return _f32(_f32(_f32(_f32(1) - _f32(quality)) * _f32(QUALITY_GAIN)) + _f32(factor))


def smooth_records(landmark_filter, records, time, entries):
    """0x14007f5a0: smooth listed coordinates, then present non-negative qualities.

records maps (group, index) to LandmarkRecord and is updated in place; absent
keys behave as cleared records.
"""
    cells = []
    for group, first, last, factor in entries:
        for index in range(int(first), int(last) + 1):
            record = records.setdefault((group, index), LandmarkRecord())
            gain = coordinate_gain(group, record.quality, factor)
            cells.extend(((record, 'x', gain), (record, 'y', gain)))
    smoothed = landmark_filter.smooth_coordinates([getattr(r, field) for r, field, _ in cells],
                                                  [gain for _, _, gain in cells], time)
    for (record, field, _), value in zip(cells, smoothed):
        setattr(record, field, float(value))
    gain, low, high, initial_high = QUALITY_SMOOTHING
    for group in sorted(GROUP_SIZES):
        for index in range(1, GROUP_SIZES[group] + 1):
            record = records.get((group, index))
            if record is None or not record.present or _f32(0) > _f32(record.quality):
                continue
            record.quality = float(landmark_filter.smooth_scalar(record.quality, time, gain, low, high,
                                                                 initial_high))
