"""Linux PRP service for the game DLL's head-pose session, reached through a pipe relay.

A Windows bridge inside the Wine prefix (tools/prp_bridge) owns the named pipes
and forwards each pipe connection over its own loopback TCP connection. This
module speaks the service side of the PRP handshake and the head-pose subset of
the session: connect, stream start/stop for stream 2, timesync, and stream
packages. Relay framing and real-service comparison: docs/prp-bridge.md.
"""

import argparse
import asyncio
from dataclasses import dataclass
import itertools
import logging
import math
import struct
import time
import uuid

from tobii_linux.head_pose_pipeline import HeadPoseSample
from tobii_linux.prp import HEAD_POSE, HeadPose, ProtocolError, decode_envelope, encode_envelope
from tobii_linux.prp import encode_stream_packet
from tobii_linux.prp_session import (
    COMMAND_CHANNEL, COMMAND_CONNECT, COMMAND_STREAM_START, COMMAND_STREAM_STOP,
    COMMAND_TIMESYNC, OPERATION_COMMAND_REQUEST, OPERATION_COMMAND_RESPONSE, SERVER_PIPE_PREFIX,
    STATUS_NOT_SUPPORTED, STATUS_OK, SUBSCRIPTION_CHANNEL, TIMESYNC_CHANNEL, CommandResponse,
    ConnectResponse, HandshakeInfo, decode_client_hello, decode_client_pid,
    decode_command_request, encode_command_response, encode_handshake_ack,
    encode_handshake_info, split_packets)

log = logging.getLogger("tobii_linux.prp_service")

RELAY_VERSION = 1
RELAY_HEADER = struct.Struct("<II")
FRAME_BRIDGE_HELLO = 1
FRAME_PIPES = 2
FRAME_PIPE_OPEN = 3
FRAME_REPLY_PIPE = 4
FRAME_MESSAGE = 5
MAX_FRAME = 1 << 20
DEFAULT_PORT = 7760
DEFAULT_NAME = "ET5-LINUX"
INTERNAL_URL_SCHEME = "prp-internal://"

STATUS_CONNECTION_FAILED = 3
STATUS_NOT_SUBSCRIBED = 5
STATUS_INVALID_PARAMETER = 8
# Generic error body: four zero words satisfy every fixed-size response branch of the
# client decoder except connect (0x180055780, operation 10).
ERROR_BODY_WORDS = 4
# Real service head-pose cadence on albedo: 30.207 ms between timestamps.
REAL_SERVICE_PERIOD_S = 0.030207
# Unsent bytes above which stream packages are dropped for a slow client.
MAX_PENDING_STREAM_BYTES = 64 * 1024


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


def device_connect_response(serial: str, license_count: int) -> ConnectResponse:
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
        streams=(HEAD_POSE,),
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


# Head-pose sample distribution ------------------------------------------------------------


class HeadPoseHub:
    """Fan-out of HeadPoseSample values to every streaming subscription; never blocks producers."""

    def __init__(self, depth: int = 4):
        self.depth = depth
        self.queues: set[asyncio.Queue] = set()

    def subscribe(self) -> asyncio.Queue:
        queue = asyncio.Queue(self.depth)
        self.queues.add(queue)
        return queue

    def unsubscribe(self, queue: asyncio.Queue) -> None:
        self.queues.discard(queue)

    def publish(self, sample: HeadPoseSample) -> None:
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


def synthetic_sample(timestamp_us: int, start_us: int) -> HeadPoseSample:
    """A slow head sweep: yaw and pitch sinusoids with a small positional drift."""
    seconds = (timestamp_us - start_us) / 1e6
    yaw = 0.35 * math.sin(2 * math.pi * seconds / 6.0)
    pitch = 0.15 * math.sin(2 * math.pi * seconds / 4.0)
    position = (20.0 * math.sin(2 * math.pi * seconds / 8.0), 10.0, 600.0)
    return HeadPoseSample(timestamp_us, True, position, (pitch, yaw, 0.0))


async def publish_synthetic(hub: HeadPoseHub, period_s: float = REAL_SERVICE_PERIOD_S,
                            clock=service_clock_us) -> None:
    loop = asyncio.get_running_loop()
    start, first_tick = clock(), loop.time()
    for tick in itertools.count(1):
        hub.publish(synthetic_sample(clock(), start))
        await asyncio.sleep(max(0.0, first_tick + tick * period_s - loop.time()))


def stream_package(sample: HeadPoseSample) -> bytes:
    """Operation 1 package as sent by the real service: message tag 0, one sample."""
    valid = bool(sample.valid)
    pose = HeadPose(sample.timestamp_us, valid, tuple(sample.position_mm), (valid,) * 3,
                    tuple(sample.rotation_rad))
    return encode_stream_packet(pose, message_tag=0)


# Command handling -----------------------------------------------------------------------


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
    """One client's subscription channel and its head-pose stream state."""

    def __init__(self, subscription_id: int, connection: "PipeConnection"):
        self.subscription_id = subscription_id
        self.connection = connection
        self.task = None
        self.hub = self.queue = None

    @property
    def streaming(self) -> bool:
        return self.task is not None

    def start(self, hub: HeadPoseHub) -> None:
        if self.task is None:
            self.hub, self.queue = hub, hub.subscribe()
            self.task = asyncio.create_task(self._push())

    def stop(self) -> None:
        if self.task is not None:
            self.task.cancel()
            self.hub.unsubscribe(self.queue)
            self.task = None

    async def _push(self) -> None:
        while True:
            sample = await self.queue.get()
            self.connection.send_stream(stream_package(sample))


class PipeConnection:
    """Service side of one relayed pipe connection (one channel of one client)."""

    def __init__(self, service: "PrpService", reader, writer, client_pid: int, pipe: str):
        self.service, self.reader, self.writer = service, reader, writer
        self.client_pid, self.pipe = client_pid, pipe
        self.channel = None
        self.subscription = None
        self.tail = b""

    def send(self, message: bytes) -> None:
        log.debug("%s -> %s", self.pipe, message.hex())
        self.writer.write(encode_frame(FRAME_MESSAGE, message))

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
        await self.handshake()
        try:
            while True:
                packets, self.tail = split_packets(self.tail + await self.message())
                for packet in packets:
                    self.handle_packet(packet)
                await self.writer.drain()
        finally:
            if self.subscription is not None:
                self.subscription.stop()
                self.service.subscriptions.pop(self.subscription.subscription_id, None)

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
        header = request_header(packet)
        try:
            request = decode_command_request(packet)
        except ProtocolError:
            log.info("command %#x tag %d not supported", header.command, header.tag)
            self.send(error_response(header.tag, header.command, STATUS_NOT_SUPPORTED))
            return
        self.send(encode_command_response(self.service.respond(self, request)))


class PrpService:
    """Shared state: pipe names, subscription ids and the sample hub."""

    def __init__(self, hub: HeadPoseHub, name: str = DEFAULT_NAME, serial: str | None = None,
                 clock=service_clock_us):
        self.hub, self.name, self.clock = hub, name, clock
        self.serial = serial or name
        self.subscription_pipe = str(uuid.uuid4())
        self.timesync_pipe = str(uuid.uuid4())
        self.subscriptions: dict[int, Subscription] = {}
        self.ids = itertools.count(1)
        self.sent_packages = self.dropped_packages = 0

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
        if request.stream != HEAD_POSE:
            return CommandResponse(request.tag, request.command, STATUS_NOT_SUPPORTED,
                                   request.stream)
        if request.command == COMMAND_STREAM_START:
            subscription.start(self.hub)
            log.info("head pose stream started for subscription %d", subscription.subscription_id)
            return CommandResponse(request.tag, request.command, STATUS_OK, request.stream)
        if not subscription.streaming:
            return CommandResponse(request.tag, request.command, STATUS_NOT_SUBSCRIBED,
                                   request.stream)
        subscription.stop()
        log.info("head pose stream stopped for subscription %d", subscription.subscription_id)
        return CommandResponse(request.tag, request.command, STATUS_OK, request.stream)

    def connect(self, connection: PipeConnection, request) -> CommandResponse:
        subscription = self.subscriptions.get(request.connect.subscription_client_id)
        if subscription is None:
            return CommandResponse(request.tag, COMMAND_CONNECT, STATUS_INVALID_PARAMETER,
                                   connect=device_connect_response(self.serial, 0))
        connection.subscription = subscription
        licenses = len(request.connect.licenses)
        log.info("connect from pid %d with %d license(s), subscription %d",
                 request.connect.process_id, licenses, subscription.subscription_id)
        return CommandResponse(request.tag, COMMAND_CONNECT, STATUS_OK,
                               connect=device_connect_response(self.serial, licenses))

    async def handle(self, reader, writer) -> None:
        peer = writer.get_extra_info("peername")
        try:
            kind, payload = await read_frame(reader)
            if kind == FRAME_BRIDGE_HELLO:
                writer.write(encode_frame(FRAME_PIPES, encode_names(self.pipe_names)))
                await writer.drain()
                log.info("bridge %s attached", peer)
                await reader.read()
            elif kind == FRAME_PIPE_OPEN:
                client_pid, pipe = decode_pipe_open(payload)
                await PipeConnection(self, reader, writer, client_pid, pipe).run()
            else:
                raise ProtocolError(f"unexpected first relay frame {kind}")
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        except ProtocolError as exc:
            log.warning("closing relay connection %s: %s", peer, exc)
        finally:
            writer.close()

    async def serve(self, host: str = "127.0.0.1", port: int = DEFAULT_PORT):
        return await asyncio.start_server(self.handle, host, port)


async def _main(args) -> None:
    hub = HeadPoseHub()
    clock = wine_clock(args.wine_clock)
    service = PrpService(hub, args.name, clock=clock)
    server = await service.serve(args.host, args.port)
    log.info("PRP service on %s, pipes %s", server.sockets[0].getsockname()[:2], service.pipe_names)
    producer = asyncio.create_task(publish_synthetic(hub, clock=clock)) if args.synthetic else None
    async with server:
        await server.serve_forever()
    if producer:
        producer.cancel()


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(description="Serve PRP head pose to a Wine pipe bridge.")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--name", default=DEFAULT_NAME, help="device name in tobii-prp://<name>")
    parser.add_argument("--synthetic", action="store_true",
                        help="publish a synthetic head sweep instead of tracker output")
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
