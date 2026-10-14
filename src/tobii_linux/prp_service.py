"""Linux PRP service for the game DLL's head-pose session, reached through a pipe relay.

A Windows bridge inside the Wine prefix (tools/prp_bridge) owns the named pipes
and forwards each pipe connection over its own loopback TCP connection. This
module speaks the service side of the PRP handshake and the subset of the session
the game DLL uses: connect, stream start/stop for head pose (stream 2) and, with a
gaze hub, gaze point (3), responsive gaze point (29) and gaze origin (4),
timesync, stream packages, and the property requests of the game integration
layer (display area, display id, user presence, with presence change notifications).
Like the Windows runtime, it serves clients only while a tracker session runs: when
the tracker goes away every client connection is closed and new ones are refused
until it is back. Relay framing and real-service comparison: docs/prp-bridge.md,
docs/gaze-host.md and docs/service-behaviour.md.
"""

import argparse
import asyncio
from dataclasses import dataclass
import functools
import itertools
import logging
import math
import struct
import time
import uuid

from tobii_linux import gaze
from tobii_linux.head_pose_pipeline import DisplayArea, HeadPoseSample
from tobii_linux.prp import HEAD_POSE, HeadPose, ProtocolError, decode_envelope, encode_envelope
from tobii_linux.prp import encode_stream_packet
from tobii_linux.ttp_fields import float32
from tobii_linux.prp_session import (
    COMMAND_CHANNEL, COMMAND_CONNECT, COMMAND_PROPERTY_SUBSCRIBE, COMMAND_PROPERTY_UNSUBSCRIBE,
    COMMAND_STREAM_START, COMMAND_STREAM_STOP, COMMAND_TIMESYNC, OPERATION_COMMAND_REQUEST,
    OPERATION_COMMAND_RESPONSE, OPERATION_PROPERTY_GET, PROPERTY_DISPLAY_AREA,
    PROPERTY_DISPLAY_ID, SERVER_PIPE_PREFIX,
    STATUS_NOT_SUPPORTED, STATUS_OK, SUBSCRIPTION_CHANNEL, TIMESYNC_CHANNEL, CommandResponse,
    ConnectResponse, HandshakeInfo, PropertyResponse, decode_client_hello, decode_client_pid,
    decode_command_request, decode_property_request, decode_property_subscribe_request,
    display_area_value, encode_command_response, encode_handshake_ack, encode_handshake_info,
    encode_property_notification, encode_property_response, encode_property_subscribe_response,
    split_packets, string_value)

log = logging.getLogger("tobii_linux.prp_service")

RELAY_VERSION = 1
RELAY_HEADER = struct.Struct("<II")
FRAME_BRIDGE_HELLO = 1
FRAME_PIPES = 2
FRAME_PIPE_OPEN = 3
FRAME_REPLY_PIPE = 4
FRAME_MESSAGE = 5
FRAME_DISPLAY_ID = 6
MAX_FRAME = 1 << 20
DEFAULT_PORT = 7760
DEFAULT_NAME = "ET5-LINUX"
INTERNAL_URL_SCHEME = "prp-internal://"

STATUS_CONNECTION_FAILED = 3
STATUS_NOT_SUBSCRIBED = 5
STATUS_INVALID_PARAMETER = 8
STATUS_INSUFFICIENT_LICENSE = 17
GAZE_STREAMS = (gaze.PRP_GAZE_POINT, gaze.PRP_RESPONSIVE_GAZE_POINT, gaze.PRP_GAZE_ORIGIN)
# The real service refused these with INSUFFICIENT_LICENSE to a client without license keys.
LICENSED_STREAMS = (gaze.PRP_GAZE_DATA, gaze.PRP_EYE_OPENNESS)
# Stream list of the real service's connect response, without the streams this service lacks.
GAZE_SERVICE_STREAMS = (gaze.PRP_GAZE_DATA, gaze.PRP_GAZE_POINT, gaze.PRP_RESPONSIVE_GAZE_POINT,
                        gaze.PRP_GAZE_ORIGIN, HEAD_POSE)
# Generic error body: four zero words satisfy every fixed-size response branch of the
# client decoder except connect (0x180055780, operation 10).
ERROR_BODY_WORDS = 4
# Real service head-pose cadence on albedo: 30.207 ms between timestamps.
REAL_SERVICE_PERIOD_S = 0.030207
# Unsent bytes above which stream packages are dropped for a slow client.
MAX_PENDING_STREAM_BYTES = 64 * 1024
PROPERTY_MODEL = 8
# Power save active (2), device paused (4), exclusive mode (5), calibration active (7) and
# 0x10: the real service answered subscribe with 0 (and get, where tried: 2, 4, 5). The game
# layer drops its notification subscriptions when one of them is refused, and its reconnect
# then stalls re-subscribing the refused one.
PROPERTY_ZERO_WORDS = (2, 4, 5, 7, 0x10)
PROPERTY_CALIBRATION_ID = 6
PROPERTY_USER_PRESENCE = gaze.PROPERTY_USER_PRESENCE
PROPERTY_TRACK_BOX = 0xF
PROPERTY_FIELD_OF_USE = 0x11
PROPERTY_FAULTS, PROPERTY_WARNINGS = 0x15, 0x16
PROPERTY_LENS_CONFIGURATION = 0xC
# Subscribe and unsubscribe answers of the real service (live, Windows,
# evidence/private/robust-live/subscribe-all.jsonl): these are refused with INSUFFICIENT_LICENSE,
# 3, 8 and 9 with NOT_SUPPORTED, and unsubscribing anything not subscribed gives NOT_SUBSCRIBED.
LICENSED_SUBSCRIPTIONS = (0xA, 0xC, 0xD, 0xE, 0x13, 0x14, 0x17, 0x18)
SUBSCRIBE_ONLY_PROPERTIES = (PROPERTY_USER_PRESENCE, PROPERTY_CALIBRATION_ID, PROPERTY_TRACK_BOX,
                             PROPERTY_FIELD_OF_USE, PROPERTY_FAULTS, PROPERTY_WARNINGS)
# Track box corners in mm as the reference tracker reported them: front plane at 450 mm,
# back plane at 900 mm (property 0xF, live). The device getter 1400 is not queried.
TRACK_BOX_MM = (125.0, 100.0, 450.0, -125.0, 100.0, 450.0, -125.0, -100.0, 450.0,
                125.0, -100.0, 450.0, 250.0, 200.0, 900.0, -250.0, 200.0, 900.0,
                -250.0, -200.0, 900.0, 250.0, -200.0, 900.0)
TRACKER_STATUS_TEXT = "ok"
MODEL_NAME = "IS5_Large_Eyetracker_5"
# Subscribing presence on the device emits an unknown record this much before the tracker's
# current time when no presence data has arrived yet (0x140276dd0).
PRESENCE_SUBSCRIBE_LEAD_US = 30000
# Real service notifications carry message tag 0, as its stream packages do.
NOTIFICATION_TAG = 0
# Smallest value block the client decoder accepts per property id (0x180055780, operation 6);
# ids not listed need 8 bytes. Error replies carry a zero block of this size, as the real
# service did for the properties it refused.
PROPERTY_BLOCK_SIZES = {0: 4, 1: 0x28, 0xA: 0x1C, 0xB: 0x10, 0xC: 0x18, 0xD: 0xC, 0xF: 0x64,
                        0x13: 0x488, 0x14: 0x10, 0x17: 0x34, 0x18: 0x10}
DEFAULT_PROPERTY_BLOCK_SIZE = 8


WINE_CLOCKS = ("boottime", "raw")


def wine_clock(name: str = "boottime"):
    """Microsecond clock that runs at the rate of Wine's QueryPerformanceCounter.

    The game DLL converts stream timestamps with an offset measured by timesync,
    so the service clock must tick at the same rate as the client's counter.
    Wine 10.17 and later read CLOCK_BOOTTIME ("boottime"); 10.16 and earlier read
    CLOCK_MONOTONIC_RAW ("raw"). The two can drift apart by percent under WSL2.
    """
    if name not in WINE_CLOCKS:
        raise ValueError(f"Unknown Wine clock {name!r}; expected one of {WINE_CLOCKS}")
    clock_id = time.CLOCK_BOOTTIME if name == "boottime" else time.CLOCK_MONOTONIC_RAW
    return lambda: time.clock_gettime_ns(clock_id) // 1000


service_clock_us = wine_clock()


def device_connect_response(serial: str, license_count: int,
                            streams: tuple = (HEAD_POSE,)) -> ConnectResponse:
    """Connect response modelled on the real ET5 service's reply (albedo, build 1.37.4.0).

    Only the license-result count, firmware flag and stream list are known to be
    checked by device_create. The other values are copied from the real reply so
    later queries see a plausible device; their meaning is not established.
    """
    return ConnectResponse(
        build_version="1.37.4.0_tobii_linux",
        list_a=(8, 12, 22, 21, 1, 15, 18, 2, 5, 4, 9, 16, 7, 6, 19, 17),
        list_b=(1, 18, 2, 4, 16),
        list_c=(0, 1, 2, 0, 4, 5, 6, 7, 0, 0, 0, 11, 0, 0, 0, 15, 16, 17, 18, 0, 0, 21, 22, 0, 0),
        list_d=(9,),
        streams=streams,
        supported_commands=(COMMAND_CONNECT, COMMAND_STREAM_START, COMMAND_STREAM_STOP,
                            COMMAND_TIMESYNC),
        groups=((2, (17, 18)),),
        strings=(serial, "IS5_Large_Eyetracker_5", "IS5", "tobii-linux", "Peripheral",
                 "IS5LEYETRACKER5", ""),
        words=(2, _float_word(184.0), _float_word(20.0), 0, _float_word(-0.16),
               _float_word(13.85), 0, _float_word(5.38), _float_word(9.86)),
        license_results=(0,) * license_count,
        mode=2)


def _float_word(value: float) -> int:
    return struct.unpack("<i", struct.pack("<f", value))[0]


def parse_display_area(text: str) -> DisplayArea:
    """Nine comma-separated millimetre values: top-left, top-right, bottom-left corners."""
    values = [float(value) for value in text.split(",")]
    if len(values) != 9:
        raise ValueError("display area needs nine comma-separated numbers")
    return DisplayArea(tuple(values[0:3]), tuple(values[3:6]), tuple(values[6:9]))


def word_value(value: int) -> tuple[bytes, bytes]:
    return struct.pack("<II", 8, value), struct.pack("<I", 4)


def presence_value(value: int, since_us: int) -> tuple[bytes, bytes]:
    """Property 0xB: time of the last presence change (high, low word), then the wire value.

    The real service answered two subscribes minutes apart with the same timestamp, so
    the time is that of the last change, not of the request.
    """
    block = struct.pack("<IIII", 0x10, since_us >> 32 & 0xFFFFFFFF, since_us & 0xFFFFFFFF,
                        value)
    return block, struct.pack("<I", 4)


def floats_value(values) -> tuple[bytes, bytes]:
    return struct.pack(f"<I{len(values)}f", 4 + 4 * len(values), *values), struct.pack("<I", 4)


def property_error(tag: int, property_id: int, status: int = STATUS_NOT_SUPPORTED):
    """Refusal with a zero value block, so the client decoder still accepts the packet."""
    size = PROPERTY_BLOCK_SIZES.get(property_id, DEFAULT_PROPERTY_BLOCK_SIZE)
    block = struct.pack("<I", size) + bytes(size - 4)
    return PropertyResponse(tag, status, property_id, block, struct.pack("<I", 4))


def subscribe_error(tag: int, property_id: int, status: int = STATUS_NOT_SUPPORTED):
    """Refused command 5, carrying the empty value the real service sent for that property."""
    if property_id == PROPERTY_MODEL:
        return PropertyResponse(tag, status, property_id, *string_value(""))
    if property_id == PROPERTY_LENS_CONFIGURATION:
        block = struct.pack("<6I", 0x18, 0, 4, 0, 4, 0)
        return PropertyResponse(tag, status, property_id, block, struct.pack("<I", 4))
    return property_error(tag, property_id, status)


# Relay framing ------------------------------------------------------------------------


def encode_frame(kind: int, payload: bytes = b"") -> bytes:
    if len(payload) > MAX_FRAME:
        raise ProtocolError("relay frame exceeds the limit")
    return RELAY_HEADER.pack(kind, len(payload)) + payload


async def read_frame(reader: asyncio.StreamReader) -> tuple[int, bytes]:
    kind, size = RELAY_HEADER.unpack(await reader.readexactly(RELAY_HEADER.size))
    if size > MAX_FRAME:
        raise ProtocolError("relay frame exceeds the limit")
    return kind, await reader.readexactly(size)


def encode_names(names) -> bytes:
    return b"".join(name.encode("ascii") + b"\0" for name in names)


def decode_names(payload: bytes) -> list[str]:
    if payload and not payload.endswith(b"\0"):
        raise ProtocolError("unterminated relay name")
    return [name.decode("ascii") for name in payload.split(b"\0")[:-1]]


def encode_pipe_open(client_pid: int, pipe: str) -> bytes:
    return struct.pack("<II", RELAY_VERSION, client_pid) + encode_names([pipe])


def decode_pipe_open(payload: bytes) -> tuple[int, str]:
    if len(payload) < 9:
        raise ProtocolError("truncated pipe open frame")
    version, client_pid = struct.unpack_from("<II", payload)
    names = decode_names(payload[8:])
    if version != RELAY_VERSION or len(names) != 1:
        raise ProtocolError("unsupported pipe open frame")
    return client_pid, names[0]


# Sample distribution ------------------------------------------------------------------------


class SampleHub:
    """Fan-out of samples to every streaming subscription; never blocks producers."""

    def __init__(self, depth: int = 4):
        self.depth = depth
        self.queues: set[asyncio.Queue] = set()

    def subscribe(self) -> asyncio.Queue:
        queue = asyncio.Queue(self.depth)
        self.queues.add(queue)
        return queue

    def unsubscribe(self, queue: asyncio.Queue) -> None:
        self.queues.discard(queue)

    def publish(self, sample) -> None:
        for queue in self.queues:
            if queue.full():
                queue.get_nowait()
            queue.put_nowait(sample)

    async def feed_iterator(self, samples) -> None:
        """Publish from a blocking iterator (for example the USB driver) without stalling the loop."""
        loop = asyncio.get_running_loop()
        iterator = iter(samples)
        done = object()
        while (sample := await loop.run_in_executor(None, next, iterator, done)) is not done:
            self.publish(sample)


class HeadPoseHub(SampleHub):
    """HeadPoseSample values, one per processed camera frame, on the service clock."""


def presence_subscribe_time(tracker_time_us: int) -> int:
    """Time of the unknown record a presence subscription emits (0x140276dd0)."""
    if tracker_time_us >= PRESENCE_SUBSCRIBE_LEAD_US:
        return tracker_time_us - PRESENCE_SUBSCRIBE_LEAD_US
    return tracker_time_us


class UserPresence:
    """Property 0xB as the service caches it (0x1400fd220, cache 0x14010e930).

    Every platmod presence record replaces the cached property record; it is
    pushed to subscribed clients unless it is the first record since the cache
    was cleared. There is no comparison with the previous value: the device
    only sends changes (and its current value when subscribed).
    """

    def __init__(self):
        self.clear()

    def clear(self) -> None:
        """Forget the record, as the service does when a device is added again (0x14010e8d0)."""
        self.value: int | None = None
        self.since_us = 0

    def record(self, timestamp_us: int, platmod_value: int) -> bool:
        """Cache one platmod record; True when the service would push it as a notification."""
        notify = self.value is not None
        self.value, self.since_us = gaze.prp_presence(platmod_value), timestamp_us
        return notify

    @property
    def present(self) -> bool:
        return self.value == gaze.PRESENCE_PRESENT

    def value_block(self) -> tuple[bytes, bytes]:
        return presence_value(self.value, self.since_us)

    def notification(self) -> bytes:
        return encode_property_notification(NOTIFICATION_TAG, PROPERTY_USER_PRESENCE,
                                            *self.value_block())


class GazeHub(SampleHub):
    """gaze.GazeFrame values, one per device gaze message, on the service clock."""


def synthetic_sample(timestamp_us: int, start_us: int) -> HeadPoseSample:
    """A slow head sweep: yaw and pitch sinusoids with a small positional drift."""
    seconds = (timestamp_us - start_us) / 1e6
    yaw = 0.35 * math.sin(2 * math.pi * seconds / 6.0)
    pitch = 0.15 * math.sin(2 * math.pi * seconds / 4.0)
    position = (20.0 * math.sin(2 * math.pi * seconds / 8.0), 10.0, 600.0)
    return HeadPoseSample(timestamp_us, True, position, (pitch, yaw, 0.0))


def synthetic_gaze_frame(timestamp_us: int, start_us: int) -> gaze.GazeFrame:
    """A gaze point tracing a slow figure eight over the display, both eyes valid."""
    seconds = (timestamp_us - start_us) / 1e6
    x = 0.5 + 0.35 * math.sin(2 * math.pi * seconds / 10.0)
    y = 0.5 + 0.3 * math.sin(4 * math.pi * seconds / 10.0)
    point = (float32(x), float32(y))
    origin_left = tuple(float32(v) for v in (-32.0, 20.0, 620.0))
    origin_right = tuple(float32(v) for v in (32.0, 20.0, 620.0))
    guide_left = tuple(float32(v) for v in (0.55, 0.5, 0.45))
    guide_right = tuple(float32(v) for v in (0.45, 0.5, 0.45))
    device = gaze.DeviceGaze(
        timestamp=timestamp_us, combined_point=point, combined_validity=1,
        responsive_point=point, responsive_validity=1, left_gaze_point_2d=point,
        right_gaze_point_2d=point, left_validity=0, right_validity=0,
        left_origin_display=origin_left, right_origin_display=origin_right,
        left_origin_display_validity=1, right_origin_display_validity=1,
        left_position_guide=guide_left, right_position_guide=guide_right,
        left_position_guide_validity=1, right_position_guide_validity=1)
    return gaze.gaze_frame(device)


async def publish_synthetic(hub: HeadPoseHub, period_s: float = REAL_SERVICE_PERIOD_S,
                            clock=service_clock_us, gaze_hub: GazeHub | None = None) -> None:
    loop = asyncio.get_running_loop()
    start, first_tick = clock(), loop.time()
    for tick in itertools.count(1):
        now = clock()
        hub.publish(synthetic_sample(now, start))
        if gaze_hub is not None:
            gaze_hub.publish(synthetic_gaze_frame(now, start))
        await asyncio.sleep(max(0.0, first_tick + tick * period_s - loop.time()))


def stream_package(sample: HeadPoseSample) -> bytes:
    """Operation 1 package as sent by the real service: message tag 0, one sample."""
    valid = bool(sample.valid)
    pose = HeadPose(sample.timestamp_us, valid, tuple(sample.position_mm), (valid,) * 3,
                    tuple(sample.rotation_rad))
    return encode_stream_packet(pose, message_tag=0)


def gaze_package(frame: gaze.GazeFrame, stream: int) -> bytes:
    """Gaze stream package as the real service sends it: message tag 0, one record."""
    return gaze.frame_packet(frame, stream, message_tag=0)


# Command handling -----------------------------------------------------------------------


def packet_operation(packet: bytes) -> int:
    payload = decode_envelope(packet)
    if len(payload) < 12:
        raise ProtocolError("truncated PRP header")
    return struct.unpack_from("<I", payload, 8)[0]


@dataclass(frozen=True)
class RequestHeader:
    tag: int
    command: int


def request_header(packet: bytes) -> RequestHeader:
    """Tag and command id of any operation 9 request, including commands without a codec."""
    payload = decode_envelope(packet)
    if len(payload) < 16:
        raise ProtocolError("truncated PRP request")
    size, tag, operation = struct.unpack_from("<III", payload)
    command_block = size + struct.unpack_from("<I", payload, size)[0] if size == 12 else 0
    if (operation != OPERATION_COMMAND_REQUEST or command_block == 0
            or command_block + 8 > len(payload)):
        raise ProtocolError("not a PRP command request")
    block_size, command = struct.unpack_from("<II", payload, command_block)
    if block_size < 8:
        raise ProtocolError("PRP command block is too short")
    return RequestHeader(tag, command)


def error_response(tag: int, command: int, status: int) -> bytes:
    """Nonzero-status reply for commands this service does not implement."""
    header = struct.pack("<IIII", 12, tag, OPERATION_COMMAND_RESPONSE, 4)
    status_block = struct.pack("<IiII", 12, status, command, 4)
    body = struct.pack("<I", 4 * ERROR_BODY_WORDS) + bytes(4 * ERROR_BODY_WORDS - 4)
    return encode_envelope(header + status_block + body + struct.pack("<II", 4, 0))


class Subscription:
    """One client: its subscription channel, the streams it started and the properties it watches.

    The command channel that connected with this subscription is paired with it;
    when either channel closes, the other is closed too, as the real service
    destroys a command client together with its subscription client.
    """

    def __init__(self, subscription_id: int, connection: "PipeConnection"):
        self.subscription_id = subscription_id
        self.connection = connection
        self.command: "PipeConnection | None" = None
        self.field_of_use = 0
        self.streams: dict[int, tuple] = {}
        self.properties: set[int] = set()

    def streaming(self, stream: int) -> bool:
        return stream in self.streams

    def start(self, stream: int, hub: SampleHub, encode) -> None:
        if stream not in self.streams:
            queue = hub.subscribe()
            self.streams[stream] = (hub, queue, asyncio.create_task(self._push(queue, encode)))

    def stop(self, stream: int) -> None:
        if stream in self.streams:
            hub, queue, task = self.streams.pop(stream)
            task.cancel()
            hub.unsubscribe(queue)

    def stop_all(self) -> None:
        for stream in list(self.streams):
            self.stop(stream)
        self.properties.clear()

    def close(self) -> None:
        self.stop_all()
        for connection in (self.connection, self.command):
            if connection is not None:
                connection.close()

    async def _push(self, queue: asyncio.Queue, encode) -> None:
        while True:
            self.connection.send_stream(encode(await queue.get()))


class PipeConnection:
    """Service side of one relayed pipe connection (one channel of one client)."""

    def __init__(self, service: "PrpService", reader, writer, client_pid: int, pipe: str):
        self.service, self.reader, self.writer = service, reader, writer
        self.client_pid, self.pipe = client_pid, pipe
        self.channel = None
        self.subscription = None
        self.tail = b""

    def send(self, message: bytes) -> None:
        if self.writer.is_closing():
            return
        log.debug("%s -> %s", self.pipe, message.hex())
        self.writer.write(encode_frame(FRAME_MESSAGE, message))

    def close(self) -> None:
        self.writer.close()

    def send_stream(self, packet: bytes) -> None:
        if self.writer.transport.get_write_buffer_size() > MAX_PENDING_STREAM_BYTES:
            self.service.dropped_packages += 1
            return
        self.service.sent_packages += 1
        self.send(packet)

    async def message(self) -> bytes:
        kind, payload = await read_frame(self.reader)
        if kind != FRAME_MESSAGE:
            raise ProtocolError("expected a relayed pipe message")
        log.debug("%s <- %s", self.pipe, payload.hex())
        return payload

    async def run(self) -> None:
        self.service.connections.add(self)
        try:
            await self.handshake()
            while True:
                packets, self.tail = split_packets(self.tail + await self.message())
                for packet in packets:
                    self.handle_packet(packet)
                await self.writer.drain()
        finally:
            self.service.connections.discard(self)
            if self.subscription is not None:
                self.service.subscriptions.pop(self.subscription.subscription_id, None)
                self.subscription.close()

    async def handshake(self) -> None:
        """Steps 1 to 5 of docs/service-transport.md, as the real service answered them."""
        hello = decode_client_hello(await self.message())
        self.channel = hello.channel
        self.writer.write(encode_frame(FRAME_REPLY_PIPE, encode_names([hello.reply_pipe])))
        self.send(encode_handshake_ack())
        pid = decode_client_pid(await self.message())
        if self.client_pid and pid != self.client_pid:
            raise ProtocolError(f"client pid {pid} differs from pipe client {self.client_pid}")
        self.send(encode_handshake_info(self.service.handshake_info(self)))
        await self.writer.drain()
        log.info("pipe %s channel %#x handshake done, pid %d", self.pipe, self.channel, pid)

    def handle_packet(self, packet: bytes) -> None:
        if self.channel == SUBSCRIPTION_CHANNEL:
            log.warning("ignoring client packet on subscription channel")
            return
        if packet_operation(packet) == OPERATION_PROPERTY_GET:
            tag, property_id = decode_property_request(packet)
            response = self.service.answer_property(tag, property_id)
            log.info("property %#x tag %d status %d", property_id, tag, response.status)
            self.send(encode_property_response(response))
            return
        header = request_header(packet)
        if header.command in (COMMAND_PROPERTY_SUBSCRIBE, COMMAND_PROPERTY_UNSUBSCRIBE):
            self.send(self.property_subscription(packet))
            return
        try:
            request = decode_command_request(packet)
        except ProtocolError:
            log.info("command %#x tag %d not supported", header.command, header.tag)
            self.send(error_response(header.tag, header.command, STATUS_NOT_SUPPORTED))
            return
        self.send(encode_command_response(self.service.respond(self, request)))

    def property_subscription(self, packet: bytes) -> bytes:
        """Command 5 answers with the current value and starts notifications; command 6 ends them."""
        tag, command, property_id = decode_property_subscribe_request(packet)
        subscribed = self.subscription.properties if self.subscription is not None else set()
        if command == COMMAND_PROPERTY_UNSUBSCRIBE:
            if property_id in LICENSED_SUBSCRIPTIONS:
                status = STATUS_INSUFFICIENT_LICENSE
            elif property_id in subscribed:
                status = STATUS_OK
            else:
                status = STATUS_NOT_SUBSCRIBED
            subscribed.discard(property_id)
            log.info("property %#x unsubscribe tag %d status %d", property_id, tag, status)
            return encode_command_response(CommandResponse(tag, command, status, property_id))
        response = self.service.answer_property(tag, property_id, subscribe=True,
                                                subscription=self.subscription)
        if response.status == STATUS_OK and self.subscription is not None:
            self.subscription.properties.add(property_id)
        log.info("property %#x subscribe tag %d status %d", property_id, tag, response.status)
        return encode_property_subscribe_response(response)


class PrpService:
    """Shared state: pipe names, subscription ids and the sample hub."""

    def __init__(self, hub: HeadPoseHub, name: str = DEFAULT_NAME, serial: str | None = None,
                 clock=service_clock_us, display_area: DisplayArea | None = None,
                 display_id: str | None = None, gaze_hub: GazeHub | None = None):
        self.hub, self.name, self.clock = hub, name, clock
        self.gaze_hub = gaze_hub
        self.serial = serial or name
        self.display_area, self.display_id = display_area, display_id
        self.reported_display_id: str | None = None
        self.subscription_pipe = str(uuid.uuid4())
        self.timesync_pipe = str(uuid.uuid4())
        self.subscriptions: dict[int, Subscription] = {}
        self.connections: set[PipeConnection] = set()
        self.bridges: set = set()
        self.ids = itertools.count(1)
        self.sent_packages = self.dropped_packages = self.sent_notifications = 0
        self.presence = UserPresence()
        self.tracker_attached = False
        self.calibration_id = 0

    def attach_tracker(self, calibration_id: int | None = None) -> None:
        """A tracker session is running: accept clients, as the real server starts listening."""
        if calibration_id is not None:
            self.calibration_id = calibration_id
        if not self.tracker_attached:
            log.info("tracker attached; accepting game clients (calibration id %#x)",
                     self.calibration_id)
        self.tracker_attached = True

    def detach_tracker(self) -> None:
        """The tracker is gone: close every client connection and refuse new ones.

        The real service shuts its server down when the device is removed or the
        host goes to sleep, destroying every command, subscription and timesync
        client, and listens again once the device is re-added; clients reconnect.
        """
        if self.tracker_attached:
            log.info("tracker detached; closing %d client connection(s)", len(self.connections))
        self.tracker_attached = False
        for connection in list(self.connections):
            connection.close()
        for subscription in list(self.subscriptions.values()):
            subscription.stop_all()
        self.presence.clear()

    def publish_presence(self, timestamp_us: int, platmod_value: int) -> None:
        """One platmod presence record on the service clock; pushes operation 2 when due."""
        if not self.presence.record(timestamp_us, platmod_value):
            return
        notification = self.presence.notification()
        for subscription in self.subscriptions.values():
            if PROPERTY_USER_PRESENCE in subscription.properties:
                subscription.connection.send(notification)
                self.sent_notifications += 1
        log.info("user presence %d since %d", self.presence.value, self.presence.since_us)

    @property
    def served_streams(self) -> dict:
        """PRP stream id: (hub, package encoder) for every stream this service publishes."""
        streams = {HEAD_POSE: (self.hub, stream_package)}
        if self.gaze_hub is not None:
            for stream in GAZE_STREAMS:
                streams[stream] = (self.gaze_hub, functools.partial(gaze_package, stream=stream))
        return streams

    @property
    def advertised_streams(self) -> tuple:
        return GAZE_SERVICE_STREAMS if self.gaze_hub is not None else (HEAD_POSE,)

    @property
    def pipe_names(self) -> list[str]:
        """Pipes the bridge must serve: prefixed and bare command pipes, then the two internal ones.

        The client opens TOBIIPRP-<name> when a pipe listing shows it and <name>
        otherwise (0x18004e8a0); prp-internal:// URLs name their pipe directly.
        """
        return [SERVER_PIPE_PREFIX + self.name, self.name, self.subscription_pipe,
                self.timesync_pipe]

    def handshake_info(self, connection: PipeConnection) -> HandshakeInfo:
        """Ids and URLs as the real service sent them: URLs only on the command channel."""
        if connection.channel == COMMAND_CHANNEL:
            return HandshakeInfo(next(self.ids), INTERNAL_URL_SCHEME + self.subscription_pipe,
                                 INTERNAL_URL_SCHEME + self.timesync_pipe)
        if connection.channel == SUBSCRIPTION_CHANNEL:
            subscription = Subscription(next(self.ids), connection)
            connection.subscription = subscription
            self.subscriptions[subscription.subscription_id] = subscription
            return HandshakeInfo(subscription.subscription_id, "", "")
        if connection.channel == TIMESYNC_CHANNEL:
            return HandshakeInfo(0, "", "")
        raise ProtocolError(f"unknown channel {connection.channel:#x}")

    def respond(self, connection: PipeConnection, request) -> CommandResponse:
        if request.command == COMMAND_TIMESYNC:
            return CommandResponse(request.tag, request.command, STATUS_OK, self.clock())
        if connection.channel != COMMAND_CHANNEL:
            return CommandResponse(request.tag, request.command, STATUS_NOT_SUPPORTED, 0)
        if request.command == COMMAND_CONNECT:
            return self.connect(connection, request)
        subscription = connection.subscription
        if subscription is None:
            return CommandResponse(request.tag, request.command, STATUS_CONNECTION_FAILED,
                                   request.stream)
        served = self.served_streams
        if request.stream not in served:
            status = (STATUS_INSUFFICIENT_LICENSE
                      if self.gaze_hub is not None and request.stream in LICENSED_STREAMS
                      and request.command == COMMAND_STREAM_START else STATUS_NOT_SUPPORTED)
            return CommandResponse(request.tag, request.command, status, request.stream)
        if request.command == COMMAND_STREAM_START:
            subscription.start(request.stream, *served[request.stream])
            log.info("stream %d started for subscription %d", request.stream,
                     subscription.subscription_id)
            return CommandResponse(request.tag, request.command, STATUS_OK, request.stream)
        if not subscription.streaming(request.stream):
            return CommandResponse(request.tag, request.command, STATUS_NOT_SUBSCRIBED,
                                   request.stream)
        subscription.stop(request.stream)
        log.info("stream %d stopped for subscription %d", request.stream,
                 subscription.subscription_id)
        return CommandResponse(request.tag, request.command, STATUS_OK, request.stream)

    def answer_property(self, tag: int, property_id: int, subscribe: bool = False,
                        subscription: Subscription | None = None) -> PropertyResponse:
        """Value of a property for a get, or for a subscribe (command 5), or a refusal.

        Served: display area and display id when configured, the model name, properties
        2 (power save active), 4 (device paused) and 5 (exclusive mode), all 0 as the real
        service sent them, and user presence for subscribes only (the real service refused
        the plain get).
        """
        if subscribe and property_id in LICENSED_SUBSCRIPTIONS:
            return subscribe_error(tag, property_id, STATUS_INSUFFICIENT_LICENSE)
        value = self.property_value(property_id, subscribe, subscription)
        if value is None:
            return (subscribe_error if subscribe else property_error)(tag, property_id)
        return PropertyResponse(tag, STATUS_OK, property_id, *value)

    def property_value(self, property_id: int, subscribe: bool,
                       subscription: Subscription | None = None):
        if property_id in SUBSCRIBE_ONLY_PROPERTIES and not subscribe:
            return self.get_value(property_id)
        if property_id == PROPERTY_CALIBRATION_ID:
            return word_value(self.calibration_id)
        if property_id == PROPERTY_TRACK_BOX:
            return floats_value(TRACK_BOX_MM)
        if property_id == PROPERTY_FIELD_OF_USE:
            return word_value(subscription.field_of_use if subscription is not None else 0)
        if property_id in (PROPERTY_FAULTS, PROPERTY_WARNINGS):
            return string_value(TRACKER_STATUS_TEXT)
        return self.get_value(property_id, subscribe)

    def get_value(self, property_id: int, subscribe: bool = False):
        if property_id == PROPERTY_DISPLAY_AREA and self.display_area:
            area = self.display_area
            return display_area_value(area.top_left, area.top_right, area.bottom_left)
        if property_id == PROPERTY_DISPLAY_ID and (self.display_id or self.reported_display_id):
            return string_value(self.display_id or self.reported_display_id)
        if property_id == PROPERTY_MODEL and not subscribe:
            return string_value(MODEL_NAME)
        if property_id in PROPERTY_ZERO_WORDS:
            return word_value(0)
        if property_id == PROPERTY_USER_PRESENCE and subscribe and self.presence.value is not None:
            return self.presence.value_block()
        return None

    def connect(self, connection: PipeConnection, request) -> CommandResponse:
        subscription = self.subscriptions.get(request.connect.subscription_client_id)
        if subscription is None:
            return CommandResponse(request.tag, COMMAND_CONNECT, STATUS_INVALID_PARAMETER,
                                   connect=device_connect_response(self.serial, 0,
                                                                   self.advertised_streams))
        connection.subscription = subscription
        subscription.command = connection
        subscription.field_of_use = request.connect.field_of_use
        licenses = len(request.connect.licenses)
        log.info("connect from pid %d with %d license(s), subscription %d",
                 request.connect.process_id, licenses, subscription.subscription_id)
        return CommandResponse(request.tag, COMMAND_CONNECT, STATUS_OK,
                               connect=device_connect_response(self.serial, licenses,
                                                               self.advertised_streams))

    async def handle(self, reader, writer) -> None:
        peer = writer.get_extra_info("peername")
        try:
            kind, payload = await read_frame(reader)
            if kind == FRAME_BRIDGE_HELLO:
                writer.write(encode_frame(FRAME_PIPES, encode_names(self.pipe_names)))
                await writer.drain()
                self.bridges.add(writer)
                log.info("bridge %s attached", peer)
                while True:
                    kind, payload = await read_frame(reader)
                    if kind == FRAME_DISPLAY_ID:
                        self.reported_display_id = payload.decode("utf-8", "replace")
                        log.info("bridge reports primary monitor %s", self.reported_display_id)
            elif kind == FRAME_PIPE_OPEN:
                client_pid, pipe = decode_pipe_open(payload)
                if not self.tracker_attached:
                    log.info("refusing pid %d on %s: no tracker", client_pid, pipe)
                    return
                await PipeConnection(self, reader, writer, client_pid, pipe).run()
            else:
                raise ProtocolError(f"unexpected first relay frame {kind}")
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        except ProtocolError as exc:
            log.warning("closing relay connection %s: %s", peer, exc)
        finally:
            self.bridges.discard(writer)
            writer.close()

    async def serve(self, host: str = "127.0.0.1", port: int = DEFAULT_PORT):
        return await asyncio.start_server(self.handle, host, port)

    async def close(self, server, timeout_s: float = 2.0) -> None:
        """Disconnect every client and bridge, then stop listening."""
        self.detach_tracker()
        for writer in list(self.bridges):
            writer.close()
        server.close()
        try:
            await asyncio.wait_for(server.wait_closed(), timeout_s)
        except asyncio.TimeoutError:
            log.warning("relay connections still open after %.1f s", timeout_s)


async def _main(args) -> None:
    hub = HeadPoseHub()
    gaze_hub = GazeHub() if args.synthetic and not args.no_gaze else None
    clock = wine_clock(args.wine_clock)
    service = PrpService(hub, args.name, clock=clock, display_area=args.display_area,
                         display_id=args.display_id, gaze_hub=gaze_hub)
    if args.synthetic:
        service.attach_tracker()
        service.publish_presence(clock(), gaze.PLATMOD_PRESENT)
    server = await service.serve(args.host, args.port)
    log.info("PRP service on %s, pipes %s", server.sockets[0].getsockname()[:2], service.pipe_names)
    producer = (asyncio.create_task(publish_synthetic(hub, clock=clock, gaze_hub=gaze_hub))
                if args.synthetic else None)
    async with server:
        await server.serve_forever()
    if producer:
        producer.cancel()


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(description="Serve PRP head pose and gaze to a Wine pipe bridge.")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--name", default=DEFAULT_NAME, help="device name in tobii-prp://<name>")
    parser.add_argument("--synthetic", action="store_true",
                        help="publish a synthetic head sweep and gaze figure eight instead of "
                             "tracker output")
    parser.add_argument("--no-gaze", action="store_true",
                        help="with --synthetic, serve head pose only, as before gaze support")
    parser.add_argument("--display-area", type=parse_display_area,
                        help="top-left, top-right and bottom-left display corners in mm, "
                             "x,y,z each, comma-separated (tobii_get_display_area)")
    parser.add_argument("--display-id",
                        help="display id string (tobii_get_display_id); the game layer matches "
                             "the part before '##' with a monitor's device instance id")
    parser.add_argument("--debug", action="store_true", help="log every relayed pipe message")
    parser.add_argument("--wine-clock", choices=WINE_CLOCKS, default="boottime",
                        help="clock behind the game's Wine QueryPerformanceCounter: "
                             "boottime for Wine 10.17 and later, raw for older")
    args = parser.parse_args(argv)
    logging.basicConfig(level=logging.DEBUG if args.debug else logging.INFO,
                        format="%(asctime)s %(message)s")
    asyncio.run(_main(args))


if __name__ == "__main__":
    main()
