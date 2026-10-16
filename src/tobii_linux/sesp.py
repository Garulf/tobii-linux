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
service builders byte for byte (tools/sesp_oracle.py). Implemented in
crates/tobii-service/src/sesp.rs; tables are its native Table class.
"""
from dataclasses import dataclass
import struct

from ._native import sesp as _sesp

PIPE_NAME = "streamengineservices"
URL = "tobii-sesp://" + PIPE_NAME
COMMAND_CHANNEL = 0x2711
SUBSCRIPTION_CHANNEL = 0x2712
CHANNELS = range(0x2711, 0x2715)

MAGIC = 0x70736573
HEADER = struct.Struct("<III")
MAX_MESSAGE = _sesp.MAX_MESSAGE

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


SespError = _sesp.SespError


# Framing ----------------------------------------------------------------------------------


def frame(buffer: bytes) -> bytes:
    return _sesp.frame(buffer)


FrameDecoder = _sesp.FrameDecoder


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


FlatccBuilder = _sesp.FlatccBuilder


def message(sequence: int, kind: int, payload_fields, strings=()) -> bytes:
    """One framed message: payload table, then the root table {sequence, type, payload}.

    `strings` are (field index, bytes) emitted before the payload table, which then
    refers to them.
    """
    return _sesp.message(sequence, kind, list(payload_fields), list(strings))


# Service messages (ses_windows.dll builders) ---------------------------------------------


def wire_error(error: int) -> int:
    return _sesp.wire_error(error)


def encode_initialize_response(sequence: int, error: int, capabilities=()) -> bytes:
    """Type 3: error, then capability flags 1..15 (`capabilities` lists the set ones)."""
    return _sesp.encode_initialize_response(sequence, error, capabilities)


def encode_error_response(sequence: int, kind: int, error: int) -> bytes:
    """Responses that carry only an error: head-pose start (5) and stop (7), send statistics
    (0x33) and the invalid-operation signal (1)."""
    return _sesp.encode_error_response(sequence, kind, error)


def encode_stream_status_response(sequence: int) -> bytes:
    """Type 0x1b, an empty table."""
    return _sesp.encode_stream_status_response(sequence)


@dataclass(frozen=True)
class HeadPose:
    """tobii_head_pose_t as the service forwards it; validity 1 means valid."""
    timestamp_us: int
    position: tuple
    rotation: tuple
    position_validity: int = VALID
    rotation_validity: tuple = (VALID, VALID, VALID)


def encode_head_pose(sequence: int, pose: HeadPose) -> bytes:
    """Type 8: timestamp high and low words, position, rotation, then the validities."""
    return _sesp.encode_head_pose(sequence, pose)


# Reading ----------------------------------------------------------------------------------


Table = _sesp.Table


@dataclass(frozen=True)
class Message:
    sequence: int
    kind: int
    payload: Table | None


def decode(buffer: bytes) -> Message:
    return Message(*_sesp.decode(buffer))


@dataclass(frozen=True)
class InitializeRequest:
    device_url: str
    subscription_id: int


def initialize_request(message: Message) -> InitializeRequest:
    """Type 2: the client's device URL (field 0) and its subscription channel id (field 1)."""
    return InitializeRequest(*_sesp.initialize_request(message.kind, message.payload))


def head_pose(message: Message) -> HeadPose:
    """Type 8 as the client reads it (0x18008b3a0): absent validities default to valid."""
    return HeadPose(*_sesp.head_pose(message.kind, message.payload))


def capabilities(message: Message) -> tuple[int, set[int]]:
    """Error and set capability flags of an initialize response."""
    return _sesp.capabilities(message.kind, message.payload)


# Transport handshake ----------------------------------------------------------------------


@dataclass(frozen=True)
class Hello:
    channel: int
    reply_pipe: str


def decode_hello(data: bytes) -> Hello:
    """First pipe message of a channel: u32 channel, reply pipe name, NUL (0x18008f470, flag 0)."""
    return Hello(*_sesp.decode_hello(data))


def encode_hello(channel: int, reply_pipe: str) -> bytes:
    return _sesp.encode_hello(channel, reply_pipe)


def encode_slot(index: int) -> bytes:
    """The service's answer to a hello: its slot index for the connection."""
    return _sesp.encode_slot(index)
