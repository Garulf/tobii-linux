"""TTP request encoders for stream startup, recovered from the ET5 runtime.

Builders and command numbers come from static analysis of the installed
platform runtime; tools/ttp_commands_oracle.py compares every encoder with the
original builder in emulation. See docs/usb-streams.md. Nothing here has been
sent to a device, and device acceptance of any request is unverified.
"""
from enum import IntEnum
import hmac
import math
import struct

from .ttp import Field, Message, ProtocolError, uint32, uint32_array

REQUEST_KIND = 0x51
RESPONSE_KIND = 0x52
STREAM_DATA_KIND = 0x53
NOTIFICATION_KIND = 0x4E

FIELD_INT32 = 0x01
FIELD_UINT32 = 0x02
FIELD_FIXED16 = 0x03
FIELD_FIXED42 = 0x04
FIELD_STRUCT = 0x05
FIELD_INT64 = 0x06
FIELD_STRING = 0x14
FIELD_BLOB = 0x15
FIELD_UINT32_ARRAY = 0x17
FIELD_FIXED42_ARRAY = 0x19

FREQUENCY_PAIR_STRUCT = 0x2AF8
REALM_DIGEST_SIZE = 16


class Command(IntEnum):
    PROTOCOL_UPGRADE = 1000
    ENUMERATE_STREAM_TYPES = 1200
    ENUMERATE_STREAM_TYPE_COLUMNS = 1210
    SUBSCRIBE_STREAM = 1220
    UNSUBSCRIBE_STREAM = 1230
    GET_DEVICE_PROPERTIES = 1330
    GET_CAPABILITIES = 1350
    GET_CAPABILITIES_ALL = 1351
    GET_TRACK_BOX = 1400
    GET_DEVICE_INFO = 1420
    GET_DISPLAY_AREA = 1430
    GET_DEVICE_STATUS = 1490
    UNLOCK_PPU = 1600
    GET_OUTPUT_FREQUENCY = 1610
    SET_OUTPUT_FREQUENCY = 1620
    ENUMERATE_OUTPUT_FREQUENCIES = 1630
    GET_FREQUENCIES = 1650
    SET_FREQUENCIES = 1660
    ENUMERATE_FREQUENCIES = 1670
    GET_DEVICE_NAME = 1700
    ENUMERATE_EXTENSIONS = 1810
    ENUMERATE_ENABLED_EXTENSIONS = 1820
    OPEN_REALM_SALT = 1900
    OPEN_REALM_RESPONSE = 1910
    OPEN_REALM_RESPONSE_WITH_DATA = 1911
    CLOSE_REALM = 1915
    GET_LOW_BLINK_MODE = 1920
    GET_ILLUMINATION_MODE = 2010
    SET_ILLUMINATION_MODE = 2020
    ENUMERATE_ILLUMINATION_MODES = 2030
    GET_GEOMETRY_MOUNTING = 2110
    HARDWARE_CONFIGURATION_GET = 2120
    POWER_SAVE_ACTIVATE = 3000
    POWER_SAVE_DEACTIVATE = 3010
    PAUSE_DEVICE = 3100
    GET_ENABLED_EYE = 3170


HEADER_ONLY_COMMANDS = frozenset({
    Command.ENUMERATE_STREAM_TYPES, Command.GET_DEVICE_PROPERTIES,
    Command.GET_CAPABILITIES, Command.GET_CAPABILITIES_ALL, Command.GET_TRACK_BOX,
    Command.GET_DEVICE_INFO, Command.GET_DISPLAY_AREA, Command.GET_DEVICE_STATUS,
    Command.UNLOCK_PPU,
    Command.GET_OUTPUT_FREQUENCY, Command.ENUMERATE_OUTPUT_FREQUENCIES,
    Command.GET_FREQUENCIES, Command.ENUMERATE_FREQUENCIES, Command.GET_DEVICE_NAME,
    Command.ENUMERATE_EXTENSIONS, Command.ENUMERATE_ENABLED_EXTENSIONS,
    Command.GET_LOW_BLINK_MODE, Command.GET_ILLUMINATION_MODE,
    Command.ENUMERATE_ILLUMINATION_MODES, Command.GET_GEOMETRY_MOUNTING,
    Command.HARDWARE_CONFIGURATION_GET, Command.POWER_SAVE_ACTIVATE,
    Command.POWER_SAVE_DEACTIVATE, Command.GET_ENABLED_EYE,
})


class Stream(IntEnum):
    """Runtime-internal stream index used by the tracker subscribe slots."""
    GAZE = 1
    IMAGE = 2
    PRESENCE = 5
    ALGO_DEBUG = 8
    DEVICE_LOG = 9
    IMAGE_COLLECTION = 13
    WEARABLE_FOVEATED = 14
    IMAGE_COLLECTION_B = 15
    INTERMEDIATE_ALGO = 16
    PRIMARY_CAMERA_IMAGE = 17
    IMAGE_COLLECTION_C = 18
    CUSTOM_DATA = 20
    SYNC = 21


WIRE_STREAM_IDS = {
    1: 0x500, 2: 0x501, 3: 0x502, 4: 0x503, 5: 0x504, 6: 0x505, 7: 0x50A,
    8: 0x1770, 9: 0x1772, 10: 0x506, 11: 0x50D, 12: 0x507, 13: 0x508,
    14: 0x509, 15: 0x50B, 16: 0x50C, 17: 0x50E, 18: 0x50F, 19: 0x511,
    20: 0x1774, 21: 0x1771,
}


def wire_stream_id(stream: int) -> int:
    """Map a stream index to the id the runtime writes (table at 0x1402c4500)."""
    try:
        return WIRE_STREAM_IDS[int(stream)]
    except KeyError:
        raise ProtocolError("Unknown stream index") from None


def int32_field(value: int) -> Field:
    if type(value) is not int or not -0x80000000 <= value <= 0x7FFFFFFF:
        raise ProtocolError("Expected a signed 32-bit integer")
    return Field(FIELD_INT32, struct.pack(">i", value))


def uint32_field(value: int) -> Field:
    return Field(FIELD_UINT32, struct.pack(">I", uint32(value)))


def fixed16_field(value: float) -> Field:
    """Float32 times 65536, truncated as the runtime's cvttss2si (0x1403b0160)."""
    scaled = struct.unpack("<f", struct.pack("<f", value))[0] * 65536.0
    if math.isfinite(scaled) and -2**31 <= math.trunc(scaled) < 2**31:
        raw = math.trunc(scaled) & 0xFFFFFFFF
    else:
        raw = 0x80000000
    return Field(FIELD_FIXED16, struct.pack(">I", raw))


def string_field(text: str) -> Field:
    data = text.encode()
    if b"\0" in data:
        raise ProtocolError("String fields are NUL-terminated in the runtime")
    return Field(FIELD_STRING, struct.pack(">I", uint32(len(data))) + data)


def blob_field(data: bytes) -> Field:
    if not isinstance(data, bytes):
        raise TypeError("Blob data must be bytes")
    return Field(FIELD_BLOB, struct.pack(">I", uint32(len(data))) + data)


def struct_header(member_count: int, struct_type: int) -> Field:
    """Tag 5 header; the members follow as sibling fields."""
    if not 0 <= member_count <= 0xFFF or not 0 <= struct_type <= 0xFFFF:
        raise ProtocolError("Struct member count or type out of range")
    return Field(FIELD_STRUCT, struct.pack(">I", member_count << 16 | struct_type))


def request(request_id: int, command: int, fields=()) -> Message:
    """Request header 0x51; non-empty field data gets the two zero prefix bytes."""
    encoded = b"".join(field.encode() for field in fields)
    payload = b"\0\0" + encoded if encoded else b""
    return Message(REQUEST_KIND, uint32(request_id), 0, uint32(int(command)), 0, payload)


def header_only_request(request_id: int, command: Command) -> Message:
    if command not in HEADER_ONLY_COMMANDS:
        raise ProtocolError("Command is not built by a header-only wrapper")
    return request(request_id, command)


def subscribe_stream(request_id: int, stream: int, columns=()) -> Message:
    """Built by 0x1402c71b0; the subscribe slots pass no columns."""
    return request(request_id, Command.SUBSCRIBE_STREAM,
                   (uint32_field(wire_stream_id(stream)), uint32_array(columns)))


def unsubscribe_stream(request_id: int, stream: int) -> Message:
    return request(request_id, Command.UNSUBSCRIBE_STREAM,
                   (uint32_field(wire_stream_id(stream)),))


def enumerate_stream_type_columns(request_id: int, stream: int) -> Message:
    return request(request_id, Command.ENUMERATE_STREAM_TYPE_COLUMNS,
                   (uint32_field(wire_stream_id(stream)),))


def open_realm_salt(request_id: int, realm: int) -> Message:
    """First realm step (0x1402c4600); open_realm_salt passes the array [0]."""
    return request(request_id, Command.OPEN_REALM_SALT,
                   (uint32_field(realm), uint32_array((0,))))


def realm_digest(key: bytes, salt: bytes) -> bytes:
    """HMAC-MD5 of the device salt, as computed by 0x1402c0ef0."""
    return hmac.new(key, salt, "md5").digest()


def open_realm_response(request_id: int, challenge: tuple[int, int],
                        digest: bytes, extra: bytes = b"") -> Message:
    """Second realm step: 1910 without extra data, 1911 with it.

    challenge holds the two integers from the salt reply in reply order.
    """
    if len(digest) != REALM_DIGEST_SIZE:
        raise ProtocolError("Realm digest must be 16 bytes")
    fields = [uint32_field(challenge[0]), uint32_field(challenge[1]), blob_field(digest)]
    command = Command.OPEN_REALM_RESPONSE
    if extra:
        fields.append(blob_field(extra))
        command = Command.OPEN_REALM_RESPONSE_WITH_DATA
    return request(request_id, command, fields)


def close_realm(request_id: int, realm: int) -> Message:
    return request(request_id, Command.CLOSE_REALM, (uint32_field(realm),))


def pause_device(request_id: int) -> Message:
    return request(request_id, Command.PAUSE_DEVICE, (uint32_field(1),))


def resume_device(request_id: int) -> Message:
    return request(request_id, Command.PAUSE_DEVICE, (uint32_field(0),))


def set_illumination_mode(request_id: int, mode: str) -> Message:
    return request(request_id, Command.SET_ILLUMINATION_MODE, (string_field(mode),))


def set_output_frequency(request_id: int, frequency: float) -> Message:
    """Command 1620 from slot 0x248, which has no log name; the name is inferred."""
    return request(request_id, Command.SET_OUTPUT_FREQUENCY, (fixed16_field(frequency),))


def set_frequencies(request_id: int, first: int, second: int) -> Message:
    """Struct 0x2af8 with two uint32 members in wire order (0x1402c5ac0)."""
    return request(request_id, Command.SET_FREQUENCIES,
                   (struct_header(2, FREQUENCY_PAIR_STRUCT),
                    uint32_field(first), uint32_field(second)))
