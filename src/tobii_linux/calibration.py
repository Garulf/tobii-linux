"""User gaze calibration of the Eye Tracker 5: backup, restore and the Tobii Experience sequence.

The device computes gaze and keeps the active calibration itself (see
docs/gaze-device.md). Calibrating means driving its calibration commands in
the order Tobii's configuration app does through Stream Engine and the
platform runtime; docs/gaze-calibration.md has the evidence for every step.

backup_calibration and verify_backup only read. restore_calibration and
Calibrator write to the device; DryRunSession wraps a session so that only
read commands reach the device and every other command is reported instead.
"""
from dataclasses import dataclass
import hashlib
import math
from pathlib import Path
import threading
import time

from . import gaze_device as gd
from .gaze_device import Eye, GazeCommand, LocalCalibration
from .ttp import Message
from .ttp_fields import float32
from .ttp_commands import (Command, NOTIFICATION_KIND, RESPONSE_KIND, STREAM_DATA_KIND, Stream,
                           header_only_request, subscribe_stream, unsubscribe_stream, wire_stream_id)

COMMAND_TIMEOUT_S = 3.0
TRANSFER_TIMEOUT_S = 15.0
COLLECT_TIMEOUT_S = 15.0
COMPUTE_TIMEOUT_S = 30.0

INTERACTIVE_POINTS = ((0.5, 0.5), (0.1, 0.9), (0.5, 0.1), (0.9, 0.9), (0.1, 0.1), (0.5, 0.9), (0.9, 0.1))
STAGES = ((0,), (1, 2, 3), (4, 5, 6))

MAX_CALIBRATION_AREA_MM = (600.0, 340.0)
MAX_DIAGONAL_MM = 690.0
MAX_HEIGHT_MM = 350.0
DISPLAY_TOLERANCE_MM = 10.0

ZONE_RADIUS_FACTOR = 0.45
FOCUS_CHANGE_MS = 150.0
GAZE_LOST_MS = 250.0
EYE_VALID_BELOW = 2
READ_SLICE_MS = 100

READ_COMMANDS = frozenset(gd.READ_ONLY | {Command.GET_DEVICE_STATUS, Command.GET_DISPLAY_AREA,
                                          Command.GET_DEVICE_INFO, Command.GET_ENABLED_EYE})


class CalibrationError(RuntimeError):
    pass


class CalibrationFailed(CalibrationError):
    """A calibration step failed; the device was told to stop and nothing was applied."""
    def __init__(self, step, cause):
        super().__init__(f'{step} failed: {cause}')
        self.step = step
        self.cause = cause


def calibration_area(display_mm):
    """Normalized (x, y, width, height) the targets span, as CalibrationAreaCalculator chooses it.

Displays up to 690 mm diagonal and 350 mm height (10 mm tolerance) use the
whole screen; larger ones use a centred 600 x 340 mm area.
"""
    width, height = display_mm
    if math.hypot(width, height) < MAX_DIAGONAL_MM + DISPLAY_TOLERANCE_MM \
            and height < MAX_HEIGHT_MM + DISPLAY_TOLERANCE_MM:
        return 0.0, 0.0, 1.0, 1.0
    area_w = min(1.0, MAX_CALIBRATION_AREA_MM[0] / width)
    area_h = min(1.0, MAX_CALIBRATION_AREA_MM[1] / height)
    return (1.0 - area_w) / 2, (1.0 - area_h) / 2, area_w, area_h


def calibration_targets(display_mm=None, points=INTERACTIVE_POINTS):
    """Targets in normalized display coordinates, scaled into the calibration area like ScalePoints."""
    x0, y0, w, h = calibration_area(display_mm) if display_mm else (0.0, 0.0, 1.0, 1.0)
    return tuple((float32(x0 + x * w), float32(y0 + y * h)) for x, y in points)


def display_size_mm(values):
    """Width and height in mm from a 1430 reply: top-left, top-right, bottom-left corners."""
    numbers = [v for v in values if isinstance(v, float)]
    if len(numbers) < 9:
        raise CalibrationError('Display area reply holds fewer than three corners')
    top_left, top_right, bottom_left = numbers[0:3], numbers[3:6], numbers[6:9]
    return top_right[0] - top_left[0], math.dist(top_left, bottom_left)


def read_display_size(session):
    reply = session.call(lambda rid: header_only_request(rid, Command.GET_DISPLAY_AREA), timeout_s=COMMAND_TIMEOUT_S)
    return display_size_mm(session.reply_values(reply))


def read_device_calibration_id(session) -> int:
    reply = session.call(lambda rid: header_only_request(rid, Command.GET_DEVICE_STATUS), timeout_s=COMMAND_TIMEOUT_S)
    return gd.device_calibration_id(session.reply_values(reply)) or 0


def retrieve_blob(session) -> bytes:
    reply = session.call(lambda rid: gd.header_only(rid, GazeCommand.CALIBRATION_RETRIEVE),
                         timeout_s=TRANSFER_TIMEOUT_S)
    blobs = [bytes(v) for v in session.reply_values(reply) if isinstance(v, (bytes, bytearray))]
    if len(blobs) != 1:
        raise CalibrationError('Calibration reply must hold one blob')
    return blobs[0]


def read_stimulus_points(session):
    reply = session.call(lambda rid: gd.header_only(rid, GazeCommand.GET_CALIBRATION_STIMULUS_POINTS),
                         timeout_s=COMMAND_TIMEOUT_S)
    return gd.stimulus_points(session.reply_values(reply))


def backup_calibration(session) -> LocalCalibration:
    """Read-only copy of the device calibration with the id the runtime would store with it.

The id is read before and after the transfer so a calibration that changes
in between is reported instead of saved with the wrong id.
"""
    before = read_device_calibration_id(session)
    blob = retrieve_blob(session)
    after = read_device_calibration_id(session)
    if before != after:
        raise CalibrationError(f'Device calibration id changed during backup ({before:#x} -> {after:#x})')
    return LocalCalibration(after, blob)


def save_backup(path, calibration: LocalCalibration, overwrite=False):
    """Write calibration.setpm bytes; an existing file is kept unless overwrite is set."""
    path = Path(path)
    if path.exists() and not overwrite:
        raise FileExistsError(f'{path} exists')
    temporary = path.with_name(path.name + '.partial')
    temporary.write_bytes(gd.calibration_setting(calibration))
    temporary.replace(path)
    return path


def load_backup(path) -> LocalCalibration:
    return gd.parse_calibration_setting(Path(path).read_bytes())


def sha256(data):
    return hashlib.sha256(data).hexdigest()


@dataclass(frozen=True)
class BackupCheck:
    file_id: int
    device_id: int
    file_blob_sha256: str
    device_blob_sha256: str

    @property
    def ids_match(self):
        return self.file_id == self.device_id

    @property
    def blob_matches(self):
        return self.file_blob_sha256 == self.device_blob_sha256

    @property
    def ok(self):
        return self.ids_match and self.blob_matches


def verify_backup(session, calibration: LocalCalibration) -> BackupCheck:
    """Compare a backup with the device's current id and blob; reads only."""
    current = backup_calibration(session)
    return BackupCheck(calibration.calibration_id, current.calibration_id,
                       sha256(calibration.blob), sha256(current.blob))


def restore_calibration(session, calibration: LocalCalibration) -> int:
    """Apply a backup (1110) and confirm the device now reports its id; returns that id.

This is what the runtime does at start when its calibration.setpm id differs
from the device ("Restoring calibration from disk"). The device takes the id
from the blob itself.
"""
    session.call(lambda rid: gd.calibration_apply(rid, calibration.blob), timeout_s=TRANSFER_TIMEOUT_S)
    device_id = read_device_calibration_id(session)
    if device_id != calibration.calibration_id:
        raise CalibrationError(f'Device reports calibration id {device_id:#x} after restoring '
                               f'{calibration.calibration_id:#x}')
    return device_id


class CalibrationUI:
    """Callbacks a display implements to drive Calibrator; the defaults do nothing.

Calibrator runs each callback on a worker thread while it keeps reading the
device, so a callback may block but must not use the device session.
"""

    def show_point(self, index, x, y):
        """Show target index at normalized (x, y) and return once the user's eye is settled on it."""

    def point_done(self, index, accepted):
        """Collection for the target ended; accepted is False when the data was discarded."""

    def stage_done(self, stage):
        """The device computed and applied the calibration from all targets so far."""


@dataclass(frozen=True)
class GazeSample:
    timestamp_us: int
    point: tuple | None


def gaze_sample(message):
    """Mean of the valid per-eye display points, as CalibrationProcessViewModel.OnGazeData forms it."""
    from .gaze import gaze_message
    g = gaze_message(message)
    if g is None:
        return None
    eyes = [p for p, validity in ((g.left_gaze_point_2d, g.left_validity), (g.right_gaze_point_2d, g.right_validity))
            if validity < EYE_VALID_BELOW and p != (0.0, 0.0)]
    point = (sum(p[0] for p in eyes) / len(eyes), sum(p[1] for p in eyes) / len(eyes)) if eyes else None
    return GazeSample(g.timestamp, point)


class FocusCheck:
    """Whether the user's gaze left a target while the device collected it.

Mirrors DetermineFocusedCalibrationTarget and CalibrationStateManager: gaze
selects the closest target of the stage within a zone radius of 0.45 times
half the closest target distance (in pixels); focus moves to another target
only after 150 ms on it, and 250 ms without valid gaze counts as the user
leaving. Either sets "focus changed during calibration" and the data is
discarded. The proximity modifier the Tobii client applies when the user
sits beyond the track box is not reproduced.
"""

    def __init__(self, screen_px=(2560, 1440)):
        self.screen_px = screen_px

    def _pixels(self, point):
        return point[0] * self.screen_px[0], point[1] * self.screen_px[1]

    def zone_radius(self, targets):
        pixels = [self._pixels(t) for t in targets]
        closest = min(math.dist(a, b) for i, a in enumerate(pixels) for b in pixels[i + 1:])
        return closest / 2 * ZONE_RADIUS_FACTOR

    def closest(self, point, targets, radius):
        gaze = self._pixels(point)
        distances = [(math.dist(gaze, self._pixels(t)), i) for i, t in enumerate(targets)]
        distance, index = min(distances)
        return index if distance < radius else -1

    def focus_changed(self, samples, targets, focused):
        """samples: GazeSample in time order; targets: the stage's uncalibrated targets; focused: index into targets."""
        if len(targets) < 2 or not samples:
            return False
        radius = self.zone_radius(targets)
        valid = [s for s in samples if s.point is not None]
        if not valid:
            return True
        last_valid = samples[0].timestamp_us
        last_focus = valid[0].timestamp_us
        for sample in samples:
            if (sample.timestamp_us - last_valid) / 1000 > GAZE_LOST_MS:
                return True
            if sample.point is None:
                continue
            last_valid = sample.timestamp_us
            index = self.closest(sample.point, targets, radius)
            if index == focused:
                last_focus = sample.timestamp_us
            elif index > -1 and (sample.timestamp_us - last_focus) / 1000 > FOCUS_CHANGE_MS:
                return True
        return (samples[-1].timestamp_us - last_valid) / 1000 > GAZE_LOST_MS


@dataclass(frozen=True)
class CalibrationOutcome:
    calibration: LocalCalibration
    stimulus_points: tuple
    points: tuple
    discarded: tuple


class Calibrator:
    """The Tobii Experience calibration (CalibrationProcessViewModel, 7-point flow) on a device session.

Device commands, in order: subscribe gaze, 1010 start, 1060 clear, optionally
1110 with the previous calibration (the client's "improve calibration"), then
per stage 1030 for each target (1080 for a target whose collection the user
looked away from, then 1030 again) and 1070 once the stage is complete, then
1020 stop and unsubscribe. Afterwards it reads the new calibration (1100),
its id (1490 key 7) and the per-target quality (1120). Any failure after 1010
sends 1020 and raises CalibrationFailed; nothing is applied on failure.

The UI callbacks block (a target animates for about 1.5 s), so they run on a
worker thread while this thread keeps reading the device. The device stops
sending, and stops answering requests for the rest of the USB session, once a
subscribed stream has gone unread for about two seconds (live, see
docs/gaze-calibration.md); the session must never pause reading while gaze is
subscribed.
"""

    def __init__(self, session, ui=None, targets=INTERACTIVE_POINTS, stages=STAGES, previous=None,
                 focus=None, max_attempts=3, subscribe_gaze=True, log=None):
        self.session = session
        self.ui = ui or CalibrationUI()
        self.targets = tuple(targets)
        self.stages = stages
        self.previous = previous
        self.focus = focus
        self.max_attempts = max_attempts
        self.subscribe_gaze = subscribe_gaze
        self.log = log or (lambda kind, **values: None)
        self.notifications = []
        self.discarded = []

    def _call(self, step, build, timeout_s=COMMAND_TIMEOUT_S):
        self.log('command', step=step)
        return self.session.call(build, timeout_s=timeout_s)

    def run(self) -> CalibrationOutcome:
        if self.subscribe_gaze:
            self._call('subscribe gaze', lambda rid: subscribe_stream(rid, Stream.GAZE))
        try:
            self._call('calibration start (1010)', lambda rid: gd.header_only(rid, GazeCommand.CALIBRATION_START))
        except Exception as error:
            self._unsubscribe()
            raise CalibrationFailed('calibration start (1010)', error) from error
        step = 'calibration clear (1060)'
        try:
            self._call(step, lambda rid: gd.header_only(rid, GazeCommand.CALIBRATION_CLEAR))
            if self.previous is not None:
                step = 'apply previous calibration (1110)'
                self._call(step, lambda rid: gd.calibration_apply(rid, self.previous.blob), TRANSFER_TIMEOUT_S)
            for number, stage in enumerate(self.stages):
                for index in stage:
                    step = f'target {index}'
                    self._collect(number, stage, index)
                step = f'compute stage {number} (1070)'
                self._call(step, lambda rid: gd.header_only(rid, GazeCommand.CALIBRATION_COMPUTE_AND_APPLY),
                           COMPUTE_TIMEOUT_S)
                self._while_reading(self.ui.stage_done, number)
            step = 'calibration stop (1020)'
            self._call(step, lambda rid: gd.header_only(rid, GazeCommand.CALIBRATION_STOP))
        except BaseException as error:
            self._stop_after_failure()
            if isinstance(error, Exception):
                raise CalibrationFailed(step, error) from error
            raise
        self._unsubscribe()
        return self._outcome()

    def _collect(self, stage_number, stage, index):
        x, y = self.targets[index]
        for attempt in range(1, self.max_attempts + 1):
            self._while_reading(self.ui.show_point, index, x, y)
            self.session.pending_streams.clear()
            started = time.monotonic()
            self._call(f'collect target {index} (1030)',
                       lambda rid: gd.calibration_collect_2d(rid, x, y, Eye.BOTH), COLLECT_TIMEOUT_S)
            self.log('collected', target=index, seconds=round(time.monotonic() - started, 3),
                     stream_messages=len(self.session.pending_streams))
            if not self._focus_changed(stage_number, stage, index):
                self._while_reading(self.ui.point_done, index, True)
                return
            self.discarded.append((index, attempt))
            self._call(f'discard target {index} (1080)',
                       lambda rid: gd.calibration_discard_2d(rid, x, y, Eye.BOTH))
            self._while_reading(self.ui.point_done, index, False)
        raise CalibrationError(f'target {index} lost focus {self.max_attempts} times')

    def _focus_changed(self, stage_number, stage, index):
        if self.focus is None or stage_number == 0:
            return False
        gaze_id = wire_stream_id(Stream.GAZE)
        samples = [s for s in (gaze_sample(m) for m in self.session.pending_streams
                               if m.kind == STREAM_DATA_KIND and m.command == gaze_id) if s is not None]
        samples.sort(key=lambda s: s.timestamp_us)
        targets = [self.targets[i] for i in stage]
        changed = self.focus.focus_changed(samples, targets, stage.index(index))
        self.log('focus', target=index, samples=len(samples), changed=changed)
        return changed

    def _while_reading(self, callback, *args):
        """Run a UI callback on a worker thread and keep reading the device until it returns."""
        outcome = {}

        def run():
            try:
                callback(*args)
            except BaseException as error:
                outcome['error'] = error
        worker = threading.Thread(target=run, name='calibration-ui', daemon=True)
        worker.start()
        while worker.is_alive():
            self._read_once()
            worker.join(0)
        if 'error' in outcome:
            raise outcome['error']

    def _read_once(self):
        """One bounded read; stream data is kept for the focus check, notifications are logged."""
        channel = getattr(self.session, 'channel', None)
        decoder = getattr(self.session, 'decoder', None)
        if channel is None or decoder is None:
            time.sleep(READ_SLICE_MS / 1000)
            return
        data = channel.read(timeout_ms=READ_SLICE_MS)
        for message in decoder.feed(data) if data else ():
            if message.kind == STREAM_DATA_KIND:
                self.session.pending_streams.append(message)
            elif message.kind == NOTIFICATION_KIND:
                self.notifications.append(message)
                self.log('notification', command=message.command, payload=message.payload.hex())

    def _stop_after_failure(self):
        try:
            self._call('calibration stop after failure (1020)',
                       lambda rid: gd.header_only(rid, GazeCommand.CALIBRATION_STOP))
        except Exception as error:
            self.log('stop_failed', error=str(error))
        self._unsubscribe()

    def _unsubscribe(self):
        if not self.subscribe_gaze:
            return
        try:
            self._call('unsubscribe gaze', lambda rid: unsubscribe_stream(rid, Stream.GAZE))
        except Exception as error:
            self.log('unsubscribe_failed', error=str(error))

    def _outcome(self):
        blob = retrieve_blob(self.session)
        calibration_id = read_device_calibration_id(self.session)
        stimulus = read_stimulus_points(self.session)
        points = gd.calibration_points(blob) if blob else ()
        return CalibrationOutcome(LocalCalibration(calibration_id, blob), stimulus, points, tuple(self.discarded))


def quality_report(stimulus_points):
    """Per-target status, bias and precision from 1120, in normalized display units."""
    return [{'target': [round(c, 4) for c in point.point[:2]],
             **{side: {'status': gd.PointStatus(eye.status).name.lower(),
                       'bias': round(eye.bias, 4), 'precision': round(eye.precision, 4)}
                for side, eye in (('left', point.left), ('right', point.right))}}
            for point in stimulus_points]


def points_report(points, screen_px):
    """Per calibration record of the blob: status and distance of each eye's mapped point from the target in pixels."""
    def offset(mapped, target):
        return round(math.hypot((mapped[0] - target[0]) * screen_px[0], (mapped[1] - target[1]) * screen_px[1]), 1)
    return [{'target': [round(c, 4) for c in p.point],
             'left': {'status': p.left_status.name.lower(), 'offset_px': offset(p.left_mapping, p.point)},
             'right': {'status': p.right_status.name.lower(), 'offset_px': offset(p.right_mapping, p.point)}}
            for p in points]


def describe_request(message):
    """A printable summary of a request: command, name and decoded fields (large blobs by size and digest)."""
    from .ttp import fields as message_fields
    from .ttp_fields import value
    try:
        name = GazeCommand(message.command).name
    except ValueError:
        try:
            name = Command(message.command).name
        except ValueError:
            name = 'UNKNOWN'
    decoded = []
    for field in message_fields(message.payload):
        item = value(field)
        if isinstance(item, (bytes, bytearray)) and len(item) > 64:
            item = {'bytes': len(item), 'sha256': sha256(bytes(item))}
        decoded.append(item if isinstance(item, (int, float, dict)) else repr(item))
    return {'command': message.command, 'name': name, 'fields': decoded,
            'request_bytes': len(message.encode()), 'request_sha256': sha256(message.encode())}


class DryRunSession:
    """Forwards read commands to a session and reports every other command without sending it."""

    def __init__(self, session, report=print, allowed=READ_COMMANDS):
        self.session = session
        self.report = report
        self.allowed = allowed
        self.blocked = []

    def call(self, build, timeout_s=COMMAND_TIMEOUT_S):
        message = build(0)
        if message.command in self.allowed:
            return self.session.call(build, timeout_s=timeout_s)
        self.blocked.append(message)
        self.report(describe_request(message))
        return Message(RESPONSE_KIND, message.request_id, 0, message.command, 0, b'')

    def reply_values(self, reply):
        return self.session.reply_values(reply) if reply.payload else []

    def __getattr__(self, name):
        return getattr(self.session, name)
