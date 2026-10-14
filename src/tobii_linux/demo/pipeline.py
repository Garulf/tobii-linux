"""Tracker data to browser events: head pose on its own thread, gaze as it arrives, and session status."""
from collections import deque
from dataclasses import dataclass
import logging
import threading
import time

from .. import gaze
from ..prp_service import UserPresence
from ..streams import Presence
from ..ttp_commands import Stream
from .events import frame_event, gaze_columns_event, gaze_event, mesh_topology_event, pose_event

LOG = logging.getLogger('tobii_linux.demo')
MAX_ERRORS = 20
GAZE_COLUMNS_INTERVAL_S = 0.25
PRESENCE_STATES = {gaze.PRESENCE_UNKNOWN: 'unknown', gaze.PRESENCE_AWAY: 'away',
                   gaze.PRESENCE_PRESENT: 'present'}
RATE_WINDOW_S = 2.0


@dataclass(frozen=True)
class GazePoint:
    """A gaze point normalised to the display area: (0, 0) top-left, (1, 1) bottom-right."""
    x: float
    y: float


class DeviceCombinedGaze:
    """The gaze point games receive: the tracker's combined point (column 28) where column 27 marks it valid."""
    name = 'device'
    description = 'Combined gaze point computed by the tracker (column 28)'

    def __call__(self, frame):
        if not frame.gaze_point.valid:
            return None
        return GazePoint(*frame.gaze_point.point)


GAZE_SOURCES = {DeviceCombinedGaze.name: DeviceCombinedGaze}


class RateMeter:
    def __init__(self, window_s=RATE_WINDOW_S, clock=time.monotonic):
        self.window_s, self.clock = window_s, clock
        self.times = deque()
        self.lock = threading.Lock()

    def tick(self):
        now = self.clock()
        with self.lock:
            self.times.append(now)
            self._expire(now)

    def _expire(self, now):
        while self.times and now - self.times[0] > self.window_s:
            self.times.popleft()

    def rate(self):
        now = self.clock()
        with self.lock:
            self._expire(now)
            if len(self.times) < 2:
                return 0.0
            span = max(now - self.times[0], self.times[-1] - self.times[0])
            return (len(self.times) - 1) / span if span > 0 else 0.0


class LatestSlot:
    """Holds the newest frame for the processing thread; a frame replaced before it is taken counts as dropped."""
    def __init__(self):
        self.condition = threading.Condition()
        self.item = None
        self.dropped = 0

    def put(self, item):
        with self.condition:
            if self.item is not None:
                self.dropped += 1
            self.item = item
            self.condition.notify()

    def take(self, timeout_s):
        with self.condition:
            self.condition.wait_for(lambda: self.item is not None, timeout_s)
            item, self.item = self.item, None
            return item


def display_area_status(area):
    def distance(a, b):
        return sum((p - q) ** 2 for p, q in zip(a, b)) ** 0.5
    return {'top_left': list(area.top_left), 'top_right': list(area.top_right),
            'bottom_left': list(area.bottom_left),
            'width_mm': round(distance(area.top_left, area.top_right), 1),
            'height_mm': round(distance(area.top_left, area.bottom_left), 1)}


@dataclass(frozen=True)
class ProcessedFrame:
    tracked: object
    sample: object
    mesh_pixels: object = None
    mesh_millimetres: object = None


class HeadPoseProcessor:
    """FaceTracker and HeadPoseOutput as driver.head_pose_samples runs them, plus the fitted wireframe."""
    def __init__(self, model_directory, display_area):
        from pathlib import Path
        from ..face_tracking import FaceTracker
        from ..face_wireframe import FaceWireframe
        self.tracker = FaceTracker(model_directory)
        self.wireframe = FaceWireframe.load(Path(model_directory)/'support', self.tracker.pose_model)
        self.display_area = display_area
        self.output = None

    def calibrate(self, calibration):
        from ..head_pose_pipeline import HeadPoseOutput
        self.output = HeadPoseOutput(calibration.translation, calibration.angles, self.display_area)
        self.tracker.reset()

    def topology(self):
        from ..face_wireframe import display_scene
        corners, tracker = display_scene(self.display_area, self.output.display)
        return mesh_topology_event(self.wireframe.edges, len(self.wireframe.vertices), corners, tracker)

    def __call__(self, frame):
        import numpy as np
        pixels = np.frombuffer(frame.pixels, np.uint8).reshape(frame.height, frame.width)
        tracked = self.tracker.process(pixels, frame.timestamp // 1000)
        fit = tracked.fit
        sample = self.output(fit['rigid_state'] if fit is not None else None, frame.timestamp)
        if fit is None:
            return ProcessedFrame(tracked, sample)
        return ProcessedFrame(tracked, sample, self.wireframe.image_points(fit, frame.width, frame.height),
                              self.wireframe.display_points(fit, self.output, sample))


class DemoPipeline:
    """Receives frames and gaze records from a source thread and publishes browser events.

`publish(topic, payload, retain=False)` must be thread safe. Image frames go
through a one-frame slot to the processing thread, so frames that arrive while
a frame is being processed are dropped and head pose never falls behind.
"""
    def __init__(self, publish, display_area, model_directory=None, image_rate=15.0,
                 gaze_source=None, mode='live', head_pose=True, processor_factory=HeadPoseProcessor):
        self.publish = publish
        self.display_area = display_area
        self.model_directory = model_directory
        self.image_interval_s = 1.0 / image_rate if image_rate > 0 else float('inf')
        self.gaze_source = gaze_source if gaze_source is not None else DeviceCombinedGaze()
        self.mode = mode
        self.head_pose_wanted = head_pose
        self.processor_factory = processor_factory
        self.slot = LatestSlot()
        self.lock = threading.Lock()
        self.errors = deque(maxlen=MAX_ERRORS)
        self.connection, self.message = 'starting', 'Starting'
        self.device = {}
        self.session_details = {}
        self.calibration, self.calibration_generation = None, 0
        self.head_pose_state, self.head_pose_message = ('loading', 'Loading models') if head_pose else (
            'disabled', 'Head pose disabled')
        self.reader = None
        self.presence = None
        self.user_presence = UserPresence()
        self.presence_notifications = 0
        self.sessions = self.lost_sessions = 0
        self.counts = {'frames': 0, 'processed': 0, 'gaze': 0, 'gaze_valid': 0, 'gaze_errors': 0}
        self.rates = {name: RateMeter() for name in ('camera', 'processed', 'gaze', 'gaze_valid')}
        self.processing_ms = None
        self.started = time.monotonic()
        self.last_columns = 0.0
        self.frame_sequence = 0
        self.stopping = threading.Event()
        self.thread = None

    def start(self):
        self.thread = threading.Thread(target=self._run, name='demo-processing', daemon=True)
        self.thread.start()

    def stop(self, timeout_s=5.0):
        self.stopping.set()
        if self.thread is not None:
            self.thread.join(timeout_s)

    def set_connection(self, connection, message):
        with self.lock:
            changed = (connection, message) != (self.connection, self.message)
            self.connection, self.message = connection, message
        if changed:
            LOG.info('%s: %s', connection, message)

    def report_error(self, message):
        with self.lock:
            if self.errors and self.errors[-1]['message'] == message:
                self.errors[-1]['count'] += 1
                self.errors[-1]['time'] = time.time()
            else:
                self.errors.append({'time': time.time(), 'message': message, 'count': 1})
                LOG.warning('%s', message)

    def on_session(self, calibration, device, details=None):
        with self.lock:
            self.sessions += 1
            self.device = dict(device)
            self.session_details = dict(details or {})
            self.calibration = calibration
            self.calibration_generation += 1

    def on_frame(self, frame):
        self.counts['frames'] += 1
        self.rates['camera'].tick()
        self.slot.put(frame)

    def on_presence(self, presence):
        """A device presence record, kept as the game service keeps property 0xB."""
        with self.lock:
            if self.user_presence.record(presence.timestamp_us, gaze.platmod_presence(presence.value)):
                self.presence_notifications += 1
            value = self.user_presence.value
            self.presence = {'value': presence.value, 'state': PRESENCE_STATES[value],
                             'property_value': value, 'since_us': self.user_presence.since_us,
                             'notifications': self.presence_notifications}

    def on_presence_subscribed(self):
        """A new session subscribed presence: unknown until the device reports, as on Windows."""
        with self.lock:
            self.user_presence.clear()
        self.on_presence(Presence(0, 0))

    def on_session_lost(self):
        with self.lock:
            self.lost_sessions += 1

    def on_gaze(self, frame):
        self.counts['gaze'] += 1
        self.rates['gaze'].tick()
        try:
            point = self.gaze_source(frame)
        except Exception as error:
            self.counts['gaze_errors'] += 1
            self.report_error(f'Gaze source {self.gaze_source.name}: {error}')
            point = None
        if point is not None:
            self.counts['gaze_valid'] += 1
            self.rates['gaze_valid'].tick()
        self.publish('gaze', gaze_event(frame, point, self.gaze_source.name))
        now = time.monotonic()
        if now - self.last_columns >= GAZE_COLUMNS_INTERVAL_S:
            self.last_columns = now
            self.publish('gaze_columns', gaze_columns_event(frame))

    def _load_processor(self):
        if not self.head_pose_wanted:
            return None
        try:
            processor = self.processor_factory(self.model_directory, self.display_area)
        except Exception as error:
            self._head_pose('unavailable', f'Head pose unavailable: {type(error).__name__}: {error}')
            self.report_error(self.head_pose_message)
            return None
        self._head_pose('waiting', 'Waiting for the camera calibration')
        return processor

    def _head_pose(self, state, message):
        with self.lock:
            self.head_pose_state, self.head_pose_message = state, message

    def _run(self):
        processor = self._load_processor()
        generation = 0
        next_image = 0.0
        while not self.stopping.is_set():
            frame = self.slot.take(0.25)
            if frame is None:
                continue
            if processor is not None and generation != self.calibration_generation:
                generation = self.calibration_generation
                if self.calibration is not None:
                    processor.calibrate(self.calibration)
                    self.publish('mesh_topology', processor.topology(), True)
                    self._head_pose('running', 'Running')
                else:
                    self._head_pose('waiting', 'No camera calibration for this source')
            processed = None
            if processor is not None and processor.output is not None:
                processed = self._process(processor, frame)
            now = time.monotonic()
            if now >= next_image:
                next_image += self.image_interval_s
                if next_image < now:
                    next_image = now + self.image_interval_s
                self.frame_sequence += 1
                tracked = processed.tracked if processed is not None else None
                state = processor.tracker.state if processed is not None else None
                mesh = processed.mesh_pixels if processed is not None else None
                self.publish('frame', frame_event(self.frame_sequence, frame, tracked, state, mesh), True)

    def _process(self, processor, frame):
        started = time.perf_counter()
        try:
            processed = processor(frame)
        except Exception as error:
            self.report_error(f'Head pose: {type(error).__name__}: {error}')
            return None
        elapsed_ms = (time.perf_counter() - started) * 1000
        self.processing_ms = elapsed_ms if self.processing_ms is None else (
            0.9 * self.processing_ms + 0.1 * elapsed_ms)
        self.counts['processed'] += 1
        self.rates['processed'].tick()
        self.publish('pose', pose_event(processed.sample, processed.tracked, processor.tracker.state, elapsed_ms,
                                        processed.mesh_millimetres))
        return processed

    def status(self):
        reader = self.reader
        with self.lock:
            status = {
                'mode': self.mode, 'connection': self.connection, 'message': self.message,
                'device': self.device, 'session': self.session_details,
                'head_pose': {'state': self.head_pose_state, 'message': self.head_pose_message},
                'presence': self.presence, 'errors': list(self.errors),
                'sessions': self.sessions, 'lost_sessions': self.lost_sessions}
        status.update({
            'uptime_s': round(time.monotonic() - self.started, 1),
            'display_area': display_area_status(self.display_area),
            'gaze_source': {'name': self.gaze_source.name, 'description': self.gaze_source.description},
            'image_rate_hz': round(1 / self.image_interval_s, 2) if self.image_interval_s > 0 else 0,
            'rates': {name: round(meter.rate(), 1) for name, meter in self.rates.items()},
            'counters': {**self.counts, 'dropped_frames': self.slot.dropped,
                         'decode_errors': sum(reader.decode_errors.values()) if reader else 0,
                         'gaze_dropped': reader.dropped[Stream.GAZE] if reader else 0,
                         'presence': reader.received[Stream.PRESENCE] if reader else 0,
                         'other_stream_messages': reader.unexpected if reader else 0},
            'processing_ms': round(self.processing_ms, 1) if self.processing_ms is not None else None,
            'last_decode_error': reader.last_error if reader else None})
        return status
