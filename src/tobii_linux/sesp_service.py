"""Service side of SESP: head pose for the game DLL's tet-tcp devices through the pipe relay.

The bridge owns `\\\\.\\pipe\\streamengineservices` and relays each connection
like a PRP pipe (docs/prp-bridge.md). For every relayed connection this module
answers the SESP hello with a slot index, pairs the command channel with the
subscription channel named in the initialize request, and pushes the head-pose
hub's samples as SESP head-pose messages once the client starts head pose.
Messages are those of ses_windows.dll (sesp.py). The game layer's own SESP
client sends usage statistics, which are acknowledged and dropped. Requests it
does not serve get the service's own "invalid operation" signal. docs/sesp.md
has the evidence.

The legacy module turns every SESP head position p into A p + t, with A a
rotation about x by the display tilt and t minus the display centre, both from
the device display area (0x18004ac80, applied by 0x180048d30). Stream Engine
head positions are already display centred, so by default positions are sent
as A^T (p - t), which the module turns back into the position the PRP route
delivers. `compensate_position=False` sends them unchanged, as ses_windows.dll does.
"""
import asyncio
from dataclasses import dataclass
import itertools
import logging
import math
import struct

from tobii_linux import sesp

log = logging.getLogger("tobii_linux.sesp_service")

# Unsent bytes above which head-pose messages are dropped for a slow client.
MAX_PENDING_BYTES = 64 * 1024
CAPABILITIES = frozenset({sesp.CAPABILITY_HEAD_POSE, sesp.CAPABILITY_STREAM_STATUS})


def _f32(value: float) -> float:
    return struct.unpack("<f", struct.pack("<f", value))[0]


@dataclass(frozen=True)
class LegacyPositionTransform:
    """What the legacy module does to SESP head positions for one display area."""
    cos: float
    sin: float
    translation: tuple

    @classmethod
    def from_display_area(cls, area) -> "LegacyPositionTransform":
        top_left, top_right, bottom_left = area.top_left, area.top_right, area.bottom_left
        tilt = _f32(-math.atan2(_f32(top_left[2] - bottom_left[2]),
                                _f32(top_left[1] - bottom_left[1])))
        translation = tuple(_f32(_f32(bottom_left[i] + top_right[i]) * -0.5) for i in range(3))
        return cls(_f32(math.cos(tilt)), _f32(math.sin(tilt)), translation)

    def apply(self, position) -> tuple:
        x, y, z = position
        tx, ty, tz = self.translation
        return (x + tx, self.cos * y - self.sin * z + ty, self.sin * y + self.cos * z + tz)

    def invert(self, position) -> tuple:
        x, y, z = (value - offset for value, offset in zip(position, self.translation))
        return (x, self.cos * y + self.sin * z, -self.sin * y + self.cos * z)


def head_pose_message(sequence: int, sample, timestamp_us: int, transform=None) -> bytes:
    validity = sesp.VALID if sample.valid else sesp.INVALID
    position = tuple(sample.position_mm)
    if transform is not None:
        position = transform.invert(position)
    return sesp.encode_head_pose(sequence, sesp.HeadPose(
        timestamp_us, position, tuple(sample.rotation_rad), validity, (validity,) * 3))


class SespClient:
    """One relayed SESP connection: a command or a subscription channel."""

    def __init__(self, server: "SespServer", pipe, client_pid: int):
        self.server, self.pipe, self.client_pid = server, pipe, client_pid
        self.slot = next(server.slots)
        self.channel = None
        self.peer: "SespClient | None" = None
        self.sequence = itertools.count(1)
        self.head_pose: asyncio.Task | None = None
        self.decoder = sesp.FrameDecoder()

    async def run(self) -> None:
        hello = sesp.decode_hello(await self.pipe.message())
        self.channel = hello.channel
        self.pipe.open_reply(hello.reply_pipe)
        self.pipe.send(sesp.encode_slot(self.slot))
        log.info("SESP channel %#x from pid %d is slot %d", self.channel, self.client_pid,
                 self.slot)
        self.server.clients[self.slot] = self
        try:
            while True:
                for buffer in self.decoder.feed(await self.pipe.message()):
                    self.handle(sesp.decode(buffer))
                await self.pipe.drain()
        finally:
            self.server.clients.pop(self.slot, None)
            self.stop_head_pose()
            if self.peer is not None:
                self.peer.stop_head_pose()
                self.peer.pipe.close()

    def handle(self, message: sesp.Message) -> None:
        if message.kind == sesp.REQUEST_INITIALIZE:
            self.initialize(message)
        elif message.kind == sesp.REQUEST_HEADPOSE_START:
            error = self.start_head_pose()
            self.pipe.send(sesp.encode_error_response(message.sequence,
                                                      sesp.RESPONSE_HEADPOSE_START, error))
        elif message.kind == sesp.REQUEST_HEADPOSE_STOP:
            if self.peer is not None:
                self.peer.stop_head_pose()
            self.pipe.send(sesp.encode_error_response(message.sequence,
                                                      sesp.RESPONSE_HEADPOSE_STOP, sesp.ERROR_NONE))
            log.info("SESP head pose stopped for slot %d", self.slot)
        elif message.kind == sesp.REQUEST_NOTIFY_STREAM_STATUS:
            self.pipe.send(sesp.encode_stream_status_response(message.sequence))
        elif message.kind == sesp.REQUEST_SEND_STATISTICS:
            self.pipe.send(sesp.encode_error_response(message.sequence,
                                                      sesp.RESPONSE_SEND_STATISTICS, sesp.ERROR_NONE))
        else:
            log.info("SESP request type %#x on slot %d not served", message.kind, self.slot)
            self.pipe.send(sesp.encode_error_response(message.sequence,
                                                      sesp.SIGNAL_INVALID_OPERATION,
                                                      sesp.ERROR_NOT_SUPPORTED))

    def initialize(self, message: sesp.Message) -> None:
        """Pair with the subscription slot named in the request; ses_windows answers 7 without it."""
        request = sesp.initialize_request(message)
        subscription = self.server.clients.get(request.subscription_id)
        if subscription is None or subscription.channel != sesp.SUBSCRIPTION_CHANNEL:
            log.info("SESP initialize for %s names unknown subscription slot %d",
                     request.device_url, request.subscription_id)
            self.pipe.send(sesp.encode_initialize_response(message.sequence, sesp.ERROR_NOT_FOUND))
            return
        self.peer, subscription.peer = subscription, self
        log.info("SESP initialize for %s: command slot %d, subscription slot %d",
                 request.device_url, self.slot, subscription.slot)
        self.pipe.send(sesp.encode_initialize_response(message.sequence, sesp.ERROR_NONE,
                                                       CAPABILITIES))

    def start_head_pose(self) -> int:
        if self.peer is None:
            return sesp.ERROR_NOT_FOUND
        self.peer.push_head_pose()
        log.info("SESP head pose started on subscription slot %d", self.peer.slot)
        return sesp.ERROR_NONE

    def push_head_pose(self) -> None:
        if self.head_pose is None:
            self.head_pose = asyncio.create_task(self._push(self.server.hub.subscribe()))

    def stop_head_pose(self) -> None:
        if self.head_pose is not None:
            self.head_pose.cancel()
            self.head_pose = None

    async def _push(self, queue: asyncio.Queue) -> None:
        try:
            while True:
                sample = await queue.get()
                if self.pipe.pending_bytes() > MAX_PENDING_BYTES:
                    self.server.dropped += 1
                    continue
                self.pipe.send(head_pose_message(next(self.sequence), sample,
                                                 self.server.game_time(sample.timestamp_us),
                                                 self.server.position_transform()))
                self.server.sent += 1
        finally:
            self.server.hub.unsubscribe(queue)


class SespServer:
    """Slot table and head-pose source shared by every SESP connection.

    `game_time` maps service-clock microseconds onto the game's
    QueryPerformanceCounter microseconds (qpc_clock.CounterMapping), since SESP
    has no timesync: the module hands these timestamps to the game unchanged.
    `display_area` returns the area the module received from the device, or None.
    """

    def __init__(self, hub, game_time=lambda us: us, display_area=lambda: None,
                 compensate_position: bool = True):
        self.hub, self.game_time, self.display_area = hub, game_time, display_area
        self.compensate_position = compensate_position
        self.slots = itertools.count(1)
        self.clients: dict[int, SespClient] = {}
        self.sent = self.dropped = 0

    def position_transform(self) -> LegacyPositionTransform | None:
        area = self.display_area() if self.compensate_position else None
        return LegacyPositionTransform.from_display_area(area) if area is not None else None

    async def serve(self, pipe, client_pid: int) -> None:
        await SespClient(self, pipe, client_pid).run()
