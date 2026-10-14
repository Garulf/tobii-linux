"""A scripted Eye Tracker 5 and a scripted user, for trying the setup flow without hardware.

The device side answers the commands the setup flow sends with the semantics
recovered from the runtime and the Tobii client (docs/gaze-calibration.md,
docs/display-setup.md); it is NOT recorded device traffic. While gaze is
subscribed it streams about 30 messages a second carrying the columns the
flow reads: per-eye validity and 2D gaze point, the relative eye positions
(3, 9) and the user position guide (37 to 40).

The user sits too far back for the first seconds of eye positioning, then
leans in to the middle of the track box, and looks at whichever calibration
target is the first one of the current stage not collected yet, so the
gaze-driven target choice of the calibration finds it.
"""
from dataclasses import dataclass
import math
import random
import struct
import threading
import time

from . import gaze
from . import gaze_device as gd
from . import ttp_commands as tc
from .calibration import INTERACTIVE_POINTS, STAGES
from .display_setup import DisplayArea, ET5_MOUNTING
from .gaze_device import GazeCommand
from .ttp import Decoder, Message, fields
from .ttp_fields import (array_fields, encode_payload, fixed42_field, float32, struct_field,
                         triple_fixed42_fields, uint32_field, value)

ALBEDO_AREA = DisplayArea((-296.6200866699219, 326.0040588378906, 111.8187484741211),
                          (300.3799133300781, 326.0040588378906, 111.8187484741211),
                          (-296.6200866699219, 10.267330169677734, -3.100020170211792))
UNMAPPED_AREA = DisplayArea((0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
DISPLAY_AREA_TRAILER = 0x3039
STATE_ERROR = 0x20000508
OPERATION_FAILED = 0x20000407
GAZE_PERIOD_S = 1 / 30
COLLECT_S = 0.7
COMPUTE_S = 0.4
SEATED_AFTER_S = 9.0
LEAN_IN_S = 1.0
FAR_Z = 0.85
IDEAL_Z = 0.5
EYE_SPACING = 0.12
EYE_INVALID = 4


@dataclass
class SimulatedUser:
    """Where the user sits (normalized track box, z 0.5 at the ideal distance) and where they look.

From the first time gaze is subscribed the user sits too far back until the
last second of `seated_after_s`, then leans in to the ideal distance.
"""
    seated_after_s: float = SEATED_AFTER_S
    start: float | None = None
    present: bool = True

    def sit_down(self, now):
        if self.start is None:
            self.start = now

    def head(self, now):
        elapsed = now - (now if self.start is None else self.start)
        settle = min(1.0, max(0.0, elapsed - (self.seated_after_s - LEAN_IN_S)) / LEAN_IN_S)
        sway = 0.01 * math.sin(now * 1.3)
        return 0.5 + sway, 0.5 + 0.6 * sway, FAR_Z - (FAR_Z - IDEAL_Z) * settle


def calibration_blob(points, calibration_id):
    header = struct.pack('<5I', 110, 84, 1, 0, calibration_id) + b'human'.ljust(64, b'\0')
    table = b''.join(gd.CALIBRATION_RECORD.pack(x, y, x + 0.004, y, 1, x, y - 0.004, 1) for x, y in points)
    trailer_offset = 4 + len(header)
    return struct.pack('<I', trailer_offset) + header + struct.pack('<Ii', 0x5A5A5A5A, len(points)) + table


def stimulus_quality(points, weak_points=()):
    def eye(point):
        weak = point in weak_points
        return gd.EyeQuality(int(gd.PointStatus.VALID_AND_USED), 0.06 if weak else 0.008, 0.05 if weak else 0.01)
    return [gd.StimulusPoint((x, y, 0.0), eye((x, y)), eye((x, y))) for x, y in points]


class SimulatedTracker:
    """A channel (write, read) that behaves like the tracker for the setup flow.

mapped=False starts with no display area stored, as a new installation;
fail_compute_at makes the nth compute fail as "all points failed";
weak_points get a poor quality in 1120 so the result offers to improve them.
"""

    def __init__(self, mapped=True, calibration_id=0x3B9ACA07, fail_compute_at=(), weak_points=(),
                 user=None, seed=7, collect_s=COLLECT_S, compute_s=COMPUTE_S):
        self.area = ALBEDO_AREA if mapped else UNMAPPED_AREA
        self.calibration_id = calibration_id
        self.blob = calibration_blob(INTERACTIVE_POINTS, calibration_id)
        self.fail_compute_at = set(fail_compute_at)
        self.weak_points = tuple(weak_points)
        self.collect_s, self.compute_s = collect_s, compute_s
        self.user = user or SimulatedUser()
        self.random = random.Random(seed)
        self.requests = Decoder()
        self.lock = threading.Lock()
        self.outgoing = []
        self.log = []
        self.calibrating = False
        self.collected = []
        self.computes = 0
        self.subscribed = False
        self.next_gaze = 0.0
        self.collecting = (None, 0.0)

    def commands(self):
        return [message.command for message in self.log]

    def write(self, data):
        for request in self.requests.feed(data):
            self.log.append(request)
            self.handle(request)

    def read(self, timeout_ms=100):
        deadline = time.monotonic() + timeout_ms / 1000
        while True:
            now = time.monotonic()
            self._stream(now)
            with self.lock:
                ready = [item for item in self.outgoing if item[0] <= now]
                if ready:
                    self.outgoing.remove(ready[0])
                    return ready[0][1]
            if now >= deadline:
                return b''
            time.sleep(min(0.005, deadline - now))

    def _send(self, message, delay_s=0.0):
        with self.lock:
            self.outgoing.append((time.monotonic() + delay_s, message.encode()))
            self.outgoing.sort(key=lambda item: item[0])

    def reply(self, request, items=(), status=0, delay_s=0.0):
        payload = encode_payload(list(items)) if items else b''
        self._send(Message(tc.RESPONSE_KIND, request.request_id, 0, request.command, status, payload), delay_s)

    def handle(self, request):
        handler = self.HANDLERS.get(request.command)
        if handler is None:
            self.reply(request)
        else:
            handler(self, request)

    def _status(self, request):
        self.reply(request, (struct_field(2, gd.STATUS_STRUCT), uint32_field(gd.STATUS_CALIBRATION_ID),
                             tc.string_field(str(self.calibration_id))))

    def _display_area(self, request):
        corners = [f for corner in self.area.corners for f in triple_fixed42_fields(corner)]
        self.reply(request, corners + array_fields(0, DISPLAY_AREA_TRAILER))

    def _set_display_area(self, request):
        numbers = [v for v in (value(f) for f in fields(request.payload)) if isinstance(v, float)]
        self.area = DisplayArea.from_floats(numbers)
        self.reply(request)

    def _mounting(self, request):
        m = ET5_MOUNTING
        self.reply(request, [uint32_field(m.guides), fixed42_field(m.width_mm), fixed42_field(m.angle_deg)]
                   + triple_fixed42_fields(m.external_offset_mm) + triple_fixed42_fields(m.internal_offset_mm))

    def _subscribe(self, request):
        self.subscribed = request.command == tc.Command.SUBSCRIBE_STREAM
        if self.subscribed:
            self.user.sit_down(time.monotonic())
        self.reply(request)

    def _retrieve(self, request):
        self.reply(request, (tc.blob_field(self.blob),))

    def _stimulus(self, request):
        points = [r.point for r in gd.calibration_points(self.blob)]
        self.reply(request, gd.stimulus_points_payload(stimulus_quality(points, self.weak_points)))

    def _start(self, request):
        self.reply(request, status=STATE_ERROR if self.calibrating else 0)
        self.calibrating = True

    def _stop(self, request):
        self.reply(request, status=0 if self.calibrating else STATE_ERROR)
        self.calibrating = False

    def _clear(self, request):
        self.collected = []
        self.reply(request)

    def _point(self, request):
        x, y = (value(f) for f in fields(request.payload)[:2])
        return float32(x), float32(y)

    def _collect(self, request):
        if not self.calibrating:
            self.reply(request, status=STATE_ERROR)
            return
        point = self._point(request)
        self.collected.append(point)
        self.collecting = (point, time.monotonic() + self.collect_s)
        self.reply(request, delay_s=self.collect_s)

    def _discard(self, request):
        point = self._point(request)
        if point in self.collected:
            self.collected.remove(point)
        self.reply(request)

    def _compute(self, request):
        self.computes += 1
        if self.computes in self.fail_compute_at or not self.calibrating:
            self.reply(request, status=OPERATION_FAILED, delay_s=self.compute_s)
            return
        self.calibration_id = (self.calibration_id * 31 + len(self.collected)) & 0xFFFFFFFF
        self.blob = calibration_blob(self.collected, self.calibration_id)
        self.reply(request, delay_s=self.compute_s)

    def _apply(self, request):
        blob = value(fields(request.payload)[0])
        if self.calibrating:
            self.collected += [r.point for r in gd.calibration_points(blob)]
        else:
            self.blob, self.calibration_id = blob, gd.blob_calibration_id(blob)
        self.reply(request)

    HANDLERS = {
        tc.Command.GET_DEVICE_STATUS: _status,
        tc.Command.GET_DISPLAY_AREA: _display_area,
        tc.Command.GET_GEOMETRY_MOUNTING: _mounting,
        tc.Command.SUBSCRIBE_STREAM: _subscribe,
        tc.Command.UNSUBSCRIBE_STREAM: _subscribe,
        GazeCommand.SET_DISPLAY_AREA: _set_display_area,
        GazeCommand.CALIBRATION_RETRIEVE: _retrieve,
        GazeCommand.GET_CALIBRATION_STIMULUS_POINTS: _stimulus,
        GazeCommand.CALIBRATION_START: _start,
        GazeCommand.CALIBRATION_STOP: _stop,
        GazeCommand.CALIBRATION_CLEAR: _clear,
        GazeCommand.CALIBRATION_COLLECT_2D: _collect,
        GazeCommand.CALIBRATION_DISCARD_2D: _discard,
        GazeCommand.CALIBRATION_COMPUTE_AND_APPLY: _compute,
        GazeCommand.CALIBRATION_APPLY: _apply,
    }

    def looking_at(self):
        """The target being collected, else the first of the current stage not collected yet, else the centre."""
        point, until = self.collecting
        if point is not None and time.monotonic() < until:
            return point
        targets = [tuple(float32(c) for c in p) for p in INTERACTIVE_POINTS]
        for stage in STAGES:
            remaining = [targets[i] for i in stage if targets[i] not in self.collected]
            if remaining:
                return remaining[0]
        return (0.5, 0.5)

    def _stream(self, now):
        if not self.subscribed or now < self.next_gaze:
            return
        self.next_gaze = now + GAZE_PERIOD_S
        self._send(gaze.gaze_stream_message(self.gaze_columns(now)))

    def gaze_columns(self, now):
        timestamp = gaze.int64_column(int(now * 1e6))
        if not self.user.present:
            return {1: timestamp, 7: gaze.uint32_column(EYE_INVALID), 13: gaze.uint32_column(EYE_INVALID),
                    38: gaze.uint32_column(0), 40: gaze.uint32_column(0)}
        x, y, z = self.user.head(now)
        left, right = (x + EYE_SPACING / 2, y, z), (x - EYE_SPACING / 2, y, z)
        target = self.looking_at()
        point = tuple(c + self.random.gauss(0.0, 0.002) for c in target)
        return {1: timestamp, 5: gaze.vec2_column(point), 11: gaze.vec2_column(point),
                7: gaze.uint32_column(0), 13: gaze.uint32_column(0),
                3: gaze.vec3_column(left), 9: gaze.vec3_column(right),
                29: gaze.uint32_column(1), 30: gaze.uint32_column(1),
                37: gaze.vec3_column(left), 38: gaze.uint32_column(1),
                39: gaze.vec3_column(right), 40: gaze.uint32_column(1)}
