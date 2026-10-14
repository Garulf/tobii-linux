"""Stream Engine Services protocol (SESP): the head-pose source of the game DLL's tet-tcp route.

The legacy TTP platform module that `tet-tcp://` devices get (docs/tet-tcp.md)
computes no head pose. It asks Stream Engine Services for it over the named
pipe `streamengineservices`, opened by its fixed name. The service side is
`ses_windows.dll` of the Tobii Windows software; the client side is
`sesp_services.cpp` in the game DLL. docs/sesp.md has the evidence.

Transport: per channel, the client writes `u32 channel, reply pipe name, NUL`
(command channel 0x2711, subscription channel 0x2712) and the service answers
on the reply pipe with one u32, its slot index for that connection. The
subscription channel's index is the subscription id the client then sends in
its initialize request. There is no pid or URL message as in PRP.

Messages: a 12-byte header `"sesp", u32 size, u32 size ^ 0x70736573`, then a
FlatBuffers buffer of that size laid out the way flatcc builds it (tables
front to back after the root offset, vtables clustered at the end, fields in
the order the builder adds them, fields equal to their default left out).
The root table holds a sequence number (field 0), the payload type (field 1,
u8) and the payload table (field 2). `encode_*` reproduce the original
service builders byte for byte (tools/sesp_oracle.py).
"""
from dataclasses import dataclass
import struct

PIPE_NAME = "streamengineservices"
URL = "tobii-sesp://" + PIPE_NAME
COMMAND_CHANNEL = 0x2711
SUBSCRIPTION_CHANNEL = 0x2712
CHANNELS = range(0x2711, 0x2715)

MAGIC = 0x70736573
HEADER = struct.Struct("<III")
MAX_MESSAGE = 1 << 20

SIGNAL_INVALID_OPERATION = 1
REQUEST_INITIALIZE = 2
RESPONSE_INITIALIZE = 3
REQUEST_HEADPOSE_START = 4
RESPONSE_HEADPOSE_START = 5
REQUEST_HEADPOSE_STOP = 6
RESPONSE_HEADPOSE_STOP = 7
SUBSCRIPTION_HEADPOSE = 8
REQUEST_GET_DISPLAY_INFO = 9
RESPONSE_GET_DISPLAY_INFO = 10
REQUEST_LIST_DEVICES = 0x10
REQUEST_NOTIFY_STREAM_STATUS = 0x1A
RESPONSE_NOTIFY_STREAM_STATUS = 0x1B
REQUEST_DEVICE_LIST_CHANGES_START = 0x21
REQUEST_SET_FW_UPGRADE_ALLOWED = 0x26
REQUEST_SEND_STATISTICS = 0x32
RESPONSE_SEND_STATISTICS = 0x33

ERROR_NONE = 0
ERROR_INTERNAL = 1
ERROR_NOT_SUPPORTED = 6
ERROR_NOT_FOUND = 7
# The service maps every internal error above 7 to 1 on the wire (ses_windows 0x1800c2d10).
WIRE_ERRORS = range(0, 8)

# Initialize response fields 1..15 are capability flags. The client keeps eight
# of them (0x18005c130); the ones it acts on:
CAPABILITY_HEAD_POSE = 1          # head-pose start, platmod head-pose stream (+0x2a24)
CAPABILITY_DISPLAY_INFO = 2       # get display info, platmod display id (+0x2a25)
CAPABILITY_STREAM_STATUS = 3      # notify stream status (+0x2a26); always set by the service
CAPABILITY_DEVICE_LIST = 9        # device list changes (+0x2a29)
CAPABILITY_COUNT = 15

VALID = 1
INVALID = 0


class SespError(ValueError):
    pass


# Framing ----------------------------------------------------------------------------------


def frame(buffer: bytes) -> bytes:
    return HEADER.pack(MAGIC, len(buffer), len(buffer) ^ MAGIC) + buffer


class FrameDecoder:
    """Reassembles messages from pipe reads of any size, as sesp_parse_message does."""

    def __init__(self, limit: int = MAX_MESSAGE):
        self.pending = b""
        self.limit = limit

    def feed(self, data: bytes) -> list[bytes]:
        self.pending += data
        buffers = []
        while len(self.pending) >= HEADER.size:
            magic, size, check = HEADER.unpack_from(self.pending)
            if magic != MAGIC or size ^ MAGIC != check:
                raise SespError("not a SESP message header")
            if size > self.limit:
                raise SespError("SESP message exceeds the local limit")
            if len(self.pending) < HEADER.size + size:
                break
            buffers.append(self.pending[HEADER.size:HEADER.size + size])
            self.pending = self.pending[HEADER.size + size:]
        return buffers


# flatcc-shaped buffer building ----------------------------------------------------------


@dataclass(frozen=True)
class Scalar:
    index: int
    format: str
    value: object


@dataclass(frozen=True)
class Reference:
    """Offset field pointing at an object emitted earlier (`position` from the builder)."""
    index: int
    position: int


class FlatccBuilder:
    """Back-to-front FlatBuffers builder with flatcc's layout for small tables.

    Objects are prepended to the front part; positions are kept relative to the
    boundary between the front part and the vtable cluster, which grows in
    creation order and shares identical vtables.
    """

    def __init__(self):
        self.front: list[bytes] = []
        self.front_size = 0
        self.vtables: list[bytes] = []
        self.vtable_size = 0
        self.vtable_positions: dict[bytes, int] = {}

    def _prepend(self, data: bytes, alignment: int = 4) -> int:
        padding = -len(data) % alignment
        self.front.append(data + bytes(padding))
        self.front_size += len(data) + padding
        return -self.front_size

    def string(self, text: bytes) -> int:
        return self._prepend(struct.pack("<I", len(text)) + text + b"\0")

    def _vtable(self, entries: bytes) -> int:
        if entries not in self.vtable_positions:
            self.vtable_positions[entries] = self.vtable_size
            self.vtables.append(entries)
            self.vtable_size += len(entries)
        return self.vtable_positions[entries]

    def table(self, fields) -> int:
        """Table of `fields` (Scalar or Reference) in the order the original builder adds them."""
        layout = bytearray(4)
        slots = {}
        references = []
        for field in fields:
            size = 4 if isinstance(field, Reference) else struct.calcsize("<" + field.format)
            layout += bytes(-len(layout) % size)
            slots[field.index] = len(layout)
            if isinstance(field, Reference):
                references.append((len(layout), field.position))
                layout += bytes(4)
            else:
                layout += struct.pack("<" + field.format, field.value)
        count = max(slots, default=-1) + 1
        entries = struct.pack(f"<HH{count}H", 4 + 2 * count, len(layout),
                              *(slots.get(index, 0) for index in range(count)))
        vtable = self._vtable(entries)
        position = -(self.front_size + len(layout) + -len(layout) % 4)
        struct.pack_into("<i", layout, 0, position - vtable)
        for offset, target in references:
            struct.pack_into("<I", layout, offset, target - (position + offset))
        return self._prepend(bytes(layout))

    def finish(self, root: int) -> bytes:
        position = self._prepend(b"\0\0\0\0")
        front = b"".join(reversed(self.front))
        front = struct.pack("<I", root - position) + front[4:]
        return front + b"".join(self.vtables)


def _scalars(values) -> list[Scalar]:
    """Fields (index, format, value, default) that differ from their default."""
    return [Scalar(index, form, value) for index, form, value, default in values
            if value != default]


def message(sequence: int, kind: int, payload_fields, strings=()) -> bytes:
    """One framed message: payload table, then the root table {sequence, type, payload}.

    `strings` are (field index, bytes) emitted before the payload table, which then
    refers to them.
    """
    builder = FlatccBuilder()
    references = [Reference(index, builder.string(text)) for index, text in strings]
    payload = builder.table(sorted(references + list(payload_fields), key=lambda f: f.index))
    root = builder.table(_scalars([(0, "I", sequence, 0)]) + [Reference(2, payload),
                                                               Scalar(1, "B", kind)])
    return frame(builder.finish(root))


# Service messages (ses_windows.dll builders) ---------------------------------------------


def wire_error(error: int) -> int:
    return error if error in WIRE_ERRORS else ERROR_INTERNAL


def encode_initialize_response(sequence: int, error: int, capabilities=()) -> bytes:
    """Type 3: error, then capability flags 1..15 (`capabilities` lists the set ones)."""
    flags = [(index, "B", 1 if index in capabilities else 0, 0)
             for index in range(1, CAPABILITY_COUNT + 1)]
    return message(sequence, RESPONSE_INITIALIZE,
                   _scalars([(0, "I", wire_error(error), 0)] + flags))


def encode_error_response(sequence: int, kind: int, error: int) -> bytes:
    """Responses that carry only an error: head-pose start (5) and stop (7), send statistics
    (0x33) and the invalid-operation signal (1)."""
    return message(sequence, kind, _scalars([(0, "I", wire_error(error), 0)]))


def encode_stream_status_response(sequence: int) -> bytes:
    """Type 0x1b, an empty table."""
    return message(sequence, RESPONSE_NOTIFY_STREAM_STATUS, [])


@dataclass(frozen=True)
class HeadPose:
    """tobii_head_pose_t as the service forwards it; validity 1 means valid."""
    timestamp_us: int
    position: tuple
    rotation: tuple
    position_validity: int = VALID
    rotation_validity: tuple = (VALID, VALID, VALID)


def _float32(value: float) -> float:
    return struct.unpack("<f", struct.pack("<f", value))[0]


def encode_head_pose(sequence: int, pose: HeadPose) -> bytes:
    """Type 8: timestamp high and low words, position, rotation, then the validities."""
    timestamp = pose.timestamp_us & 0xFFFFFFFFFFFFFFFF
    values = [(0, "I", timestamp >> 32, 0), (1, "I", timestamp & 0xFFFFFFFF, 0)]
    values += [(2 + i, "f", _float32(v), 0.0) for i, v in enumerate(pose.position)]
    values += [(5 + i, "f", _float32(v), 0.0) for i, v in enumerate(pose.rotation)]
    values += [(8, "i", 1 if pose.position_validity == VALID else 0, 1)]
    values += [(9 + i, "i", 1 if v == VALID else 0, 1) for i, v in enumerate(pose.rotation_validity)]
    return message(sequence, SUBSCRIPTION_HEADPOSE, _scalars(values))


# Reading ----------------------------------------------------------------------------------


class Table:
    """Field access on one FlatBuffers table, bounds-checked."""

    def __init__(self, buffer: bytes, position: int):
        self.buffer, self.position = buffer, position
        if position < 0 or position + 4 > len(buffer):
            raise SespError("table outside the buffer")
        vtable = position - struct.unpack_from("<i", buffer, position)[0]
        if vtable < 0 or vtable + 4 > len(buffer):
            raise SespError("vtable outside the buffer")
        self.vtable_size, self.table_size = struct.unpack_from("<HH", buffer, vtable)
        if vtable + self.vtable_size > len(buffer) or position + self.table_size > len(buffer):
            raise SespError("vtable or table runs past the buffer")
        self.vtable = vtable

    def _slot(self, index: int) -> int | None:
        entry = 4 + 2 * index
        if entry + 2 > self.vtable_size:
            return None
        offset = struct.unpack_from("<H", self.buffer, self.vtable + entry)[0]
        return self.position + offset if offset else None

    def scalar(self, index: int, form: str, default=0):
        slot = self._slot(index)
        if slot is None:
            return default
        if slot + struct.calcsize("<" + form) > len(self.buffer):
            raise SespError("field runs past the buffer")
        return struct.unpack_from("<" + form, self.buffer, slot)[0]

    def _target(self, index: int) -> int | None:
        slot = self._slot(index)
        return None if slot is None else slot + self.scalar(index, "I")

    def table(self, index: int) -> "Table | None":
        target = self._target(index)
        return None if target is None else Table(self.buffer, target)

    def string(self, index: int) -> bytes | None:
        target = self._target(index)
        if target is None:
            return None
        if target + 4 > len(self.buffer):
            raise SespError("string outside the buffer")
        size = struct.unpack_from("<I", self.buffer, target)[0]
        if target + 4 + size > len(self.buffer):
            raise SespError("string runs past the buffer")
        return self.buffer[target + 4:target + 4 + size]


@dataclass(frozen=True)
class Message:
    sequence: int
    kind: int
    payload: Table | None


def decode(buffer: bytes) -> Message:
    if len(buffer) < 4:
        raise SespError("buffer too short for a root offset")
    root = Table(buffer, struct.unpack_from("<I", buffer, 0)[0])
    return Message(root.scalar(0, "I"), root.scalar(1, "B"), root.table(2))


@dataclass(frozen=True)
class InitializeRequest:
    device_url: str
    subscription_id: int


def initialize_request(message: Message) -> InitializeRequest:
    """Type 2: the client's device URL (field 0) and its subscription channel id (field 1)."""
    if message.kind != REQUEST_INITIALIZE or message.payload is None:
        raise SespError("not an initialize request")
    url = message.payload.string(0) or b""
    return InitializeRequest(url.decode("utf-8", "replace"), message.payload.scalar(1, "I"))


def head_pose(message: Message) -> HeadPose:
    """Type 8 as the client reads it (0x18008b3a0): absent validities default to valid."""
    if message.kind != SUBSCRIPTION_HEADPOSE or message.payload is None:
        raise SespError("not a head-pose message")
    table = message.payload
    timestamp = table.scalar(0, "I") << 32 | table.scalar(1, "I")
    if timestamp >= 1 << 63:
        timestamp -= 1 << 64
    return HeadPose(timestamp,
                    tuple(table.scalar(i, "f", 0.0) for i in range(2, 5)),
                    tuple(table.scalar(i, "f", 0.0) for i in range(5, 8)),
                    table.scalar(8, "i", 1),
                    tuple(table.scalar(i, "i", 1) for i in range(9, 12)))


def capabilities(message: Message) -> tuple[int, set[int]]:
    """Error and set capability flags of an initialize response."""
    if message.kind != RESPONSE_INITIALIZE or message.payload is None:
        raise SespError("not an initialize response")
    table = message.payload
    return table.scalar(0, "I"), {i for i in range(1, CAPABILITY_COUNT + 1) if table.scalar(i, "B")}


# Transport handshake ----------------------------------------------------------------------


@dataclass(frozen=True)
class Hello:
    channel: int
    reply_pipe: str


def decode_hello(data: bytes) -> Hello:
    """First pipe message of a channel: u32 channel, reply pipe name, NUL (0x18008f470, flag 0)."""
    if len(data) < 5 or not data.endswith(b"\0"):
        raise SespError("malformed SESP hello")
    channel = struct.unpack_from("<I", data)[0]
    name = data[4:-1]
    if channel not in CHANNELS or not name or b"\0" in name:
        raise SespError("malformed SESP hello")
    return Hello(channel, name.decode("ascii"))


def encode_hello(channel: int, reply_pipe: str) -> bytes:
    return struct.pack("<I", channel) + reply_pipe.encode("ascii") + b"\0"


def encode_slot(index: int) -> bytes:
    """The service's answer to a hello: its slot index for the connection."""
    return struct.pack("<I", index)
