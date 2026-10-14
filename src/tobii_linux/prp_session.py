"""Offline codecs for the bundled game DLL's PRP client session.

Covers the named-pipe connection handshake, transport framing, and the command
request/response messages the client sends to obtain head pose: connect (1),
stream start (3), stream stop (4) and timesync (22), and the property messages of
the game integration layer: get (operation 5, answered by operation 6), subscribe
(command 5) and unsubscribe (command 6). These are pure codecs; they do not open
pipes or sockets and do not implement a service.

Recovered from DLL SHA256 35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020.
See docs/service-transport.md for locations, evidence and limits.
"""

from dataclasses import dataclass
import struct

from tobii_linux.prp import MAGIC, ProtocolError, encode_envelope, decode_envelope

URL_SCHEME = "tobii-prp://"
SERVER_PIPE_PREFIX = "TOBIIPRP-"
PIPE_ROOT = "\\\\.\\pipe\\"
CLIENT_PIPE_FORMAT = "client_{server}_{pid:08x}_{counter:016x}"

COMMAND_CHANNEL = 0x4E21
SUBSCRIPTION_CHANNEL = 0x4E22
TIMESYNC_CHANNEL = 0x4E23
CHANNELS = (COMMAND_CHANNEL, SUBSCRIPTION_CHANNEL, TIMESYNC_CHANNEL)

HANDSHAKE_VERSION = 1
HANDSHAKE_ACK = 1
# Server-side buffer for the handshake information message (0x18008f470).
MAX_HANDSHAKE_INFO = 0x100
# Client WriteFile and ReadFile chunk size (0x18004f250, 0x180090ad0).
PIPE_CHUNK = 0x1000

OPERATION_STREAM_PACKAGE = 1
OPERATION_PROPERTY_NOTIFICATION = 2
OPERATION_PROPERTY_GET = 5
OPERATION_PROPERTY_GET_RESPONSE = 6
OPERATION_COMMAND_REQUEST = 9
OPERATION_COMMAND_RESPONSE = 10

COMMAND_CONNECT = 1
COMMAND_STREAM_START = 3
COMMAND_STREAM_STOP = 4
COMMAND_TIMESYNC = 0x16
COMMAND_PROPERTY_SUBSCRIBE = 5
COMMAND_PROPERTY_UNSUBSCRIBE = 6
WORD_RESPONSE_COMMANDS = (COMMAND_STREAM_START, COMMAND_STREAM_STOP,
                          COMMAND_PROPERTY_UNSUBSCRIBE)

STATUS_NAMES = (
    "NO_ERROR", "INTERNAL", "NOT_SUPPORTED", "CONNECTION_FAILED",
    "ALREADY_SUBSCRIBED", "NOT_SUBSCRIBED", "UNKNOWN_OPERATION", "OPERATION_FAILED",
    "INVALID_PARAMETER", "VERSION_MISMATCH", "ALLOCATION_FAILED", "NOT_AVAILABLE",
    "UNKNOWN_DEVICE_URL", "ALREADY_CREATED", "CALIBRATION_INPROGRESS",
    "CALIBRATION_NOT_STARTED", "TOO_MANY_SUBSCRIBERS", "INSUFFICIENT_LICENSE",
    "CONNECTION_FAILED_DRIVER", "UNAUTHORIZED",
)
STATUS_OK = 0
STATUS_NOT_SUPPORTED = 2

CONNECT_REQUEST_BLOCK = 0x3C
CONNECT_REQUEST_BLOCK_EXTENDED = 0x40
CONNECT_RESPONSE_BLOCK = 0x98
CONNECT_RESPONSE_BLOCK_EXTENDED = 0xA8

# Decoder capacity limits for connect response lists (0x180055780, operation 10).
LIMITS = dict(list_a=0x19, list_b=0x19, list_c=0x19, list_d=0x19, streams=0x20,
              supported_commands=0x21, groups=5, group_values=0x20, license_results=0x100,
              list_g=0x20)
STRING_LIMITS = (0x100, 0x100, 0x100, 0x100, 0x100, 0x80, 0x80)
EXTENDED_STRING_LIMIT = 0x80
BUILD_VERSION_LIMIT = 0x100


def _u32(value: int) -> int:
    if not 0 <= value <= 0xFFFFFFFF:
        raise ProtocolError("value is outside unsigned 32-bit range")
    return value


def _i32(value: int) -> int:
    if not -(1 << 31) <= value < (1 << 31):
        raise ProtocolError("value is outside signed 32-bit range")
    return value & 0xFFFFFFFF


def _signed(value: int) -> int:
    return value - (1 << 32) if value >> 31 else value


def _ascii(text: str, limit: int) -> bytes:
    raw = text.encode("ascii")
    if b"\0" in raw or len(raw) >= limit:
        raise ProtocolError("string must be ASCII without NUL and fit its buffer")
    return raw


# Pipe naming and handshake ------------------------------------------------------------


def server_pipe_names(url: str) -> tuple[str, str]:
    """Pipe names the client tries for a tobii-prp:// URL, in order.

    The client looks for TOBIIPRP-<name> in a \\\\.\\pipe\\* listing; if it is not
    listed it opens <name> unchanged (0x18004e8a0, 0x18008fb20).
    """
    if not url.startswith(URL_SCHEME) or len(url) == len(URL_SCHEME):
        raise ProtocolError("expected a non-empty tobii-prp:// URL")
    name = url[len(URL_SCHEME):]
    return SERVER_PIPE_PREFIX + name, name


def client_pipe_name(server_pipe: str, pid: int, counter: int) -> str:
    """Name of the inbound pipe the client creates for one channel."""
    if not 0 <= counter < (1 << 64):
        raise ProtocolError("counter is outside unsigned 64-bit range")
    return CLIENT_PIPE_FORMAT.format(server=server_pipe, pid=_u32(pid), counter=counter)


@dataclass(frozen=True)
class ClientHello:
    channel: int
    reply_pipe: str
    version: int = HANDSHAKE_VERSION


def encode_client_hello(hello: ClientHello) -> bytes:
    name = _ascii(hello.reply_pipe, 0xF8)
    return struct.pack("<II", _u32(hello.version), _u32(hello.channel)) + name + b"\0"


def decode_client_hello(message: bytes) -> ClientHello:
    """Parse the first client message as the PRP-mode server does (0x18008f470)."""
    if len(message) < 9:
        raise ProtocolError("truncated client hello")
    version, channel = struct.unpack_from("<II", message)
    end = message.find(b"\0", 8)
    if end < 0:
        raise ProtocolError("unterminated client pipe name")
    if version == 0:
        raise ProtocolError("server rejects handshake version 0")
    return ClientHello(channel, message[8:end].decode("ascii"), version)


def encode_handshake_ack() -> bytes:
    return struct.pack("<I", HANDSHAKE_ACK)


def check_handshake_ack(message: bytes) -> None:
    """Client accepts exactly one four-byte value 1 (0x18004f300, 0x18004f00b)."""
    if len(message) != 4 or struct.unpack("<I", message)[0] != HANDSHAKE_ACK:
        raise ProtocolError("client rejects handshake acknowledgement")


def encode_client_pid(pid: int) -> bytes:
    return struct.pack("<I", _u32(pid))


def decode_client_pid(message: bytes) -> int:
    if len(message) < 4:
        raise ProtocolError("truncated client process id")
    return struct.unpack_from("<I", message)[0]


@dataclass(frozen=True)
class HandshakeInfo:
    client_id: int
    subscription_url: str
    timesync_url: str


def encode_handshake_info(info: HandshakeInfo) -> bytes:
    first = _ascii(info.subscription_url, MAX_HANDSHAKE_INFO)
    second = _ascii(info.timesync_url, MAX_HANDSHAKE_INFO)
    message = struct.pack("<I", _i32(info.client_id)) + first + b"\0" + second + b"\0"
    if len(message) > MAX_HANDSHAKE_INFO:
        raise ProtocolError("handshake information exceeds the server buffer")
    return message


def decode_handshake_info(message: bytes) -> HandshakeInfo:
    """Parse with the client's rule: id plus two NUL-terminated strings (0x18004f360)."""
    if len(message) < 6:
        raise ProtocolError("handshake information needs an id and two strings")
    first = message.find(b"\0", 4)
    second = message.find(b"\0", first + 1) if first >= 0 else -1
    if first < 0 or second < 0:
        raise ProtocolError("unterminated handshake string")
    return HandshakeInfo(_signed(struct.unpack_from("<I", message)[0]),
                         message[4:first].decode("ascii"),
                         message[first + 1:second].decode("ascii"))


# Transport framing --------------------------------------------------------------------


def pipe_chunks(packet: bytes) -> list[bytes]:
    """Split one packet into the client's WriteFile chunks (one pipe message each)."""
    return [packet[i:i + PIPE_CHUNK] for i in range(0, len(packet), PIPE_CHUNK)] or [b""]


def split_packets(stream: bytes, *,
                  max_payload: int = 16 * 1024 * 1024) -> tuple[list[bytes], bytes]:
    """Return complete TPRP packets from reassembled bytes and the incomplete tail."""
    packets, offset = [], 0
    while len(stream) - offset >= 12:
        magic, size, check = struct.unpack_from("<III", stream, offset)
        if magic != MAGIC or check != size ^ MAGIC or size > max_payload:
            raise ProtocolError("invalid PRP header in stream")
        if len(stream) - offset < 12 + size:
            break
        packets.append(stream[offset:offset + 12 + size])
        offset += 12 + size
    return packets, stream[offset:]


# Block writer and reader ---------------------------------------------------------------


class _Writer:
    """Fixed blocks, each followed by a length-prefixed variable region (0x18004fcc0)."""

    def __init__(self):
        self.buf = bytearray()
        self.region = None

    def block(self, words: int) -> int:
        self._close()
        start = len(self.buf)
        self.buf += struct.pack("<I", 4 * words) + bytes(4 * words - 4)
        self.region = len(self.buf)
        self.buf += bytes(4)
        return start

    def set(self, block: int, index: int, value: int) -> None:
        struct.pack_into("<I", self.buf, block + 4 * index, value & 0xFFFFFFFF)

    def data(self, raw: bytes) -> int:
        offset = len(self.buf) - self.region
        self.buf += raw + bytes(-len(raw) % 4)
        return offset

    def words(self, values) -> int:
        return self.data(b"".join(struct.pack("<I", _i32(v)) for v in values))

    def reserve(self, size: int) -> int:
        offset = len(self.buf) - self.region
        self.buf += bytes(size)
        return offset

    def put(self, offset: int, fmt: str, *values) -> None:
        struct.pack_into(fmt, self.buf, self.region + offset, *values)

    def raw(self, block: bytes, region: bytes) -> None:
        """Append an already encoded block and its region."""
        self._close()
        self.buf += block
        self.region = len(self.buf)
        self.buf += region

    def _close(self):
        if self.region is not None:
            struct.pack_into("<I", self.buf, self.region, len(self.buf) - self.region)

    def finish(self) -> bytes:
        self._close()
        return encode_envelope(bytes(self.buf) + bytes(4))


def _message(tag: int, operation: int) -> _Writer:
    writer = _Writer()
    header = writer.block(3)
    writer.set(header, 1, _u32(tag))
    writer.set(header, 2, operation)
    return writer


def _chain(payload: bytes) -> list[tuple[bytes, bytes]]:
    """Split a payload into (block, region) pairs as validated by 0x180053d10."""
    pairs, offset = [], 0
    while True:
        if offset + 4 > len(payload):
            raise ProtocolError("PRP block chain overruns the payload")
        size = struct.unpack_from("<I", payload, offset)[0]
        if size == 0:
            if offset + 4 != len(payload):
                raise ProtocolError("PRP block chain terminator is not final")
            break
        region = offset + size
        if size < 4 or region + 4 > len(payload):
            raise ProtocolError("PRP block overruns the payload")
        region_size = struct.unpack_from("<I", payload, region)[0]
        if region_size < 4 or region + region_size > len(payload):
            raise ProtocolError("PRP region overruns the payload")
        pairs.append((payload[offset:region], payload[region:region + region_size]))
        offset = region + region_size
    if len(pairs) < 2:
        raise ProtocolError("PRP message lacks header or operation block")
    return pairs


def _words(block: bytes, minimum: int) -> tuple[int, ...]:
    if len(block) < minimum:
        raise ProtocolError("PRP block is shorter than required")
    return struct.unpack("<%dI" % (len(block) // 4), block[:len(block) // 4 * 4])


def _bytes_at(region: bytes, length: int, offset: int) -> bytes:
    if length == 0 or offset == 0:
        return b""
    if offset + length > len(region):
        raise ProtocolError("PRP field overruns its region")
    return region[offset:offset + length]


def _string_at(region: bytes, offset: int, limit: int) -> str:
    if offset >= len(region):
        raise ProtocolError("PRP string overruns its region")
    end = region.find(b"\0", offset)
    if end < 0:
        raise ProtocolError("unterminated PRP string")
    return region[offset:min(end, offset + limit - 1)].decode("ascii")


def _ints_at(region: bytes, count: int, offset: int, limit: int) -> tuple[int, ...]:
    count = min(_signed(count), limit)
    if count <= 0:
        return ()
    if offset + 4 * count > len(region):
        raise ProtocolError("PRP list overruns its region")
    return tuple(_signed(v) for v in struct.unpack_from("<%dI" % count, region, offset))


def _header(packet: bytes) -> tuple[int, int, list[tuple[bytes, bytes]]]:
    pairs = _chain(decode_envelope(packet))
    header = _words(pairs[0][0], 12)
    return header[1], header[2], pairs[1:]


# Command requests ---------------------------------------------------------------------


@dataclass(frozen=True)
class ConnectRequest:
    """Command 1 body serialized by 0x18004fdd0 from the native request at 0x18001fed0."""
    subscription_client_id: int
    licenses: tuple[bytes, ...] = ()
    signer_public_key: bytes = b""
    client_version: tuple[int, int, int, int] = (4, 17, 0, 6)
    process_id: int = 0
    image_directory: bytes = b""
    image_name: bytes = b""
    field_of_use: int = 1


@dataclass(frozen=True)
class CommandRequest:
    tag: int
    command: int
    stream: int | None = None
    connect: ConnectRequest | None = None


def encode_connect_request(tag: int, request: ConnectRequest, *, extended: bool = True) -> bytes:
    if len(request.licenses) > 0x100 or len(request.client_version) != 4:
        raise ProtocolError("connect request has too many licenses or a bad version tuple")
    writer = _message(tag, OPERATION_COMMAND_REQUEST)
    writer.set(writer.block(2), 1, COMMAND_CONNECT)
    size = CONNECT_REQUEST_BLOCK_EXTENDED if extended else CONNECT_REQUEST_BLOCK
    body = writer.block(size // 4)
    writer.set(body, 1, _i32(request.subscription_client_id))
    writer.set(body, 2, len(request.licenses))
    table = writer.reserve(8 * len(request.licenses))
    writer.set(body, 3, table)
    for index, license in enumerate(request.licenses):
        offset = writer.data(license) if license else 0
        writer.put(table + 8 * index, "<II", len(license) if license else 0, offset)
    for slot, raw in ((4, request.signer_public_key), (12, request.image_name),
                      (10, request.image_directory)):
        if raw:
            writer.set(body, slot, len(raw))
            writer.set(body, slot + 1, writer.data(raw))
    for index, value in enumerate(request.client_version):
        writer.set(body, 6 + index, _i32(value))
    writer.set(body, 14, _u32(request.process_id))
    if extended:
        writer.set(body, 15, _i32(request.field_of_use))
    return writer.finish()


def encode_stream_request(tag: int, command: int, stream: int) -> bytes:
    """Stream start (3) or stop (4) carrying one PRP stream id."""
    if command not in (COMMAND_STREAM_START, COMMAND_STREAM_STOP):
        raise ProtocolError("not a stream command")
    writer = _message(tag, OPERATION_COMMAND_REQUEST)
    writer.set(writer.block(2), 1, command)
    writer.set(writer.block(2), 1, _i32(stream))
    return writer.finish()


def encode_timesync_request(tag: int) -> bytes:
    writer = _message(tag, OPERATION_COMMAND_REQUEST)
    writer.set(writer.block(2), 1, COMMAND_TIMESYNC)
    writer.block(1)
    return writer.finish()


def decode_command_request(packet: bytes, *, extended: bool = True) -> CommandRequest:
    """Server-side view of the commands this client sends during head-pose sessions."""
    tag, operation, pairs = _header(packet)
    if operation != OPERATION_COMMAND_REQUEST or len(pairs) < 2:
        raise ProtocolError("not a PRP command request")
    command = _words(pairs[0][0], 8)[1]
    block, region = pairs[1]
    if command in (COMMAND_STREAM_START, COMMAND_STREAM_STOP):
        return CommandRequest(tag, command, stream=_signed(_words(block, 8)[1]))
    if command == COMMAND_TIMESYNC:
        _words(block, 4)
        return CommandRequest(tag, command)
    if command != COMMAND_CONNECT:
        raise ProtocolError("command is outside the head-pose session subset")
    w = _words(block, CONNECT_REQUEST_BLOCK)
    count = min(_signed(w[2]), 0x100)
    licenses = []
    for index in range(max(count, 0)):
        length, offset = struct.unpack_from("<II", region, w[3] + 8 * index)
        licenses.append(_bytes_at(region, length, offset))
    return CommandRequest(tag, command, connect=ConnectRequest(
        _signed(w[1]), tuple(licenses), _bytes_at(region, w[4], w[5]),
        tuple(_signed(v) for v in w[6:10]), w[14], _bytes_at(region, w[10], w[11]),
        _bytes_at(region, w[12], w[13]),
        _signed(w[15]) if extended and len(block) >= CONNECT_REQUEST_BLOCK_EXTENDED else 0))


# Command responses --------------------------------------------------------------------


@dataclass(frozen=True)
class ConnectResponse:
    """Command 1 response body, serialized by 0x180050370 and stored by the device.

    Field names follow observed consumers; lists without a recovered consumer keep
    neutral names. streams holds PRP stream ids checked by stream support
    (0x18002c6d0); license_results must have one entry per requested license
    (0x18002a6a0); firmware_upgrade_in_progress must be false for device creation.
    supported_commands is compared with PRP command ids: 0x16 (timesync) makes
    device creation run up to four timesync rounds (0x180030630), and capability
    queries search it (0x18002cd30). Treating it as a command list is an inference.
    """
    build_version: str = ""
    firmware_upgrade_in_progress: bool = False
    list_a: tuple[int, ...] = ()
    list_b: tuple[int, ...] = ()
    list_c: tuple[int, ...] = ()
    list_d: tuple[int, ...] = ()
    streams: tuple[int, ...] = ()
    supported_commands: tuple[int, ...] = ()
    groups: tuple[tuple[int, tuple[int, ...]], ...] = ()
    strings: tuple[str, ...] = ("",) * 7
    words: tuple[int, ...] = (0,) * 9
    license_results: tuple[int, ...] = ()
    mode: int = 0
    list_g: tuple[int, ...] = ()
    extended_strings: tuple[str, ...] = ("",) * 4


@dataclass(frozen=True)
class CommandResponse:
    tag: int
    command: int
    status: int
    value: int | None = None
    connect: ConnectResponse | None = None


def _check_connect(response: ConnectResponse) -> None:
    lists = dict(list_a=response.list_a, list_b=response.list_b, list_c=response.list_c,
                 list_d=response.list_d, streams=response.streams,
                 supported_commands=response.supported_commands,
                 license_results=response.license_results, list_g=response.list_g,
                 groups=response.groups)
    if any(len(v) > LIMITS[k] for k, v in lists.items()):
        raise ProtocolError("connect response list exceeds the client buffer")
    if any(len(values) > LIMITS["group_values"] or not 0 <= kind < 5
           for kind, values in response.groups):
        raise ProtocolError("connect response group exceeds the client buffer")
    if any(not 0 <= v <= 0x20 for v in response.supported_commands):
        raise ProtocolError("client discards supported command values above 0x20")
    if (len(response.strings), len(response.words), len(response.extended_strings)) != (7, 9, 4):
        raise ProtocolError("connect response has the wrong number of fixed fields")


def _encode_connect_body(writer: _Writer, response: ConnectResponse, extended: bool) -> None:
    _check_connect(response)
    size = CONNECT_RESPONSE_BLOCK_EXTENDED if extended else CONNECT_RESPONSE_BLOCK
    body = writer.block(size // 4)
    writer.set(body, 1, writer.data(_ascii(response.build_version, BUILD_VERSION_LIMIT) + b"\0"))
    writer.set(body, 2, int(bool(response.firmware_upgrade_in_progress)))
    for slot, values in ((3, response.list_a), (5, response.list_b), (7, response.list_c),
                         (9, response.list_d), (11, response.streams),
                         (13, response.supported_commands)):
        writer.set(body, slot, len(values))
        writer.set(body, slot + 1, writer.words(values))
    writer.set(body, 15, len(response.groups))
    table = writer.reserve(12 * len(response.groups))
    writer.set(body, 16, table)
    for index, (kind, values) in enumerate(response.groups):
        writer.put(table + 12 * index, "<III", kind, len(values), writer.words(values))
    for slot, (text, limit) in enumerate(zip(response.strings, STRING_LIMITS), start=17):
        writer.set(body, slot, writer.data(_ascii(text, limit) + b"\0"))
    for slot, value in enumerate(response.words, start=24):
        writer.set(body, slot, _i32(value))
    writer.set(body, 33, len(response.license_results))
    writer.set(body, 34, writer.words(response.license_results))
    writer.set(body, 35, _i32(response.mode))
    writer.set(body, 36, len(response.list_g))
    writer.set(body, 37, writer.words(response.list_g))
    if extended:
        for slot, text in enumerate(response.extended_strings, start=38):
            writer.set(body, slot, writer.data(_ascii(text, EXTENDED_STRING_LIMIT) + b"\0"))


def encode_command_response(response: CommandResponse, *, extended: bool = True) -> bytes:
    """Serialize operation 10 for connect, stream start/stop, timesync and unsubscribe."""
    writer = _message(response.tag, OPERATION_COMMAND_RESPONSE)
    header = writer.block(3)
    writer.set(header, 1, _i32(response.status))
    writer.set(header, 2, response.command)
    if response.command == COMMAND_CONNECT:
        _encode_connect_body(writer, response.connect or ConnectResponse(), extended)
    elif response.command in WORD_RESPONSE_COMMANDS:
        writer.set(writer.block(2), 1, _i32(response.value or 0))
    elif response.command == COMMAND_TIMESYNC:
        if not 0 <= (response.value or 0) < (1 << 64):
            raise ProtocolError("timesync value is outside unsigned 64-bit range")
        block = writer.block(3)
        writer.set(block, 1, (response.value or 0) >> 32)
        writer.set(block, 2, response.value or 0)
    else:
        raise ProtocolError("command is outside the head-pose session subset")
    return writer.finish()


def _decode_connect_body(block: bytes, region: bytes, extended: bool) -> ConnectResponse:
    w = _words(block, CONNECT_RESPONSE_BLOCK)
    lists = [_ints_at(region, w[slot], w[slot + 1], limit)
             for slot, limit in ((3, 0x19), (5, 0x19), (7, 0x19), (9, 0x19), (11, 0x20))]
    supported_commands = tuple(v for v in _ints_at(region, w[13], w[14], 0x7FFFFFFF)
                         if v <= 0x20)[:LIMITS["supported_commands"]]
    groups = []
    for index in range(max(_signed(w[15]), 0)):
        kind, count, offset = struct.unpack_from("<III", region, w[16] + 12 * index)
        if _signed(kind) < 5 and len(groups) < 5:
            groups.append((_signed(kind), _ints_at(region, count, offset, 0x20)))
    strings = tuple(_string_at(region, w[17 + i], limit) for i, limit in enumerate(STRING_LIMITS))
    has_extended = extended and len(block) >= CONNECT_RESPONSE_BLOCK_EXTENDED
    return ConnectResponse(
        _string_at(region, w[1], BUILD_VERSION_LIMIT), w[2] != 0, *lists, supported_commands,
        tuple(groups), strings, tuple(_signed(v) for v in w[24:33]),
        _ints_at(region, w[33], w[34], 0x100), _signed(w[35]),
        _ints_at(region, w[36], w[37], 0x20),
        tuple(_string_at(region, w[38 + i], EXTENDED_STRING_LIMIT) for i in range(4))
        if has_extended else ("",) * 4)


def decode_command_response(packet: bytes, *, extended: bool = True) -> CommandResponse:
    """Client-side view of operation 10 for the head-pose session commands."""
    tag, operation, pairs = _header(packet)
    if operation != OPERATION_COMMAND_RESPONSE or len(pairs) < 2:
        raise ProtocolError("not a PRP command response")
    _, status, command = _words(pairs[0][0], 12)[:3]
    block, region = pairs[1]
    status = _signed(status)
    if command == COMMAND_CONNECT:
        return CommandResponse(tag, command, status,
                               connect=_decode_connect_body(block, region, extended))
    if command in WORD_RESPONSE_COMMANDS:
        return CommandResponse(tag, command, status, _signed(_words(block, 8)[1]))
    if command == COMMAND_TIMESYNC:
        w = _words(block, 12)
        return CommandResponse(tag, command, status, w[1] << 32 | w[2])
    raise ProtocolError("command is outside the head-pose session subset")


# Property messages ---------------------------------------------------------------------

PROPERTY_DISPLAY_AREA = 1
PROPERTY_DISPLAY_ID = 0x12
DISPLAY_AREA_FLOATS = 9
STRING_PROPERTIES = (8, PROPERTY_DISPLAY_ID)
STRING_PROPERTY_LIMIT = 0x100


def encode_property_request(tag: int, property_id: int) -> bytes:
    """Operation 5 as the client sends it (0x180019f60): one block holding the property id."""
    writer = _message(tag, OPERATION_PROPERTY_GET)
    writer.set(writer.block(2), 1, property_id)
    return writer.finish()


def encode_property_subscribe_request(tag: int, property_id: int,
                                      command: int = COMMAND_PROPERTY_SUBSCRIBE) -> bytes:
    """Command 5 (subscribe, 0x180019830) or 6 (unsubscribe) with the property id."""
    if command not in (COMMAND_PROPERTY_SUBSCRIBE, COMMAND_PROPERTY_UNSUBSCRIBE):
        raise ProtocolError("not a property subscription command")
    writer = _message(tag, OPERATION_COMMAND_REQUEST)
    writer.set(writer.block(2), 1, command)
    writer.set(writer.block(2), 1, property_id)
    return writer.finish()


def decode_property_subscribe_request(packet: bytes) -> tuple[int, int, int]:
    """Tag, command (5 or 6) and property id of a property subscription request."""
    tag, operation, pairs = _header(packet)
    command = _words(pairs[0][0], 8)[1] if pairs else None
    if (operation != OPERATION_COMMAND_REQUEST or len(pairs) < 2
            or command not in (COMMAND_PROPERTY_SUBSCRIBE, COMMAND_PROPERTY_UNSUBSCRIBE)):
        raise ProtocolError("not a PRP property subscription request")
    return tag, command, _words(pairs[1][0], 8)[1]


def decode_property_request(packet: bytes) -> tuple[int, int]:
    """Tag and property id of an operation 5 request."""
    tag, operation, pairs = _header(packet)
    if operation != OPERATION_PROPERTY_GET:
        raise ProtocolError("not a PRP property get")
    return tag, _words(pairs[0][0], 8)[1]


@dataclass(frozen=True)
class PropertyResponse:
    """A property value with its status, as the raw value block and region.

    Sent as operation 6 for a get (operation 5) and inside the command 5 response for
    a subscribe; the value block layout depends on the property id.
    """
    tag: int
    status: int
    property_id: int
    block: bytes = struct.pack("<I", 4)
    region: bytes = struct.pack("<I", 4)


def encode_property_response(response: PropertyResponse) -> bytes:
    writer = _message(response.tag, OPERATION_PROPERTY_GET_RESPONSE)
    header = writer.block(3)
    writer.set(header, 1, _i32(response.status))
    writer.set(header, 2, response.property_id)
    writer.raw(response.block, response.region)
    return writer.finish()


def encode_property_subscribe_response(response: PropertyResponse) -> bytes:
    """Command 5 response: status, property id, then its current value (decoder 0x180055780)."""
    writer = _message(response.tag, OPERATION_COMMAND_RESPONSE)
    header = writer.block(3)
    writer.set(header, 1, _i32(response.status))
    writer.set(header, 2, COMMAND_PROPERTY_SUBSCRIBE)
    writer.set(writer.block(2), 1, response.property_id)
    writer.raw(response.block, response.region)
    return writer.finish()


def encode_property_notification(tag: int, property_id: int, block: bytes, region: bytes) -> bytes:
    """Operation 2 on the subscription channel: {8, property} then the value as in operation 6.

    Layout from the game DLL's serializer 0x18004f730 and decoder 0x180055780 (emulated).
    """
    writer = _message(tag, OPERATION_PROPERTY_NOTIFICATION)
    writer.set(writer.block(2), 1, property_id)
    writer.raw(block, region)
    return writer.finish()


def decode_property_response(packet: bytes) -> PropertyResponse:
    tag, operation, pairs = _header(packet)
    if operation != OPERATION_PROPERTY_GET_RESPONSE or len(pairs) < 2:
        raise ProtocolError("not a PRP property get response")
    _, status, property_id = _words(pairs[0][0], 12)[:3]
    return PropertyResponse(tag, _signed(status), property_id, *pairs[1])


def decode_property_subscribe_response(packet: bytes) -> PropertyResponse:
    tag, operation, pairs = _header(packet)
    if operation != OPERATION_COMMAND_RESPONSE or len(pairs) < 3:
        raise ProtocolError("not a PRP command response with a property value")
    _, status, command = _words(pairs[0][0], 12)[:3]
    if command != COMMAND_PROPERTY_SUBSCRIBE:
        raise ProtocolError("not a PRP property subscribe response")
    return PropertyResponse(tag, _signed(status), _words(pairs[1][0], 8)[1], *pairs[2])


def display_area_value(top_left, top_right, bottom_left) -> tuple[bytes, bytes]:
    """Value block for property 1: nine floats in millimetres (decoder 0x180055780)."""
    floats = (*top_left, *top_right, *bottom_left)
    if len(floats) != DISPLAY_AREA_FLOATS:
        raise ProtocolError("display area needs three corners of three coordinates")
    block = struct.pack("<I9f", 4 + 4 * DISPLAY_AREA_FLOATS, *floats)
    return block, struct.pack("<I", 4)


def string_value(text: str) -> tuple[bytes, bytes]:
    """Value block for string properties: a region offset, then the NUL-terminated text."""
    raw = _ascii(text, STRING_PROPERTY_LIMIT) + b"\0"
    raw += bytes(-len(raw) % 4)
    return struct.pack("<II", 8, 4), struct.pack("<I", 4 + len(raw)) + raw


def decode_display_area(response: PropertyResponse) -> tuple[float, ...]:
    if len(response.block) < 4 + 4 * DISPLAY_AREA_FLOATS:
        raise ProtocolError("display area value block is too short")
    return struct.unpack_from("<9f", response.block, 4)


def decode_string_value(response: PropertyResponse) -> str:
    offset = _words(response.block, 8)[1]
    return _string_at(response.region, offset, STRING_PROPERTY_LIMIT)


def minimal_connect_response(build_version: str, license_count: int,
                             streams: tuple[int, ...] = (2, 3, 29)) -> ConnectResponse:
    """Connect response satisfying the device_create checks recovered statically.

    Only the build version string, the firmware-upgrade flag, the stream list and the
    license-result count are known to be checked. supported_commands is left empty,
    so device creation skips its timesync rounds. All other fields are zero or empty;
    their effect on later API calls is not established.
    """
    return ConnectResponse(build_version=build_version, streams=tuple(streams),
                           license_results=(0,) * license_count)


@dataclass
class ClientSession:
    """Client tag counters: command tags (client+0x68cc) and timesync tags (client+0x5638).

    Both are incremented before use, so the first tag on each channel is 1.
    """
    command_tag: int = 0
    timesync_tag: int = 0

    def next_command_tag(self) -> int:
        self.command_tag = (self.command_tag + 1) & 0xFFFFFFFF
        return self.command_tag

    def next_timesync_tag(self) -> int:
        self.timesync_tag = (self.timesync_tag + 1) & 0xFFFFFFFF
        return self.timesync_tag
