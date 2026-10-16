"""Typed TTP payload values and response decoding recovered from the runtime.

Derived from the installed Tobii platform runtime; see docs/ttp-replies.md.
Every payload item is a `ttp.Field`: one wire-type byte, a big-endian length
and big-endian data. Structures are not nested by length. A structure header
item announces how many sibling items follow it. Nothing here writes to hardware.
The implementation is crates/tobii-protocol/src/fields.rs.
"""
from enum import IntEnum

from ._native import fields as _fields
from .ttp import ProtocolError  # noqa: F401  (part of this module's interface)


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
PARSER_KINDS = dict(_fields.PARSER_KINDS)
PARSER_STATUS_NAMES = tuple(_fields.PARSER_STATUS_NAMES)
DEVICE_ERROR_STATUS = dict(_fields.DEVICE_ERROR_STATUS)
UNKNOWN_STATUS = 2
RESPONSE_FIELD_TYPES = dict(_fields.RESPONSE_FIELD_TYPES)

StructHeader = _fields.StructHeader
Reader = _fields.Reader
value = _fields.value
float32 = _fields.float32
fixed16 = _fields.fixed16
fixed42 = _fields.fixed42
fixed16_raw = _fields.fixed16_raw
fixed42_raw = _fields.fixed42_raw
int32_field = _fields.int32_field
uint32_field = _fields.uint32_field
fixed16_field = _fields.fixed16_field
fixed42_field = _fields.fixed42_field
struct_field = _fields.struct_field
array_fields = _fields.array_fields
fixed42_array_field = _fields.fixed42_array_field
triple_fixed42_fields = _fields.triple_fixed42_fields
encode_payload = _fields.encode_payload
response_status = _fields.response_status
status_name = _fields.status_name
reply_values = _fields.reply_values
