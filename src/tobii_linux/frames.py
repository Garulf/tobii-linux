"""Stream data messages (kind 0x53) and the camera image frames they carry.

Layout recovered by static analysis of the runtime's stream decoder
(0x1402ce2a0, column readers 0x1403b1b90/0x1403b1b40) and dispatcher
(0x1402c2700); see docs/usb-streams.md. No device traffic has been decoded
with it yet. Parsing is stricter than the runtime: duplicate or malformed
columns are rejected instead of being overwritten or skipped.
"""
from dataclasses import dataclass
import struct

from .ttp import Field, Message, ProtocolError, fields
from .ttp_commands import (FIELD_BLOB, FIELD_INT64, FIELD_STRUCT, FIELD_UINT32,
                           STREAM_DATA_KIND, wire_stream_id, Stream)

FRAME_STRUCT = 0x0BB8
COLUMN_STRUCT = 0x0BB9
COLUMN_MEMBERS = 2
HEAD_POSE_PIXEL_FORMAT = 8


@dataclass(frozen=True)
class Column:
    column_id: int
    value: tuple[Field, ...]


@dataclass(frozen=True)
class ImageFrame:
    timestamp: int
    pixel_format: int
    width: int
    height: int
    pixels: bytes


@dataclass(frozen=True)
class PrimaryCameraFrame:
    timestamp: int
    word2: int
    width: int
    stride: int
    height: int
    pixels: bytes


def struct_code(field: Field) -> tuple[int, int]:
    if field.tag != FIELD_STRUCT or len(field.data) != 4:
        raise ProtocolError("Expected a struct header field")
    code = struct.unpack(">I", field.data)[0]
    return code >> 16 & 0xFFF, code & 0xFFFF


def take_value(items: list[Field], index: int) -> int:
    """Return the index after one value, descending into nested structs."""
    if index >= len(items):
        raise ProtocolError("Column value missing")
    if items[index].tag != FIELD_STRUCT:
        return index + 1
    members, _ = struct_code(items[index])
    index += 1
    for _ in range(members):
        index = take_value(items, index)
    return index


def stream_columns(payload: bytes) -> list[Column]:
    items = fields(payload)
    if not items:
        raise ProtocolError("Empty stream payload")
    count, kind = struct_code(items[0])
    if kind != FRAME_STRUCT:
        raise ProtocolError("Stream payload does not start with a column set")
    columns = []
    index = 1
    for _ in range(count):
        if index + 1 >= len(items):
            raise ProtocolError("Truncated column set")
        if struct_code(items[index]) != (COLUMN_MEMBERS, COLUMN_STRUCT):
            raise ProtocolError("Expected a column header")
        column_id = uint32_value(items[index + 1])
        end = take_value(items, index + 2)
        columns.append(Column(column_id, tuple(items[index + 2:end])))
        index = end
    if index != len(items):
        raise ProtocolError("Unexpected fields after the column set")
    return columns


def uint32_value(field: Field) -> int:
    if field.tag != FIELD_UINT32 or len(field.data) != 4:
        raise ProtocolError("Expected a uint32 field")
    return struct.unpack(">I", field.data)[0]


def int64_value(field: Field) -> int:
    if field.tag != FIELD_INT64 or len(field.data) != 8:
        raise ProtocolError("Expected an int64 field")
    return struct.unpack(">q", field.data)[0]


def blob_value(field: Field) -> bytes:
    if field.tag != FIELD_BLOB or len(field.data) < 4:
        raise ProtocolError("Expected a blob field")
    size = struct.unpack_from(">I", field.data)[0]
    if size != len(field.data) - 4:
        raise ProtocolError("Blob size does not match field length")
    return field.data[4:]


COLUMN_DECODERS = {FIELD_INT64: int64_value, FIELD_UINT32: uint32_value, FIELD_BLOB: blob_value}


def column_values(message: Message, stream: Stream, layout: dict[int, int]) -> dict[int, object]:
    if message.kind != STREAM_DATA_KIND or message.command != wire_stream_id(stream):
        raise ProtocolError("Not a data message for this stream")
    if message.word4 != 0:
        raise ProtocolError("Stream message carries a nonzero status word")
    values = {}
    for column in stream_columns(message.payload):
        if column.column_id not in layout:
            continue
        if column.column_id in values:
            raise ProtocolError("Duplicate column")
        if len(column.value) != 1 or column.value[0].tag != layout[column.column_id]:
            raise ProtocolError("Unexpected column type")
        values[column.column_id] = COLUMN_DECODERS[layout[column.column_id]](column.value[0])
    if values.keys() != layout.keys():
        raise ProtocolError("Required image column missing")
    return values


IMAGE_LAYOUT = {1: FIELD_INT64, 2: FIELD_UINT32, 3: FIELD_UINT32, 4: FIELD_UINT32, 5: FIELD_BLOB}
PRIMARY_CAMERA_LAYOUT = {1: FIELD_INT64, 2: FIELD_UINT32, 3: FIELD_UINT32,
                         4: FIELD_UINT32, 5: FIELD_UINT32, 6: FIELD_BLOB}


def image_frame(message: Message) -> ImageFrame:
    """Image stream 0x501, with the checks receive_image (0x140266430) applies."""
    values = column_values(message, Stream.IMAGE, IMAGE_LAYOUT)
    frame = ImageFrame(*(values[column] for column in sorted(IMAGE_LAYOUT)))
    if frame.pixel_format != HEAD_POSE_PIXEL_FORMAT:
        raise ProtocolError("receive_image only accepts pixel format 8")
    if len(frame.pixels) != frame.width * frame.height:
        raise ProtocolError("Pixel byte count is not width times height")
    return frame


def primary_camera_frame(message: Message) -> PrimaryCameraFrame:
    """Primary camera stream 0x50e, with the checks of 0x140266d50 and 0x140285be0."""
    values = column_values(message, Stream.PRIMARY_CAMERA_IMAGE, PRIMARY_CAMERA_LAYOUT)
    frame = PrimaryCameraFrame(*(values[column] for column in sorted(PRIMARY_CAMERA_LAYOUT)))
    if len(frame.pixels) != frame.stride * frame.height:
        raise ProtocolError("Pixel byte count is not stride times height")
    if frame.stride != frame.width:
        raise ProtocolError("The head-pose path requires stride equal to width")
    return frame
