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
"""
from dataclasses import dataclass
from enum import Enum

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


class Status(Enum):
    CANNOT_TRACK_EYES = 'cannot-track-eyes'
    MOVE_CLOSER = 'move-closer'
    LEAN_BACK = 'lean-back'
    MOVE_RIGHT = 'move-right'
    MOVE_LEFT = 'move-left'
    MOVE_DOWN = 'move-down'
    MOVE_UP = 'move-up'
    VALID = 'valid'


HINTS = {Status.MOVE_CLOSER: 'Move closer', Status.LEAN_BACK: 'Lean back', Status.MOVE_RIGHT: 'Move right',
         Status.MOVE_LEFT: 'Move left', Status.MOVE_DOWN: 'Move down', Status.MOVE_UP: 'Move up',
         Status.VALID: '', Status.CANNOT_TRACK_EYES: ''}
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
    return GuideSample(g.timestamp,
                       tuple(g.left_position_guide) if g.left_position_guide_validity == 1 else None,
                       tuple(g.right_position_guide) if g.right_position_guide_validity == 1 else None)


def mirrored(position):
    return 1.0 - position[0], position[1]


def eye_size(distance):
    if distance <= MIN_DISTANCE:
        return BIG_EYE_SIZE
    if distance >= MAX_DISTANCE:
        return SMALL_EYE_SIZE
    if distance > FAR_EDGE:
        return int(-(-(DEFAULT_EYE_SIZE - FAR_SLOPE * (distance - FAR_EDGE)) // 1))
    if distance < CLOSE_EDGE:
        return int(-(-(DEFAULT_EYE_SIZE + CLOSE_SLOPE * (CLOSE_EDGE - distance)) // 1))
    return DEFAULT_EYE_SIZE


def eye_grey(size):
    """EyeSizeToEyeColor before darkening: white at size 50, dark grey at 25, 100 or no size."""
    if size in (0, BIG_EYE_SIZE, SMALL_EYE_SIZE):
        return DARK_GREY
    if size == DEFAULT_EYE_SIZE:
        return 255
    slope = 205.0 / (SMALL_EYE_SIZE if size < DEFAULT_EYE_SIZE else -DEFAULT_EYE_SIZE)
    return int(size * slope + 255.0 - slope * DEFAULT_EYE_SIZE) & 0xFF


def eye_opacity(position):
    """DarkenEyeColor: opaque inside the box, fading out over the last 0.2 towards an edge."""
    if position is None:
        return 1.0
    x, y = position
    x = 0.0 if x < EDGE_MIN else 1.0 if x > EDGE_MAX else (x - EDGE_MIN) / (EDGE_MAX - EDGE_MIN)
    margin = min(x, 1.0 - x, y, 1.0 - y)
    if margin <= 0.0 or margin >= 1.0:
        return 0.0
    return 1.0 if margin >= FADE_MARGIN else margin / FADE_MARGIN


def extrapolated(history):
    """The newest position, or a straight-line guess from the last two seen when the newest is missing."""
    if not history:
        return None
    if history[-1] is not None:
        return history[-1]
    seen = [(i, p) for i, p in enumerate(history) if p is not None]
    if not seen:
        return None
    if len(seen) == 1:
        return seen[0][1]
    (i2, p2), (i1, p1) = seen[-2], seen[-1]
    steps = (len(history) - 1 - i1) / (i1 - i2)
    guess = tuple(a + (a - b) * steps for a, b in zip(p1, p2))
    return tuple(min(1.0, max(0.0, c)) for c in guess)


class EyesPositioning:
    """EyesPositioningParametersCalculator: eye points, size and status from guide samples."""

    def __init__(self):
        self.left_history, self.right_history, self.distances = [], [], []
        self.left = self.right = None
        self.size = 0
        self.status = Status.CANNOT_TRACK_EYES

    def update(self, sample: GuideSample):
        for history, eye in ((self.left_history, sample.left), (self.right_history, sample.right)):
            history.append(None if eye is None else mirrored(eye))
            del history[:-EXTRAPOLATION_SAMPLES]
        self.left, self.right = extrapolated(self.left_history), extrapolated(self.right_history)
        depths = [eye[2] for eye in (sample.left, sample.right) if eye is not None]
        if depths:
            self.distances.append(sum(depths) / len(depths))
            del self.distances[:-DISTANCE_SAMPLES]
            self.size = eye_size(sum(self.distances) / len(self.distances))
        self.status = self._status()
        return self.status

    def _status(self):
        eyes = [e for e in (self.left, self.right) if e is not None]
        if not eyes:
            return Status.CANNOT_TRACK_EYES
        if self.size <= SMALL_EYE_SIZE:
            return Status.MOVE_CLOSER
        if self.size >= BIG_EYE_SIZE:
            return Status.LEAN_BACK
        if any(x <= EDGE_MIN for x, _ in eyes):
            return Status.MOVE_RIGHT
        if any(x >= EDGE_MAX for x, _ in eyes):
            return Status.MOVE_LEFT
        if any(y <= EDGE_MIN for _, y in eyes):
            return Status.MOVE_DOWN
        if any(y >= EDGE_MAX for _, y in eyes):
            return Status.MOVE_UP
        return Status.VALID

    def eyes(self):
        """What the page draws: per eye its mirrored point, grey level and opacity; size in percent."""
        def eye(point):
            if point is None:
                return None
            return {'x': point[0], 'y': point[1], 'grey': eye_grey(self.size), 'opacity': eye_opacity(point)}
        return {'left': eye(self.left), 'right': eye(self.right), 'size': self.size}


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
        self.calculator = EyesPositioning()
        self.hint = ''
        self.previous = Status.CANNOT_TRACK_EYES
        self.pending = 0
        self.lost_since_us = None
        self.start_us = None
        self.present_since_us = None
        self.absent_since_us = None

    @property
    def good(self):
        return self.previous == Status.VALID

    def update(self, sample: GuideSample):
        status = self.calculator.update(sample)
        now = sample.timestamp_us
        if self.start_us is None:
            self.start_us = now
        self._track_presence(sample, now)
        if status == Status.VALID:
            self._settle(status)
        elif status == Status.CANNOT_TRACK_EYES:
            self._eyes_lost(now)
        else:
            self.lost_since_us = None
            if self.previous == Status.VALID:
                self._after_frames(status, HINT_DELAY_FROM_VALID)
            else:
                self._settle(status)
        return self.hint

    def _settle(self, status):
        self.previous, self.pending, self.lost_since_us = status, 0, None
        self.hint = HINTS[status]

    def _after_frames(self, status, frames):
        self.pending += 1
        if self.pending >= frames:
            self._settle(status)

    def _eyes_lost(self, now):
        if self.previous != Status.CANNOT_TRACK_EYES:
            self._after_frames(Status.CANNOT_TRACK_EYES, LOST_DELAY_FRAMES)
            if self.previous == Status.CANNOT_TRACK_EYES:
                self.lost_since_us = now
            return
        if self.lost_since_us is None:
            self.lost_since_us = now
        lost_ms = (now - self.lost_since_us) / 1000
        self.hint = ADJUST_SEATING if lost_ms >= ADJUST_SEATING_MS else ARE_YOU_THERE if lost_ms >= ARE_YOU_THERE_MS else ''

    def _track_presence(self, sample, now):
        if sample.left is not None or sample.right is not None:
            self.absent_since_us = None
            if self.present_since_us is None:
                self.present_since_us = now
        elif self.present_since_us is not None:
            if self.absent_since_us is None:
                self.absent_since_us = now
            elif (now - self.absent_since_us) / 1000 > ABSENCE_GRACE_MS:
                self.present_since_us = None

    def presence_ms(self, now):
        return 0.0 if self.present_since_us is None else (now - self.present_since_us) / 1000

    def ready_to_advance(self, now):
        """Tobii's automatic advance: shown for 6 s, present for over 3 s, and in a good position."""
        if self.start_us is None:
            return False
        return ((now - self.start_us) / 1000 >= AUTO_ADVANCE_AFTER_MS
                and self.presence_ms(now) > PRESENCE_NEEDED_MS and self.good)

    def view(self):
        return {**self.calculator.eyes(), 'hint': self.hint, 'good': self.good}
