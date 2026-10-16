"""Stream data messages (kind 0x53) and the camera image frames they carry.

Layout recovered by static analysis of the runtime's stream decoder
(0x1402ce2a0, column readers 0x1403b1b90/0x1403b1b40) and dispatcher
(0x1402c2700); see docs/usb-streams.md. Parsing is stricter than the runtime:
duplicate or malformed columns are rejected instead of being overwritten or
skipped. The implementation is crates/tobii-protocol/src/frames.rs.
"""
from dataclasses import dataclass

from ._native import frames as _frames
from .ttp import Field, Message
from .ttp_commands import FIELD_BLOB, FIELD_INT64, FIELD_UINT32, Stream

FRAME_STRUCT = _frames.FRAME_STRUCT
COLUMN_STRUCT = _frames.COLUMN_STRUCT
COLUMN_MEMBERS = _frames.COLUMN_MEMBERS
HEAD_POSE_PIXEL_FORMAT = _frames.HEAD_POSE_PIXEL_FORMAT


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


struct_code = _frames.struct_code
take_value = _frames.take_value
uint32_value = _frames.uint32_value
int64_value = _frames.int64_value
blob_value = _frames.blob_value


def stream_columns(payload: bytes) -> list[Column]:
    return [Column(column_id, value) for column_id, value in _frames.stream_columns(payload)]


def column_values(message: Message, stream: Stream, layout: dict[int, int]) -> dict[int, object]:
    return _frames.column_values(message, int(stream), layout)


IMAGE_LAYOUT = {1: FIELD_INT64, 2: FIELD_UINT32, 3: FIELD_UINT32, 4: FIELD_UINT32, 5: FIELD_BLOB}
PRIMARY_CAMERA_LAYOUT = {1: FIELD_INT64, 2: FIELD_UINT32, 3: FIELD_UINT32,
                         4: FIELD_UINT32, 5: FIELD_UINT32, 6: FIELD_BLOB}


def image_frame(message: Message) -> ImageFrame:
    """Image stream 0x501, with the checks receive_image (0x140266430) applies."""
    return ImageFrame(*_frames.image_frame(message))


def primary_camera_frame(message: Message) -> PrimaryCameraFrame:
    """Primary camera stream 0x50e, with the checks of 0x140266d50 and 0x140285be0."""
    return PrimaryCameraFrame(*_frames.primary_camera_frame(message))
