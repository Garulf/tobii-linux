"""Linux device session for head pose: TTP startup, camera calibration and image frames.

Built from the runtime's tracker_lib_usb sequence (docs/usb-streams.md) and
run against a real tracker: TTP on channel 0, startup, calibration and image
frames as documented in its "Live observations" section.
"""
from collections import deque
from contextlib import contextmanager
from dataclasses import dataclass, replace
import threading
import time
from .camera_info import parse_hardware_info
from .frames import image_frame
from .ttp import Decoder, fields, hardware_info_request, protocol_upgrade
from .ttp_commands import (Command, STREAM_DATA_KIND, RESPONSE_KIND, Stream, close_realm,
                           header_only_request, open_realm_response, open_realm_salt,
                           realm_digest, subscribe_stream, unsubscribe_stream, wire_stream_id)
from .ttp_fields import response_status, status_name, value

UNLOCK_REALM_KEY = b'aaaaaaaabbbbbbbbccccccccdddddddd'
HEAD_POSE_REALM = 0x2712
HEAD_POSE_REALM_KEY = bytes.fromhex('b88cc29417ebf48ebc59d1b59d7ac573')
HEAD_POSE_REALM_EXTRA = bytes.fromhex('816d4df2')
REQUEST_TIMEOUT_S = 3.0


class DeviceError(RuntimeError):
    pass


class NoTrackerError(DeviceError):
    pass


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
more (a tracker that stalls stays silent for the rest of its session).
"""
    def __init__(self, channel):
        self.channel = channel
        self.decoder = Decoder()
        self.request_id = 0
        self.pending_streams = []
        self.dead = False
        self.usb_device = {}

    def read(self, timeout_ms):
        try:
            return self.channel.read(timeout_ms=timeout_ms)
        except (OSError, RuntimeError):
            self.dead = True
            raise

    def write(self, data):
        try:
            self.channel.write(data)
        except (OSError, RuntimeError):
            self.dead = True
            raise

    def _next_id(self):
        self.request_id = (self.request_id + 1) & 0xFFFFFFFF
        return self.request_id

    def _receive(self, timeout_s):
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            data = self.read(max(1, int((deadline - time.monotonic()) * 1000)))
            if data:
                yield from self.decoder.feed(data)

    def call(self, build, timeout_s=REQUEST_TIMEOUT_S):
        message = build(self._next_id())
        self.write(message.encode())
        for reply in self._receive(timeout_s):
            if reply.kind == RESPONSE_KIND and reply.request_id == message.request_id:
                status = response_status(reply)
                if status:
                    raise DeviceError(f'Command {message.command} failed: {status_name(status)}')
                return reply
            if reply.kind == STREAM_DATA_KIND:
                self.pending_streams.append(reply)
        self.dead = True
        raise DeviceError(f'Command {message.command} timed out')

    def reply_values(self, reply):
        return [value(field) for field in fields(reply.payload)]

    def open_realm(self, realm, key, extra=b''):
        salt_reply = self.reply_values(self.call(lambda rid: open_realm_salt(rid, realm)))
        integers = [v for v in salt_reply if isinstance(v, int)]
        salt = next(v for v in salt_reply if isinstance(v, (bytes, bytearray)))
        digest = realm_digest(key, bytes(salt))
        self.call(lambda rid: open_realm_response(rid, (integers[0], integers[1]), digest, extra))

    def initialize(self):
        """Protocol upgrade then unlock_ppu, opening the realm it requests (0x1402bc2b0)."""
        self.call(protocol_upgrade)
        unlock = [v for v in self.reply_values(self.call(
            lambda rid: header_only_request(rid, Command.UNLOCK_PPU))) if isinstance(v, int)]
        if len(unlock) >= 3 and unlock[0] and not unlock[2]:
            self.open_realm(unlock[1], UNLOCK_REALM_KEY)

    def camera_calibration(self, camera=0):
        """Realm 0x2712, hardware info (2120), close realm, as head-pose start does."""
        self.open_realm(HEAD_POSE_REALM, HEAD_POSE_REALM_KEY, HEAD_POSE_REALM_EXTRA)
        try:
            info = parse_hardware_info(self.call(hardware_info_request))
        finally:
            self.call(lambda rid: close_realm(rid, HEAD_POSE_REALM))
        record = info.cameras_0x8[camera]
        return CameraCalibration(tuple(record.triple_0x10), tuple(record.doubles_0x28[:3]))

    def subscribe_images(self):
        self.call(lambda rid: subscribe_stream(rid, Stream.IMAGE))

    def frames(self, timeout_s=1.0):
        """Image frames from stream 0x501 as they arrive, until the device stays silent for timeout_s."""
        image_stream = wire_stream_id(Stream.IMAGE)
        pending, self.pending_streams = self.pending_streams, []
        deadline = time.monotonic() + timeout_s
        while True:
            for message in pending:
                if message.kind == STREAM_DATA_KIND and message.command == image_stream:
                    yield image_frame(message)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return
            data = self.read(max(1, int(remaining * 1000)))
            pending = self.decoder.feed(data) if data else []
            if data:
                deadline = time.monotonic() + timeout_s


def newest_frames(frames, on_receive=None):
    """Read a blocking frame iterator on its own thread and yield only the newest frame.

Frames that arrive while the consumer is busy are dropped, so a slow pipeline
never lets the device's stream back up. `on_receive` runs on the reader thread
for every frame as it arrives.
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
source; the mapping then starts over from the next frame.
"""
    def __init__(self, clock, window=64):
        self.clock = clock
        self.offsets = deque(maxlen=window)
        self.offset = None
        self.generation = getattr(clock, 'generation', None)

    def observe(self, device_us):
        generation = getattr(self.clock, 'generation', None)
        if generation != self.generation:
            self.generation = generation
            self.offsets.clear()
        self.offsets.append(self.clock() - device_us)
        self.offset = min(self.offsets)

    def __call__(self, device_us):
        """Host time of a device timestamp, never later than the host clock's current time."""
        now = self.clock()
        if getattr(self.clock, 'generation', None) != self.generation:
            return now
        return min(device_us + self.offset, now)


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
    """A started TrackerSession on the `index`th Eye Tracker 5 libusb sees; closed on exit."""
    from .usb import USBContext
    from .usb_channel import USBChannel
    with USBContext() as context:
        devices = context.probe()
        if index >= len(devices):
            raise NoTrackerError('No Eye Tracker 5 is visible to libusb' if not devices else
                                 f'Tracker index {index} requested, {len(devices)} visible')
        device = devices[index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            with USBChannel(transport, channel) as usb_channel:
                session = TrackerSession(usb_channel)
                session.usb_device = device
                session.initialize()
                yield session
