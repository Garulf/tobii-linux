"""Typed TTP payload values and response decoding recovered from the runtime.

Derived from the installed Tobii platform runtime; see docs/ttp-replies.md.
Every payload item is a `ttp.Field`: one wire-type byte, a big-endian length
and big-endian data. Structures are not nested by length. A structure header
item announces how many sibling items follow it. Nothing here writes to hardware.
"""
from dataclasses import dataclass
from enum import IntEnum
import struct

from .ttp import Field, Message, ProtocolError, uint32


class Wire(IntEnum):
    INT32 = 0x01
    UINT32 = 0x02
    FIXED16 = 0x03
    FIXED42 = 0x04
    STRUCT = 0x05
    INT64 = 0x06
    UINT64 = 0x07
    STRING = 0x14
    BLOB = 0x15
    UINT32_ARRAY = 0x17
    FIXED16_ARRAY = 0x18
    FIXED42_ARRAY = 0x19
    STRING_ARRAY = 0x1A


FIXED16_ONE = 2.0 ** 16
FIXED42_ONE = 2.0 ** 42
FIXED16_SCALE = 1 / FIXED16_ONE
FIXED42_SCALE = 1 / FIXED42_ONE
ARRAY_STRUCT_ID = 0x100
ARRAY_COUNT_MASK = 0xFFF

PAIR_FIXED42 = 0x21F40
TRIPLE_FIXED42 = 0x31F41
TRIPLE_FIXED16 = 0x31F42
PAIR_FIXED16 = 0x21F43

SCALAR_LENGTHS = {Wire.INT32: 4, Wire.UINT32: 4, Wire.FIXED16: 4, Wire.STRUCT: 4,
                  Wire.FIXED42: 8, Wire.INT64: 8, Wire.UINT64: 8}

KIND_REQUEST = 0x51
KIND_RESPONSE = 0x52
PARSER_KINDS = {0x51: 1, 0x52: 2, 0x53: 3, 0x4C: 4, 0x43: 5, 0x4E: 6}

PARSER_STATUS_NAMES = (
    "NO_ERROR", "NEED_MORE_DATA", "UNKNOWN", "BUFFER_TOO_SMALL", "MESSAGE_TOO_BIG",
    "PROTOCOL", "INVALID_PARAMETER", "PROTOCOL_LEGACY_BAD_FORMAT",
    "PROTOCOL_LEGACY_MISSING_UPGRADE", "PROTOCOL_UNKNOWN_OPERATION",
    "PROTOCOL_UNSUPPORTED_OPERATION", "PROTOCOL_OPERATION_FAILED",
    "PROTOCOL_INVALID_PAYLOAD", "PROTOCOL_UNKNOWN_ID", "PROTOCOL_UNAUTHORIZED",
    "PROTOCOL_EXTENSION_REQUIRED", "PROTOCOL_INTERNAL_ERROR", "PROTOCOL_STATE_ERROR",
    "PROTOCOL_INVALID_PARAMETER", "PROTOCOL_OPERATION_ABORTED", "PROTOCOL_NOT_ENABLED",
    "PROTOCOL_TOO_MANY_SUBSCRIBERS")

DEVICE_ERROR_STATUS = {
    0x20000402: 7, 0x20000407: 11, 0x2000040A: 8, 0x20000500: 9, 0x20000501: 10,
    0x20000502: 11, 0x20000503: 12, 0x20000504: 13, 0x20000505: 14, 0x20000506: 15,
    0x20000507: 16, 0x20000508: 17, 0x20000509: 18, 0x20000510: 19, 0x20000511: 11,
    0x20000512: 11, 0x20000513: 11, 0x20000514: 11, 0x20000515: 20, 0x20000516: 21}
UNKNOWN_STATUS = 2

RESPONSE_FIELD_TYPES = {
    0x3E8: (0x01,), 0x408: (0x01,), 0x42F: (0x01,), 0x442: (0x0B,), 0x44C: (0x0D,),
    0x460: (0x1C,), 0x4B0: (0x0E,), 0x4BA: (0x0F,), 0x51E: (0x12,), 0x532: (0x15,),
    0x546: (0x16,), 0x547: (0x16,), 0x578: (0x0A,) * 8, 0x58C: (0x07,) * 4,
    0x596: (0x0A,) * 3, 0x5AA: (0x0A,) * 3, 0x5B4: (0x01,), 0x5C8: (0x0A, 0x0A),
    0x5CD: (0x01,), 0x5D2: (0x15,), 0x5D3: (0x01,), 0x5E6: (0x0D,),
    0x640: (0x01, 0x01, 0x01), 0x64A: (0x05,), 0x65E: (0x06,), 0x668: (0x05,),
    0x672: (0x17,), 0x686: (0x18,), 0x690: (0x17,), 0x6A4: (0x07,), 0x712: (0x14,),
    0x71C: (0x14,), 0x76C: (0x01, 0x01, 0x0D), 0x780: (0x01,), 0x7DA: (0x07,),
    0x7EE: (0x08,), 0x83E: (0x01, 0x05, 0x05, 0x0A, 0x0A), 0x848: (0x1B, 0x0C, 0x02),
    0xBF4: (0x08,), 0xC08: (0x07, 0x0D), 0xC26: (0x01,), 0xC62: (0x01,), 0xC6C: (0x01,),
    0xC80: (0x07,), 0xC8A: (0x07,), 0xC94: (0x01,),
    0xD16: (0x04, 0x01, 0x01, 0x01, 0x01, 0x1A, 0x01, 0x01, 0x01, 0x01, 0x19, 0x0D),
    0x1388: (0x0D,)}


@dataclass(frozen=True)
class StructHeader:
    item_count: int
    struct_id: int

    @classmethod
    def from_word(cls, word: int) -> "StructHeader":
        return cls(word >> 16, word & 0xFFFF)

    @property
    def word(self) -> int:
        return uint32(self.item_count << 16 | self.struct_id)


def float32(value: float) -> float:
    return struct.unpack(">f", struct.pack(">f", value))[0]


def fixed16(raw: int) -> float:
    return float32(raw) * FIXED16_SCALE


def fixed42(raw: int) -> float:
    return float(raw) * FIXED42_SCALE


def _scalar(field: Field):
    if len(field.data) != SCALAR_LENGTHS[field.tag]:
        raise ProtocolError(f"Wire type {field.tag:#x} needs {SCALAR_LENGTHS[field.tag]} data bytes")
    if field.tag == Wire.INT32:
        return struct.unpack(">i", field.data)[0]
    if field.tag == Wire.UINT32:
        return struct.unpack(">I", field.data)[0]
    if field.tag == Wire.FIXED16:
        return fixed16(struct.unpack(">i", field.data)[0])
    if field.tag == Wire.STRUCT:
        return StructHeader.from_word(struct.unpack(">I", field.data)[0])
    if field.tag == Wire.FIXED42:
        return fixed42(struct.unpack(">q", field.data)[0])
    if field.tag == Wire.INT64:
        return struct.unpack(">q", field.data)[0]
    return struct.unpack(">Q", field.data)[0]


def _counted(field: Field, item_size: int) -> tuple[int, bytes]:
    if len(field.data) < 4:
        raise ProtocolError("Counted field lacks its count")
    count = struct.unpack_from(">I", field.data)[0]
    if len(field.data) != 4 + count * item_size:
        raise ProtocolError("Count does not match field length")
    return count, field.data[4:]


def _strings(field: Field) -> tuple[bytes, ...]:
    if len(field.data) < 4:
        raise ProtocolError("String array lacks its count")
    count = struct.unpack_from(">I", field.data)[0]
    items, offset = [], 4
    for _ in range(count):
        if len(field.data) - offset < 4:
            raise ProtocolError("Truncated string length")
        size = struct.unpack_from(">I", field.data, offset)[0]
        offset += 4
        if size > len(field.data) - offset:
            raise ProtocolError("Truncated string data")
        items.append(field.data[offset:offset + size])
        offset += size
    if offset != len(field.data):
        raise ProtocolError("String array length does not match its items")
    return tuple(items)


def value(field: Field):
    """Decode one item; unknown wire types raise instead of guessing a layout.

    The runtime's readers ignore the outer length of variable-size items and
    trust their inner counts. Here a disagreement is refused rather than resolved.
    """
    if field.tag in SCALAR_LENGTHS:
        return _scalar(field)
    if field.tag in (Wire.STRING, Wire.BLOB):
        return _counted(field, 1)[1]
    if field.tag == Wire.UINT32_ARRAY:
        count, data = _counted(field, 4)
        return struct.unpack(f">{count}I", data)
    if field.tag == Wire.FIXED16_ARRAY:
        count, data = _counted(field, 4)
        return tuple(fixed16(n) for n in struct.unpack(f">{count}i", data))
    if field.tag == Wire.FIXED42_ARRAY:
        count, data = _counted(field, 8)
        return tuple(fixed42(n) for n in struct.unpack(f">{count}q", data))
    if field.tag == Wire.STRING_ARRAY:
        return _strings(field)
    raise ProtocolError(f"Unknown wire type {field.tag:#x}")


def _require_range(raw: int, bits: int) -> int:
    if not -(1 << bits - 1) <= raw < 1 << bits - 1:
        raise ProtocolError("Value does not fit the fixed-point wire type")
    return raw


def fixed16_raw(number: float) -> int:
    return _require_range(int(float32(number) * FIXED16_ONE), 32)


def fixed42_raw(number: float) -> int:
    return _require_range(int(number * FIXED42_ONE), 64)


def int32_field(number: int) -> Field:
    return Field(int(Wire.INT32), struct.pack(">i", number))


def uint32_field(number: int) -> Field:
    return Field(int(Wire.UINT32), struct.pack(">I", uint32(number)))


def fixed16_field(number: float) -> Field:
    return Field(int(Wire.FIXED16), struct.pack(">i", fixed16_raw(number)))


def fixed42_field(number: float) -> Field:
    return Field(int(Wire.FIXED42), struct.pack(">q", fixed42_raw(number)))


def struct_field(item_count: int, struct_id: int) -> Field:
    return Field(int(Wire.STRUCT), struct.pack(">I", StructHeader(item_count, struct_id).word))


def array_fields(count: int, descriptor: int = 0) -> list[Field]:
    """Array header as built by 0x1403b1d70; the descriptor's meaning is unknown."""
    if not 0 <= count < ARRAY_COUNT_MASK:
        raise ProtocolError("Array count does not fit the header")
    return [struct_field(count + 1, ARRAY_STRUCT_ID), uint32_field(descriptor)]


def fixed42_array_field(numbers) -> Field:
    raws = [fixed42_raw(n) for n in numbers]
    return Field(int(Wire.FIXED42_ARRAY), struct.pack(f">I{len(raws)}q", len(raws), *raws))


def triple_fixed42_fields(numbers) -> list[Field]:
    x, y, z = numbers
    return [struct_field(3, TRIPLE_FIXED42 & 0xFFFF)] + [fixed42_field(n) for n in (x, y, z)]


def encode_payload(items) -> bytes:
    return b"\0\0" + b"".join(item.encode() for item in items)


class Reader:
    """Sequential typed reader over a response payload, in schema order.

    Bytes after the last requested item are not inspected, as in the runtime;
    `remaining` exposes how many were left.
    """
    def __init__(self, payload: bytes):
        if len(payload) < 2 or payload[:2] != b"\0\0":
            raise ProtocolError("Unsupported payload prefix")
        self.payload = payload
        self.offset = 2

    @property
    def remaining(self) -> int:
        return len(self.payload) - self.offset

    def item(self, wire: int) -> Field:
        if self.remaining < 5:
            raise ProtocolError("Truncated item header")
        tag = self.payload[self.offset]
        length = struct.unpack_from(">I", self.payload, self.offset + 1)[0]
        if tag != wire:
            raise ProtocolError(f"Expected wire type {wire:#x}, got {tag:#x}")
        if length > self.remaining - 5:
            raise ProtocolError("Truncated item data")
        start = self.offset + 5
        self.offset = start + length
        return Field(tag, self.payload[start:start + length])

    def int32(self) -> int:
        return value(self.item(Wire.INT32))

    def uint32(self) -> int:
        return value(self.item(Wire.UINT32))

    def fixed16(self) -> float:
        return value(self.item(Wire.FIXED16))

    def fixed42(self) -> float:
        return value(self.item(Wire.FIXED42))

    def struct(self, header: int) -> None:
        found = value(self.item(Wire.STRUCT)).word
        if found != header:
            raise ProtocolError(f"Expected structure header {header:#x}, got {found:#x}")

    def array(self) -> int:
        header = value(self.item(Wire.STRUCT))
        count = header.item_count & ARRAY_COUNT_MASK
        if header.struct_id != ARRAY_STRUCT_ID or count == 0:
            raise ProtocolError("Expected an array header")
        self.uint32()
        return count - 1

    def fixed42_array(self, limit: int) -> tuple[float, ...]:
        """Keeps at most `limit` values; the runtime reads and drops the rest."""
        return value(self.item(Wire.FIXED42_ARRAY))[:limit]

    def triple_fixed42(self) -> tuple[float, float, float]:
        self.struct(TRIPLE_FIXED42)
        return self.fixed42(), self.fixed42(), self.fixed42()


def response_status(message: Message) -> int:
    """Parser status for a response; payload fields are only decoded when zero."""
    if message.word4 == 0:
        return 0
    return DEVICE_ERROR_STATUS.get(message.word4, UNKNOWN_STATUS)


def status_name(status: int) -> str:
    return PARSER_STATUS_NAMES[status] if 0 <= status < len(PARSER_STATUS_NAMES) else f"{status:#x}"
