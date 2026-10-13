"""Linux device session for head pose: TTP startup, camera calibration and image frames.

Built from the runtime's tracker_lib_usb sequence (docs/usb-streams.md). It has
not been run against a device: the TTP channel index, device acceptance of each
request and the frame route are unverified.
"""
from dataclasses import dataclass
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


@dataclass(frozen=True)
class CameraCalibration:
    """Extrinsic of the selected camera record: translation and three angles."""
    translation: tuple
    angles: tuple


class TrackerSession:
    """Request/response and stream demultiplexing over one USB channel.

`channel` is any object with write(bytes) and read(timeout_ms=...) -> bytes,
such as usb_channel.USBChannel.
"""
    def __init__(self, channel):
        self.channel = channel
        self.decoder = Decoder()
        self.request_id = 0
        self.pending_streams = []

    def _next_id(self):
        self.request_id = (self.request_id + 1) & 0xFFFFFFFF
        return self.request_id

    def _receive(self, timeout_s):
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            data = self.channel.read(timeout_ms=max(1, int((deadline - time.monotonic()) * 1000)))
            if data:
                yield from self.decoder.feed(data)

    def call(self, build, timeout_s=REQUEST_TIMEOUT_S):
        message = build(self._next_id())
        self.channel.write(message.encode())
        for reply in self._receive(timeout_s):
            if reply.kind == RESPONSE_KIND and reply.request_id == message.request_id:
                status = response_status(reply)
                if status:
                    raise DeviceError(f'Command {message.command} failed: {status_name(status)}')
                return reply
            if reply.kind == STREAM_DATA_KIND:
                self.pending_streams.append(reply)
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
        """Image frames from stream 0x501 until the device stays silent for timeout_s."""
        image_stream = wire_stream_id(Stream.IMAGE)
        while True:
            pending, self.pending_streams = self.pending_streams, []
            received = False
            for message in pending + list(self._receive(timeout_s)):
                received = True
                if message.kind == STREAM_DATA_KIND and message.command == image_stream:
                    yield image_frame(message)
            if not received:
                return


def head_pose_samples(session, model_directory, display_area, camera=0):
    """Frames from a started session through tracking, fitting and the output stages."""
    import numpy as np
    from .face_tracking import FaceTracker
    from .head_pose_pipeline import HeadPoseOutput
    calibration = session.camera_calibration(camera)
    output = HeadPoseOutput(calibration.translation, calibration.angles, display_area)
    tracker = FaceTracker(model_directory)
    session.subscribe_images()
    for frame in session.frames():
        pixels = np.frombuffer(frame.pixels, np.uint8).reshape(frame.height, frame.width)
        result = tracker.process(pixels)
        fit = result.fit['rigid_state'] if result.fit is not None else None
        yield output(fit, frame.timestamp)
