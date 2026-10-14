"""Production tet-tcp endpoint: the game DLL's legacy tracker route served from the service's session.

The game DLL reports `tet-tcp://127.0.0.1` when TCP 127.0.0.1:4455 accepts a
connection and then runs its legacy TTP module against that port
(docs/tet-tcp.md). Unlike the recording proxy in tet_tcp.py, nothing a game
sends reaches the tracker here:

- the startup replies (protocol upgrade, unlock_ppu) and the getters the module
  asks for are the device's own replies, read once per tracker session before
  its streams start (`DeviceReplies.capture`);
- subscribe and unsubscribe of gaze (0x500) and presence (0x504) only change
  which connections receive the service's stream, which every connection gets
  from the one device subscription the service holds;
- everything else, including every write, is refused with the device's
  unsupported-operation status.

Stream messages keep their bytes except column 1, the device timestamp, which
becomes the game's QueryPerformanceCounter microseconds: the module passes it
through unchanged because timesync (UDP 4457) never connects under Wine.
The port listens only while a tracker session runs.
"""
import asyncio
from dataclasses import dataclass, field
import itertools
import logging
import struct

from tobii_linux.frames import COLUMN_MEMBERS, COLUMN_STRUCT, FRAME_STRUCT, struct_code, take_value
from tobii_linux.ttp import Decoder, Field, Message, ProtocolError, fields
from tobii_linux.ttp_commands import (Command, FIELD_INT64, REQUEST_KIND, RESPONSE_KIND,
                                      STREAM_DATA_KIND, request)
from tobii_linux.ttp_fields import value
from tobii_linux.tet_tcp import (COMMAND_PORT, DISPLAY_AREA_WRITABLE, PERSISTENT_FILE_ENUMERATE,
                                 UNSUPPORTED_OPERATION, subscribed_stream)

log = logging.getLogger("tobii_linux.tet_tcp_service")

GAZE_STREAM = 0x500
PRESENCE_STREAM = 0x504
STREAMS = (GAZE_STREAM, PRESENCE_STREAM)
# Getters the legacy module and the game layer sent during device start (docs/tet-tcp.md).
CAPTURED_GETTERS = (
    Command.ENUMERATE_STREAM_TYPES, Command.GET_DEVICE_PROPERTIES, Command.GET_CAPABILITIES,
    Command.GET_TRACK_BOX, Command.GET_DEVICE_INFO, Command.GET_DISPLAY_AREA,
    DISPLAY_AREA_WRITABLE, Command.GET_DEVICE_STATUS, Command.GET_FREQUENCIES,
    Command.GET_DEVICE_NAME, Command.GET_GEOMETRY_MOUNTING, PERSISTENT_FILE_ENUMERATE)
CAPTURE_TIMEOUT_S = 1.0
# Device replies to subscribe and unsubscribe carry this second header word and no payload.
REPLY_WORD2 = 1
MAX_PENDING_BYTES = 64 * 1024
TIMESTAMP_COLUMN = 1


@dataclass
class DeviceReplies:
    """Device replies by command, and the wire streams the service has subscribed."""
    replies: dict = field(default_factory=dict)
    streams: frozenset = frozenset()

    @classmethod
    def capture(cls, session, commands=CAPTURED_GETTERS, timeout_s=CAPTURE_TIMEOUT_S):
        """The session's startup replies plus one reply per getter; call before streams start."""
        replies = dict(getattr(session, "startup_replies", {}))
        for command in commands:
            reply = session.exchange(lambda rid, c=command: request(rid, c), timeout_s)
            if reply is None:
                log.info("tracker gave no reply to %d; tet-tcp clients get an error", command)
            else:
                replies[int(command)] = reply
        return cls(replies)

    def display_area(self):
        from tobii_linux.display_setup import display_area_from_reply
        reply = self.replies.get(Command.GET_DISPLAY_AREA)
        if reply is None or reply.word4:
            return None
        try:
            return display_area_from_reply([value(f) for f in fields(reply.payload)])
        except (ProtocolError, ValueError):
            return None


def restamp(payload: bytes, timestamp_us: int) -> bytes:
    """Stream payload with the int64 of column 1 replaced; every other byte unchanged."""
    items = fields(payload)
    count, kind = struct_code(items[0])
    if kind != FRAME_STRUCT:
        raise ProtocolError("stream payload does not start with a column set")
    index = 1
    for _ in range(count):
        if index + 2 >= len(items) or struct_code(items[index]) != (COLUMN_MEMBERS, COLUMN_STRUCT):
            raise ProtocolError("malformed column set")
        column, data = items[index + 1], items[index + 2]
        if column.data == struct.pack(">I", TIMESTAMP_COLUMN) and data.tag == FIELD_INT64:
            items[index + 2] = Field(FIELD_INT64, struct.pack(">q", timestamp_us))
            return b"\0\0" + b"".join(item.encode() for item in items)
        index = take_value(items, index + 2)
    raise ProtocolError("stream message without a timestamp column")


def reply_to(message: Message, cached: Message) -> Message:
    return Message(RESPONSE_KIND, message.request_id, cached.word2, message.command,
                   cached.word4, cached.payload)


def refusal(message: Message) -> Message:
    return Message(RESPONSE_KIND, message.request_id, 0, message.command, UNSUPPORTED_OPERATION)


class TetTcpConnection:
    def __init__(self, service: "TetTcpService", reader, writer, number: int):
        self.service, self.reader, self.writer, self.number = service, reader, writer, number
        self.streams: set[int] = set()

    def send(self, message: Message) -> None:
        if not self.writer.is_closing():
            self.writer.write(message.encode())

    def send_stream(self, data: bytes) -> None:
        if self.writer.is_closing():
            return
        if self.writer.transport.get_write_buffer_size() > MAX_PENDING_BYTES:
            self.service.dropped += 1
            return
        self.writer.write(data)
        self.service.sent += 1

    async def run(self) -> None:
        decoder = Decoder()
        while data := await self.reader.read(65536):
            for message in decoder.feed(data):
                self.handle(message)
            await self.writer.drain()

    def handle(self, message: Message) -> None:
        if message.kind != REQUEST_KIND:
            return
        replies = self.service.device.replies
        if message.command in (Command.SUBSCRIBE_STREAM, Command.UNSUBSCRIBE_STREAM):
            self.send(self.subscription(message))
        elif message.command in replies:
            self.send(reply_to(message, replies[message.command]))
        else:
            log.info("tet-tcp connection %d: command %d refused", self.number, message.command)
            self.send(refusal(message))

    def subscription(self, message: Message) -> Message:
        stream = subscribed_stream(message)
        if stream not in STREAMS or stream not in self.service.device.streams:
            log.info("tet-tcp connection %d: stream %s not served", self.number, stream)
            return refusal(message)
        if message.command == Command.SUBSCRIBE_STREAM:
            self.streams.add(stream)
        else:
            self.streams.discard(stream)
        log.info("tet-tcp connection %d: %s %#x", self.number,
                 "subscribe" if message.command == Command.SUBSCRIBE_STREAM else "unsubscribe",
                 stream)
        return Message(RESPONSE_KIND, message.request_id, REPLY_WORD2, message.command, 0)


class TetTcpService:
    """Listens on 127.0.0.1:4455 while a tracker session runs; all calls on the event loop.

    `game_time` maps service-clock microseconds onto the game's
    QueryPerformanceCounter microseconds (qpc_clock.CounterMapping).
    """

    def __init__(self, game_time=lambda us: us, host: str = "127.0.0.1", port: int = COMMAND_PORT):
        self.game_time, self.host, self.port = game_time, host, port
        self.device = DeviceReplies()
        self.server = None
        self.connections: set[TetTcpConnection] = set()
        self.numbers = itertools.count(1)
        self.sent = self.dropped = 0

    async def attach(self, device: DeviceReplies) -> None:
        self.device = device
        if self.server is None:
            self.server = await asyncio.start_server(self.accept, self.host, self.port)
            log.info("tet-tcp route on %s:%d (streams %s)", self.host, self.port,
                     ", ".join(f"{s:#x}" for s in sorted(device.streams)))

    async def detach(self) -> None:
        server, self.server = self.server, None
        if server is not None:
            server.close()
            log.info("tet-tcp route closed with %d connection(s)", len(self.connections))
        for connection in list(self.connections):
            connection.writer.close()

    @property
    def port_bound(self) -> int | None:
        return self.server.sockets[0].getsockname()[1] if self.server else None

    async def accept(self, reader, writer) -> None:
        connection = TetTcpConnection(self, reader, writer, next(self.numbers))
        self.connections.add(connection)
        try:
            await connection.run()
        except (ConnectionError, ProtocolError) as error:
            log.info("tet-tcp connection %d ended: %s", connection.number, error)
        finally:
            self.connections.discard(connection)
            writer.close()

    def publish(self, message: Message, service_us: int) -> None:
        """One device stream message, received at `service_us` on the service clock."""
        if message.kind != STREAM_DATA_KIND or message.command not in STREAMS:
            return
        targets = [c for c in self.connections if message.command in c.streams]
        if not targets:
            return
        payload = restamp(message.payload, self.game_time(service_us))
        data = Message(message.kind, message.request_id, message.word2, message.command,
                       message.word4, payload).encode()
        for connection in targets:
            connection.send_stream(data)
