"""Where the user's eyes are in the track box, and the hint the positioning screen shows.

A port of Tobii's EyesPositioningParametersCalculator and the hint and
auto-advance rules of EyesPositioningViewModel (Tobii.Configuration.Common,
see docs/setup-flow.md). Input is the device's user position guide (gaze
columns 37 to 40): per eye a position normalized to the track box, x and y
in 0..1 across, z in 0..1 from near to far.

On screen the picture is mirrored like a mirror: x becomes 1 - x. Each eye
is drawn with a size from the distance (50 at a good distance, up to 100
when too close, down to 25 when too far) and a grey level and opacity from
that size and from how near the eye is to the edge of the box.

The calculator and the rules are crates/tobii-gaze/src/positioning.rs.
"""
from dataclasses import dataclass
from enum import Enum

from ._native import eye_positioning as _ep

MIN_DISTANCE = 0.2
MAX_DISTANCE = 0.8
FAR_EDGE = 0.65
CLOSE_EDGE = 0.4
DEFAULT_EYE_SIZE = 50
SMALL_EYE_SIZE = 25
BIG_EYE_SIZE = 100
FAR_SLOPE = 50 / 0.3
CLOSE_SLOPE = 250.0
EDGE_MIN = 0.1
EDGE_MAX = 0.9
EXTRAPOLATION_SAMPLES = 11
DISTANCE_SAMPLES = 3
FADE_MARGIN = 0.2
DARK_GREY = 50

HINT_DELAY_FROM_VALID = 11
LOST_DELAY_FRAMES = 49
ARE_YOU_THERE_MS = 2000
ADJUST_SEATING_MS = 5000
AUTO_ADVANCE_AFTER_MS = 6000
PRESENCE_NEEDED_MS = 3000
ABSENCE_GRACE_MS = 500

Status = Enum('Status', [(value.upper().replace('-', '_'), value) for value, _ in _ep.STATUSES], module=__name__)
HINTS = {Status(value): hint for value, hint in _ep.STATUSES}
ARE_YOU_THERE = 'Are you there?'
ADJUST_SEATING = 'Adjust your seating position'


@dataclass(frozen=True)
class GuideSample:
    """One user position guide record: device time and each eye's normalized (x, y, z), None when not valid."""
    timestamp_us: int
    left: tuple | None
    right: tuple | None


def guide_sample(g):
    """GuideSample from a gaze.DeviceGaze (validity column 38/40 equal to 1 marks a valid eye)."""
    return GuideSample(*_ep.guide_sample(g))


def mirrored(position):
    return _ep.mirrored(position)


def eye_size(distance):
    return _ep.eye_size(distance)


def eye_grey(size):
    """EyeSizeToEyeColor before darkening: white at size 50, dark grey at 25, 100 or no size."""
    return _ep.eye_grey(size)


def eye_opacity(position):
    """DarkenEyeColor: opaque inside the box, fading out over the last 0.2 towards an edge."""
    return _ep.eye_opacity(position)


def extrapolated(history):
    """The newest position, or a straight-line guess from the last two seen when the newest is missing."""
    return _ep.extrapolated(history)


class EyesPositioning:
    """EyesPositioningParametersCalculator: eye points, size and status from guide samples."""

    def __init__(self, native=None):
        self.native = native or _ep.EyesPositioning()

    def update(self, sample: GuideSample):
        return Status(self.native.update(sample))

    @property
    def left_history(self):
        return self.native.left_history

    @property
    def right_history(self):
        return self.native.right_history

    @property
    def distances(self):
        return self.native.distances

    @property
    def left(self):
        return self.native.left

    @property
    def right(self):
        return self.native.right

    @property
    def size(self):
        return self.native.size

    @property
    def status(self):
        return Status(self.native.status)

    def eyes(self):
        """What the page draws: per eye its mirrored point, grey level and opacity; size in percent."""
        return self.native.eyes()


class PositioningGuide:
    """EyesPositioningViewModel's hint and advance rules on top of EyesPositioning.

A hint for a new problem appears at once, except that after a good
position a problem has to last 11 samples (49 for lost eyes) before it
replaces the empty hint, so a blink or a short glance does not flash a
message. When the eyes stay lost, "Are you there?" follows after 2 s and
"Adjust your seating position" after 5 s. The screen advances by itself
once it has been up 6 s, the user has been present for more than 3 s in a
row (gaps under 0.5 s do not count as absence) and the position is good.
"""

    def __init__(self):
        self.native = _ep.PositioningGuide()

    @property
    def calculator(self):
        """The calculator's state after the latest sample."""
        return EyesPositioning(self.native.calculator)

    @property
    def hint(self):
        return self.native.hint

    @property
    def previous(self):
        return Status(self.native.previous)

    @property
    def pending(self):
        return self.native.pending

    @property
    def lost_since_us(self):
        return self.native.lost_since_us

    @property
    def start_us(self):
        return self.native.start_us

    @property
    def present_since_us(self):
        return self.native.present_since_us

    @property
    def absent_since_us(self):
        return self.native.absent_since_us

    @property
    def good(self):
        return self.native.good

    def update(self, sample: GuideSample):
        return self.native.update(sample)

    def presence_ms(self, now):
        return self.native.presence_ms(now)

    def ready_to_advance(self, now):
        """Tobii's automatic advance: shown for 6 s, present for over 3 s, and in a good position."""
        return self.native.ready_to_advance(now)

    def view(self):
        return {**self.native.eyes(), 'hint': self.native.hint, 'good': self.native.good}
