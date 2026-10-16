"""Linux device session for head pose: TTP startup, camera calibration and image frames.

Built from the runtime's tracker_lib_usb sequence (docs/usb-streams.md) and
run against a real tracker: TTP on channel 0, startup, calibration and image
frames as documented in its "Live observations" section. The session itself is
crates/tobii-device (TrackerSession, ClockMapper); this module keeps the
Python-facing state (pending stream messages, the USB device description) and
the head-pose pipeline glue.
"""
from contextlib import contextmanager
from dataclasses import dataclass, replace
import threading
import time

from . import _native
from ._native import device as _device
from .frames import ImageFrame
from .ttp_commands import Command, Stream
from .ttp_fields import reply_values as _reply_values
from .ttp_fields import response_status, status_name  # noqa: F401  (used through this module)

UNLOCK_REALM_KEY = _device.UNLOCK_REALM_KEY
HEAD_POSE_REALM = _device.HEAD_POSE_REALM
HEAD_POSE_REALM_KEY = _device.HEAD_POSE_REALM_KEY
HEAD_POSE_REALM_EXTRA = _device.HEAD_POSE_REALM_EXTRA
REQUEST_TIMEOUT_S = 3.0

DeviceError = _native.DeviceError
NoTrackerError = _native.NoTrackerError


@dataclass(frozen=True)
class CameraCalibration:
    """Extrinsic of the selected camera record: translation and three angles."""
    translation: tuple
    angles: tuple


class TrackerSession:
    """Request/response and stream demultiplexing over one USB channel.

`channel` is any object with write(bytes) and read(timeout_ms=...) -> bytes,
such as usb_channel.USBChannel. `dead` becomes true once the device stopped
answering or the transport failed; such a session is not asked for anything
more (a tracker that stalls stays silent for the rest of its session). Stream
messages that arrive while a reply is awaited collect in `pending_streams`.

With a native USBChannel the reading loops run in Rust. With a channel written
in Python they run here, calling the Rust session for every decision, so that
no Rust code waits on Python code (a daemon thread doing so would abort the
interpreter at exit).
"""
    def __init__(self, channel):
        self.channel = channel
        self.native = _device.Session(channel)
        self.native_loops = self.native.native_channel
        self.decoder = SessionDecoder(self.native)
        self.pending_streams = []
        self.usb_device = {}
        self.startup_replies = {}

    @property
    def dead(self):
        return self.native.dead

    @dead.setter
    def dead(self, value):
        self.native.dead = value

    @property
    def request_id(self):
        return self.native.request_id

    def read(self, timeout_ms):
        if self.native_loops:
            return self.native.read(timeout_ms)
        try:
            return self.channel.read(timeout_ms=timeout_ms)
        except (OSError, RuntimeError):
            self.dead = True
            raise

    def write(self, data):
        if self.native_loops:
            return self.native.write(data)
        try:
            self.channel.write(data)
        except (OSError, RuntimeError):
            self.dead = True
            raise

    def _next_id(self):
        return self.native.next_id()

    def _collect(self):
        self.pending_streams.extend(self.native.take_pending())

    def _request(self, message, timeout_s):
        if self.native_loops:
            return self.native.exchange(message, timeout_s)
        self.write(message.encode())
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            data = self.read(max(1, int((deadline - time.monotonic()) * 1000)))
            reply = self.native.route(self.decoder.feed(data), message.request_id) if data else None
            if reply is not None:
                return reply
        return None

    def exchange(self, build, timeout_s=REQUEST_TIMEOUT_S):
        """The device's reply to one request whatever its status; None when none arrives in time."""
        try:
            return self._request(build(self._next_id()), timeout_s)
        finally:
            self._collect()

    def call(self, build, timeout_s=REQUEST_TIMEOUT_S):
        message = build(self._next_id())
        try:
            return self.native.finish_call(message, self._request(message, timeout_s))
        finally:
            self._collect()

    def reply_values(self, reply):
        return _reply_values(reply.payload)

    def observe(self, callback):
        """Call `callback(message)` for every message the session decodes; None stops it."""
        self.native.set_observer(callback)

    def open_realm(self, realm, key, extra=b''):
        try:
            self.native.open_realm(realm, key, extra)
        finally:
            self._collect()

    def initialize(self):
        """Protocol upgrade then unlock_ppu, opening the realm it requests (0x1402bc2b0).

The two replies stay in `startup_replies` (command: Message) for clients that
start the same way.
"""
        try:
            replies = self.native.initialize()
        finally:
            self._collect()
        self.startup_replies = {Command(command): reply for command, reply in replies}

    def camera_calibration(self, camera=0):
        """Realm 0x2712, hardware info (2120), close realm, as head-pose start does."""
        try:
            translation, angles = self.native.camera_calibration(camera)
        finally:
            self._collect()
        return CameraCalibration(translation, angles)

    def subscribe_images(self):
        try:
            self.native.subscribe(Stream.IMAGE)
        finally:
            self._collect()

    def frames(self, timeout_s=1.0):
        """Image frames from stream 0x501 as they arrive, until the device stays silent for timeout_s."""
        pending, self.pending_streams = self.pending_streams, []
        frames = _device.ImageFrames(self.native, pending, timeout_s)
        if self.native_loops:
            while (frame := frames.next_frame()) is not None:
                yield ImageFrame(*frame)
            return
        while True:
            frame = frames.pending_frame()
            if frame is not None:
                yield ImageFrame(*frame)
                continue
            remaining_ms = frames.remaining_ms()
            if remaining_ms is None:
                return
            frames.received(self.read(remaining_ms))


class SessionDecoder:
    """The session's TTP decoder; bytes from TrackerSession.read() go through feed()."""
    def __init__(self, native):
        self.native = native

    def feed(self, data):
        return self.native.decode(data)

    def finish(self):
        self.native.decoder_finish()

    @property
    def pending(self):
        return self.native.decoder_pending()


def newest_frames(frames, on_receive=None):
    """Read a blocking frame iterator on its own thread and yield only the newest frame.

Frames that arrive while the consumer is busy are dropped, so a slow pipeline
never lets the device's stream back up. `on_receive` runs on the reader thread
for every frame as it arrives. (tobii_device::threads::newest is the Rust
counterpart for native iterators.)
"""
    condition = threading.Condition()
    state = {'frame': None, 'done': False, 'error': None}

    def read():
        try:
            for frame in frames:
                if on_receive is not None:
                    on_receive(frame)
                with condition:
                    state['frame'] = frame
                    condition.notify()
        except Exception as error:
            state['error'] = error
        finally:
            with condition:
                state['done'] = True
                condition.notify()

    threading.Thread(target=read, name='tracker-frames', daemon=True).start()
    while True:
        with condition:
            condition.wait_for(lambda: state['frame'] is not None or state['done'])
            frame, state['frame'] = state['frame'], None
        if frame is None:
            if state['error'] is not None:
                raise state['error']
            return
        yield frame


class ClockMapper:
    """Maps device timestamps onto a host clock using the smallest recent receipt delay.

A clock with a `generation` attribute (qpc_clock.ServiceClock) can change its
source; the mapping then starts over from the next frame. The mapping itself is
tobii_device::clock::ClockOffsets.
"""
    def __init__(self, clock, window=64):
        self.clock = clock
        self.native = _device.ClockOffsets(window, getattr(clock, 'generation', None))

    @property
    def offset(self):
        return self.native.offset

    @property
    def offsets(self):
        return self.native.offsets

    def observe(self, device_us):
        generation = getattr(self.clock, 'generation', None)
        self.native.observe(generation, self.clock(), device_us)

    def __call__(self, device_us):
        """Host time of a device timestamp, never later than the host clock's current time."""
        now = self.clock()
        return self.native.map(now, getattr(self.clock, 'generation', None), device_us)


class HeadPoseProcessor:
    """Image frame to head-pose sample. The models load once; start() begins a tracker session.

Each session restarts the tracking state and the output filters with that
session's camera calibration, as the runtime builds a new algorithm instance
when a device is added.
"""
    def __init__(self, model_directory, display_area):
        from .face_tracking import FaceTracker
        self.tracker = FaceTracker(model_directory)
        self.display_area = display_area
        self.output = None

    def start(self, calibration):
        from .head_pose_pipeline import HeadPoseOutput
        self.tracker.reset()
        self.output = HeadPoseOutput(calibration.translation, calibration.angles, self.display_area)

    def __call__(self, frame):
        import numpy as np
        pixels = np.frombuffer(frame.pixels, np.uint8).reshape(frame.height, frame.width)
        result = self.tracker.process(pixels, frame.timestamp // 1000)
        fit = result.fit['rigid_state'] if result.fit is not None else None
        return self.output(fit, frame.timestamp)


def head_pose_processor(session, model_directory, display_area, camera=0):
    """Camera calibration from a started session, then a function from image frame to head-pose sample."""
    processor = HeadPoseProcessor(model_directory, display_area)
    processor.start(session.camera_calibration(camera))
    return processor


def head_pose_samples(session, model_directory, display_area, camera=0, host_clock=None):
    """Frames from a started session through tracking, fitting and the output stages.

Frames are always read on a separate thread and frames that arrive during
processing are skipped: the tracker stops sending for good when its endpoint
is not read for about a second. With `host_clock` (a function returning
microseconds), sample timestamps are moved onto that clock.
"""
    process = head_pose_processor(session, model_directory, display_area, camera)
    session.subscribe_images()
    to_host = ClockMapper(host_clock) if host_clock is not None else None
    on_receive = (lambda frame: to_host.observe(frame.timestamp)) if to_host is not None else None
    for frame in newest_frames(session.frames(), on_receive=on_receive):
        sample = process(frame)
        yield sample if to_host is None else replace(sample, timestamp_us=to_host(sample.timestamp_us))


@contextmanager
def usb_session(index=0, channel=0):
    """A started TrackerSession on the `index`th connected Eye Tracker 5; closed on exit."""
    from .usb import USBContext
    from .usb_channel import USBChannel
    with USBContext() as context:
        devices = context.probe()
        if index >= len(devices):
            raise NoTrackerError('No Eye Tracker 5 is visible over USB' if not devices else
                                 f'Tracker index {index} requested, {len(devices)} visible')
        device = devices[index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            with USBChannel(transport, channel) as usb_channel:
                session = TrackerSession(usb_channel)
                session.usb_device = device
                session.initialize()
                yield session
