"""Offline subset of the bundled game DLL's PRP serialization.

Supports envelopes, bodies and non-compound stream packets. These functions do
NOT implement the service handshake, subscription commands or transport.
Recovered from DLL SHA256 35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020.
See docs/game-interface.md for locations and limits. Implemented in
crates/tobii-service/src/prp.rs; the dataclasses here carry its values.
"""

from dataclasses import dataclass

from ._native import prp as _prp

MAGIC = _prp.MAGIC
HEAD_POSE = _prp.HEAD_POSE
GAZE_POINT = _prp.GAZE_POINT
RESPONSIVE_GAZE_POINT = _prp.RESPONSIVE_GAZE_POINT
# An implementation resource limit, not a recovered vendor protocol maximum.
DEFAULT_MAX_PAYLOAD = _prp.DEFAULT_MAX_PAYLOAD

ProtocolError = _prp.ProtocolError


def encode_envelope(payload: bytes, *, max_payload: int = DEFAULT_MAX_PAYLOAD) -> bytes:
    """Wrap an already serialized, opaque PRP operation payload."""
    return _prp.encode_envelope(payload, max_payload)


def decode_envelope(packet: bytes, *, max_payload: int = DEFAULT_MAX_PAYLOAD) -> bytes:
    """Validate one complete envelope; return its opaque payload."""
    return _prp.decode_envelope(packet, max_payload)


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
    return _prp.encode_gaze_body(sample)


def decode_gaze_body(body: bytes) -> GazePoint:
    return GazePoint(*_prp.decode_gaze_body(body))


def encode_head_body(sample: HeadPose) -> bytes:
    """Serialize the 52-byte body used by stream ID 2."""
    return _prp.encode_head_body(sample)


def decode_head_body(body: bytes) -> HeadPose:
    return HeadPose(*_prp.decode_head_body(body))


def _sample_kind(sample) -> str | None:
    if isinstance(sample, GazePoint):
        return "gaze"
    if isinstance(sample, HeadPose):
        return "head"
    return None


def encode_stream_packet(sample: GazePoint | HeadPose, *, message_tag: int,
                         responsive: bool = False) -> bytes:
    """Encode the observed non-compound stream operation with empty extensions.

    message_tag is the opaque u32 preceding operation type 1. Its service-level
    meaning is not established. A caller must supply it rather than assume zero.
    This is a serializer, not an operational service implementation.
    """
    return _prp.encode_stream_packet(_sample_kind(sample), sample, message_tag, responsive)
