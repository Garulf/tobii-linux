"""TTP request encoders for stream startup, recovered from the ET5 runtime.

Builders and command numbers come from static analysis of the installed
platform runtime; tools/ttp_commands_oracle.py compares every encoder with the
original builder in emulation. See docs/usb-streams.md. The implementation is
crates/tobii-protocol/src/commands.rs.
"""
from enum import IntEnum

from ._native import commands as _commands
from .ttp import ProtocolError, uint32_array  # noqa: F401  (part of this module's interface)

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


HEADER_ONLY_COMMANDS = frozenset(Command(command) for command in _commands.HEADER_ONLY_COMMANDS)


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


WIRE_STREAM_IDS = dict(_commands.WIRE_STREAM_IDS)

wire_stream_id = _commands.wire_stream_id
int32_field = _commands.int32_field
uint32_field = _commands.uint32_field
fixed16_field = _commands.fixed16_field
string_field = _commands.string_field
blob_field = _commands.blob_field
struct_header = _commands.struct_header
request = _commands.request
subscribe_stream = _commands.subscribe_stream
unsubscribe_stream = _commands.unsubscribe_stream
enumerate_stream_type_columns = _commands.enumerate_stream_type_columns
open_realm_salt = _commands.open_realm_salt
realm_digest = _commands.realm_digest
open_realm_response = _commands.open_realm_response
close_realm = _commands.close_realm
pause_device = _commands.pause_device
resume_device = _commands.resume_device
set_illumination_mode = _commands.set_illumination_mode
set_output_frequency = _commands.set_output_frequency
set_frequencies = _commands.set_frequencies
header_only_request = _commands.header_only_request
