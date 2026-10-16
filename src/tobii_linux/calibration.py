"""User gaze calibration of the Eye Tracker 5: backup, restore and the Tobii Experience sequence.

The device computes gaze and keeps the active calibration itself (see
docs/gaze-device.md). Calibrating means driving its calibration commands in
the order Tobii's configuration app does through Stream Engine and the
platform runtime; docs/gaze-calibration.md has the evidence for every step.

backup_calibration and verify_backup only read. restore_calibration and
Calibrator write to the device; DryRunSession wraps a session so that only
read commands reach the device and every other command is reported instead.

The rules and the request sequences are crates/tobii-gaze/src/calibration.rs;
this module drives them on Python sessions and UIs and formats their reports.
"""
from dataclasses import dataclass
import hashlib
import math
from pathlib import Path
import threading
import time

from . import gaze_device as gd
from ._native import calibration as _cal
from .gaze_device import GazeCommand, LocalCalibration, drive
from .ttp import Message
from .ttp_commands import Command, NOTIFICATION_KIND, RESPONSE_KIND, STREAM_DATA_KIND

COMMAND_TIMEOUT_S = 3.0
TRANSFER_TIMEOUT_S = 15.0
COLLECT_TIMEOUT_S = 15.0
COMPUTE_TIMEOUT_S = 30.0

INTERACTIVE_POINTS = tuple(_cal.INTERACTIVE_POINTS)
STAGES = tuple(tuple(stage) for stage in _cal.STAGES)

MAX_CALIBRATION_AREA_MM = (600.0, 340.0)
MAX_DIAGONAL_MM = 690.0
MAX_HEIGHT_MM = 350.0
DISPLAY_TOLERANCE_MM = 10.0

ZONE_RADIUS_FACTOR = 0.45
FOCUS_CHANGE_MS = 150.0
GAZE_LOST_MS = 250.0
EYE_VALID_BELOW = 2
READ_SLICE_MS = 100
FIXATION_WINDOW_MS = 300.0
FIXATION_SPREAD = 0.03
FIXATION_MIN_SAMPLES = 6
FIXATION_TIMEOUT_S = 4.0
SACCADE_S = 0.2
ATTENTION_STALL_MS = 4000.0
ATTENTION_NO_POP_MS = 10000.0
USER_AWAY_MS = 500.0
MAX_STALLED_RUNS = 5
STAGES_STEP = 'targets'

READ_COMMANDS = frozenset(_cal.READ_COMMANDS)

CalibrationError = _cal.CalibrationError


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
    return _cal.calibration_area(display_mm)


def calibration_targets(display_mm=None, points=INTERACTIVE_POINTS):
    """Targets in normalized display coordinates, scaled into the calibration area like ScalePoints."""
    return tuple(_cal.calibration_targets(tuple(display_mm) if display_mm else None, list(points)))


def display_size_mm(values):
    """Width and height in mm from a 1430 reply: top-left, top-right, bottom-left corners."""
    return _cal.display_size_mm(values)


def read_display_size(session):
    return drive(_cal.read_display_size(), session)


def read_device_calibration_id(session) -> int:
    return drive(_cal.read_device_calibration_id(), session)


def retrieve_blob(session) -> bytes:
    return drive(_cal.retrieve_blob(), session)


def read_stimulus_points(session):
    return tuple(gd.stimulus_point(record) for record in drive(_cal.read_stimulus_points(), session))


def backup_calibration(session) -> LocalCalibration:
    """Read-only copy of the device calibration with the id the runtime would store with it.

The id is read before and after the transfer so a calibration that changes
in between is reported instead of saved with the wrong id.
"""
    return LocalCalibration(*drive(_cal.backup_calibration(), session))


def save_backup(path, calibration: LocalCalibration, overwrite=False):
    """Write calibration.setpm bytes; an existing file is kept unless overwrite is set."""
    return Path(_cal.save_backup(path, calibration, overwrite))


def load_backup(path) -> LocalCalibration:
    return LocalCalibration(*_cal.load_backup(path))


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
    return BackupCheck(*drive(_cal.verify_backup(calibration), session))


def restore_calibration(session, calibration: LocalCalibration) -> int:
    """Apply a backup (1110) and confirm the device now reports its id; returns that id.

This is what the runtime does at start when its calibration.setpm id differs
from the device ("Restoring calibration from disk"). The device takes the id
from the blob itself.
"""
    return drive(_cal.restore_calibration(calibration), session)


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

    def restarting(self, attempt):
        """A compute failed or the targets were not looked at; the calibration starts again from the first target."""

    def calibration_started(self, attempt):
        """The device is ready for targets (after 1010 and 1060); may block for an introduction."""

    def show_targets(self, stage, targets):
        """Gaze-chosen order: show a stage's targets, [(index, x, y)], and return once they are drawn."""

    def focus(self, index, focused):
        """Gaze-chosen order: the user's gaze settled on target index (focused) or left it (not focused)."""

    def computing(self, stage):
        """The device computes the calibration from the targets so far (1070); stage_done follows."""

    def check(self):
        """Raise to stop the calibration (the user cancelled); called often while waiting for the gaze."""


@dataclass(frozen=True)
class GazeSample:
    timestamp_us: int
    point: tuple | None


def _sample(values):
    return None if values is None else GazeSample(*values)


def gaze_sample(message):
    """Mean of the valid per-eye display points, as CalibrationProcessViewModel.OnGazeData forms it."""
    return _sample(_cal.gaze_sample(message))


def fixation(samples, window_ms=FIXATION_WINDOW_MS, spread=FIXATION_SPREAD, min_samples=FIXATION_MIN_SAMPLES):
    """Whether the newest samples within window_ms all have both eyes valid and stay within spread of their mean.

Points are in normalized display units of whatever calibration the device
holds (none after 1060), so only their stability is used, not their position.
"""
    return _cal.fixation(samples, window_ms, spread, min_samples)


def binocular_sample(message):
    """GazeSample whose point is the mean of both eyes' display points, or None unless both eyes are valid."""
    return _sample(_cal.binocular_sample(message))


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

    def zone_radius(self, targets):
        return _cal.zone_radius(tuple(self.screen_px), list(targets))

    def closest(self, point, targets, radius):
        return _cal.closest(tuple(self.screen_px), point, list(targets), radius)

    def focus_changed(self, samples, targets, focused):
        """samples: GazeSample in time order; targets: the stage's uncalibrated targets; focused: index into targets."""
        return _cal.focus_changed(tuple(self.screen_px), samples, list(targets), focused)


class GazeFocus:
    """Which target the gaze rests on, as DetermineFocusedCalibrationTarget decides it sample by sample.

The closest candidate within the zone radius of FocusCheck gains focus; it
moves to another target only once the current one has gone 150 ms without a
sample on it; gaze outside every zone keeps the focus; 250 ms without valid
gaze drops it. A stage of one target is focused by the first valid sample.
"""

    def __init__(self, check, stage_targets):
        self.check = check
        self.native = _cal.GazeFocus(tuple(check.screen_px), list(stage_targets))

    @property
    def radius(self):
        return self.native.radius

    @property
    def focused(self):
        return self.native.focused

    @focused.setter
    def focused(self, index):
        self.native.focused = index

    @property
    def last_focus_us(self):
        return self.native.last_focus_us

    @property
    def last_valid_us(self):
        return self.native.last_valid_us

    def observe(self, sample, candidates):
        """candidates: {index: (x, y)} still to collect; returns the focused index or None."""
        return self.native.observe(sample, list(candidates.items()))


class AttentionWatch:
    """CalibrationAssessor: whether the user stopped calibrating, which restarts the calibration.

From the second stage on, every valid gaze sample is checked: after a gap of
more than 500 ms (the user was away) both timers start again; otherwise the
calibration is stalled when for 4 s neither the gaze moved to a waiting
target nor a target was collected, or when for 10 s no target was collected.
"""

    def __init__(self):
        self.native = _cal.AttentionWatch(ATTENTION_STALL_MS, ATTENTION_NO_POP_MS)

    def observe(self, sample, looking_at_waiting_target):
        self.native.observe(sample, looking_at_waiting_target)

    def popped(self):
        self.native.popped()

    @property
    def stalled(self):
        return self.native.stalled

    @property
    def last_sample_us(self):
        return self.native.last_sample_us

    @property
    def last_attention_us(self):
        return self.native.last_attention_us

    @property
    def last_pop_us(self):
        return self.native.last_pop_us


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

A compute the device refuses (PROTOCOL_OPERATION_FAILED, the Tobii app's
"all points failed") stops the calibration (1020) and, up to `restarts`
times, starts it again from the first target (1010, 1060), as the app's "try
again" does. With `wait_for_fixation`, each collect waits until both eyes are
valid and the gaze has been still for 300 ms (at most 4 s); this is a Linux
addition, the app relies on its 200 ms saccade delay alone.

With `choose_by_gaze` (and a `focus` check) the targets are shown and taken
as the Tobii client does instead: every target of a stage is shown at once,
the user picks the order by looking, and a target is collected 200 ms after
the gaze settled on it (GazeFocus); data collected while the gaze moved away
is discarded and the target stays. From the second stage on an AttentionWatch
restarts the whole calibration when nothing happens, at most
`max_stalled_runs` times (the app has no limit).

The sequence is tobii_gaze::calibration::Calibrator, a state machine this
class drives: it sends each request with session.call and runs each UI
callback. The UI callbacks block (a target animates for about 1.5 s), so they
run on a worker thread while this thread keeps reading the device. The device
stops sending, and stops answering requests for the rest of the USB session,
once a subscribed stream has gone unread for about two seconds (live, see
docs/gaze-calibration.md); the session must never pause reading while gaze is
subscribed.
"""

    def __init__(self, session, ui=None, targets=INTERACTIVE_POINTS, stages=STAGES, previous=None,
                 focus=None, max_attempts=3, subscribe_gaze=True, log=None, restarts=2,
                 wait_for_fixation=False, choose_by_gaze=False, max_stalled_runs=MAX_STALLED_RUNS):
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
        self.restarts = restarts
        self.wait_for_fixation = wait_for_fixation
        self.choose_by_gaze = choose_by_gaze and focus is not None
        self.max_stalled_runs = max_stalled_runs
        self.compute_failures = []
        self.stalled_runs = 0
        self.step = STAGES_STEP

    def run(self) -> CalibrationOutcome:
        operation, progress = _cal.calibrator(
            [tuple(t) for t in self.targets], [list(stage) for stage in self.stages], self.previous,
            tuple(self.focus.screen_px) if self.focus is not None else None, self.max_attempts,
            self.subscribe_gaze, self.restarts, self.wait_for_fixation, self.choose_by_gaze, self.max_stalled_runs,
            ATTENTION_STALL_MS, ATTENTION_NO_POP_MS)
        try:
            return self._drive(operation)
        finally:
            self.step = progress.step
            self.discarded = list(progress.discarded)
            self.stalled_runs = progress.stalled_runs
            self.compute_failures = [(stage, attempt, tuple(gd.stimulus_point(p) for p in points))
                                     for stage, attempt, points in progress.compute_failures]

    def _drive(self, operation):
        step = operation.start(time.monotonic())
        while True:
            kind = step[0]
            if kind == 'done':
                return self._outcome(*step[1])
            if kind == 'failed':
                _, failed_step, cause = step
                raise CalibrationFailed(failed_step, cause) from cause
            if kind == 'call':
                step = self._send(operation, *step[1:])
            elif kind == 'ui':
                _, name, arguments = step
                step = self._report(operation, self._while_reading, getattr(self.ui, name), *arguments)
            elif kind == 'check':
                step = self._report(operation, self.ui.check)
            elif kind == 'read':
                step = self._report(operation, self._read_once)
            elif kind == 'streams':
                messages = list(self.session.pending_streams)
                if step[1]:
                    self.session.pending_streams.clear()
                step = operation.streams(messages, time.monotonic())
            elif kind == 'clear':
                self.session.pending_streams.clear()
                step = operation.done(time.monotonic())
            elif kind == 'log':
                _, record, values = step
                if 'stimulus_points' in values:
                    values['stimulus_points'] = quality_report([gd.stimulus_point(p) for p in values['stimulus_points']])
                self.log(record, **values)
                step = operation.done(time.monotonic())
            else:
                raise RuntimeError(f'Unknown calibration action {kind!r}')

    def _send(self, operation, timeout_s, tolerant):
        """One request; a failure goes back to the operation, except an interruption of a cleanup request."""
        try:
            reply = self.session.call(operation.request, timeout_s=timeout_s)
        except Exception as error:
            return operation.failed(error, time.monotonic())
        except BaseException as error:
            if tolerant:
                raise
            return operation.failed(error, time.monotonic())
        return operation.replied(self.session.reply_values(reply), time.monotonic())

    @staticmethod
    def _report(operation, action, *arguments):
        try:
            action(*arguments)
        except BaseException as error:
            return operation.failed(error, time.monotonic())
        return operation.done(time.monotonic())

    @staticmethod
    def _outcome(calibration, stimulus, points, discarded):
        return CalibrationOutcome(LocalCalibration(*calibration), tuple(gd.stimulus_point(p) for p in stimulus),
                                  tuple(gd.calibration_point(p) for p in points), tuple(discarded))

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
