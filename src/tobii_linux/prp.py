"""Offline subset of the bundled game DLL's PRP serialization.

Supports envelopes, bodies and non-compound stream packets. These functions do
NOT implement the service handshake, subscription commands or transport.
Recovered from DLL SHA256 35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020.
See docs/game-interface.md for locations and limits.
"""

from dataclasses import dataclass
import struct

MAGIC = 0x50525054
HEAD_POSE = 2
GAZE_POINT = 3
RESPONSIVE_GAZE_POINT = 29
# An implementation resource limit, not a recovered vendor protocol maximum.
DEFAULT_MAX_PAYLOAD = 16 * 1024 * 1024


class ProtocolError(ValueError):
    """Malformed or unsupported PRP data."""


def encode_envelope(payload: bytes, *, max_payload: int = DEFAULT_MAX_PAYLOAD) -> bytes:
    """Wrap an already serialized, opaque PRP operation payload."""
    size = len(payload)
    if size > min(max_payload, 0xFFFFFFFF):
        raise ProtocolError("PRP payload exceeds the configured limit")
    return struct.pack("<III", MAGIC, size, size ^ MAGIC) + payload


def decode_envelope(packet: bytes, *, max_payload: int = DEFAULT_MAX_PAYLOAD) -> bytes:
    """Validate one complete envelope; return its opaque payload."""
    if len(packet) < 12:
        raise ProtocolError("truncated PRP envelope")
    magic, size, check = struct.unpack_from("<III", packet)
    if magic != MAGIC or check != size ^ MAGIC:
        raise ProtocolError("invalid PRP header")
    if size > max_payload:
        raise ProtocolError("PRP payload exceeds the configured limit")
    if len(packet) != 12 + size:
        raise ProtocolError("PRP envelope length mismatch")
    return packet[12:]


def _timestamp_words(timestamp: int) -> tuple[int, int]:
    if not -(1 << 63) <= timestamp < (1 << 63):
        raise ProtocolError("timestamp is outside signed 64-bit range")
    value = timestamp & 0xFFFFFFFFFFFFFFFF
    return value >> 32, value & 0xFFFFFFFF


def _timestamp(high: int, low: int) -> int:
    value = high << 32 | low
    return value - (1 << 64) if value >> 63 else value


def _validity(value: int) -> bool:
    if value not in (0, 1):
        raise ProtocolError("noncanonical PRP validity field")
    return bool(value)


@dataclass(frozen=True)
class GazePoint:
    timestamp: int
    valid: bool
    x: float
    y: float


@dataclass(frozen=True)
class HeadPose:
    timestamp: int
    position_valid: bool
    position: tuple[float, float, float]
    rotation_valid: tuple[bool, bool, bool]
    rotation: tuple[float, float, float]


def encode_gaze_body(sample: GazePoint) -> bytes:
    """Serialize the 24-byte body used by stream IDs 3 and 29."""
    return struct.pack("<IIIIff", 24, *_timestamp_words(sample.timestamp),
                       int(bool(sample.valid)), sample.x, sample.y)


def decode_gaze_body(body: bytes) -> GazePoint:
    if len(body) != 24 or struct.unpack_from("<I", body)[0] != 24:
        raise ProtocolError("invalid gaze body length")
    _, high, low, valid, x, y = struct.unpack("<IIIIff", body)
    return GazePoint(_timestamp(high, low), _validity(valid), x, y)


def encode_head_body(sample: HeadPose) -> bytes:
    """Serialize the 52-byte body used by stream ID 2."""
    if any(len(v) != 3 for v in (sample.position, sample.rotation_valid, sample.rotation)):
        raise ProtocolError("head pose requires three components per vector")
    return struct.pack("<IIII3f3I3f", 52, *_timestamp_words(sample.timestamp),
                       int(bool(sample.position_valid)), *sample.position,
                       *(int(bool(v)) for v in sample.rotation_valid), *sample.rotation)


def decode_head_body(body: bytes) -> HeadPose:
    if len(body) != 52 or struct.unpack_from("<I", body)[0] != 52:
        raise ProtocolError("invalid head body length")
    values = struct.unpack("<IIII3f3I3f", body)
    return HeadPose(_timestamp(values[1], values[2]), _validity(values[3]),
                    values[4:7], tuple(_validity(v) for v in values[7:10]), values[10:13])


def encode_stream_packet(sample: GazePoint | HeadPose, *, message_tag: int,
                         responsive: bool = False) -> bytes:
    """Encode the observed non-compound stream operation with empty extensions.

    message_tag is the opaque u32 preceding operation type 1. Its service-level
    meaning is not established. A caller must supply it rather than assume zero.
    This is a serializer, not an operational service implementation.
    """
    if not 0 <= message_tag <= 0xFFFFFFFF:
        raise ProtocolError("message tag is outside unsigned 32-bit range")
    if isinstance(sample, GazePoint):
        stream = RESPONSIVE_GAZE_POINT if responsive else GAZE_POINT
        body = encode_gaze_body(sample)
    elif isinstance(sample, HeadPose) and not responsive:
        stream, body = HEAD_POSE, encode_head_body(sample)
    else:
        raise ProtocolError("unsupported stream sample")
    payload = struct.pack("<7I", 12, message_tag, 1, 4, 8, stream, 4)
    return encode_envelope(payload + body + struct.pack("<II", 4, 0))
