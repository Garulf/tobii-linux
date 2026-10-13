"""Camera hardware-info response (command 0x848) recovered from the runtime.

See docs/ttp-replies.md. Field names give the byte offset at which the
runtime stores each value; no meaning has been established for them.
docs/head-pose-output.md lists which offsets the head-pose host code consumes.
"""
from dataclasses import dataclass

from .ttp import Message, ProtocolError
from .ttp import HARDWARE_INFO_COMMAND
from .ttp_fields import (KIND_RESPONSE, Reader, array_fields, encode_payload,
                         fixed16_field, fixed42_array_field, fixed42_field, float32,
                         int32_field, response_status, status_name, struct_field,
                         triple_fixed42_fields)

CAMERA_HEADER = 0x1A332C
CAMERA_RECORD_SIZE = 0x2E8
CAMERA_SERIES_LIMIT = 64
MAX_CAMERAS = 4
MAX_POINTS = 40
RESPONSE_FIELD_TYPES = (0x1B, 0x0C, 0x02)

Triple = tuple[float, float, float]


@dataclass(frozen=True)
class CameraRecord:
    int32_0x00: int
    float32_0x04: float
    float32_0x08: float
    triple_0x10: Triple
    doubles_0x28: tuple[float, ...]
    int32_0xa0: int
    int32_0xa4: int
    int32_0xa8: int
    series_0xb0: tuple[float, ...]
    triple_0x2b0: Triple
    triple_0x2c8: Triple
    double_0x2e0: float

    @property
    def series_count_0xac(self) -> int:
        return len(self.series_0xb0)


@dataclass(frozen=True)
class HardwareInfo:
    cameras_0x8: tuple[CameraRecord, ...]
    points_0xbb0: tuple[Triple, ...]
    int32_0xf70: int


def read_camera(reader: Reader) -> CameraRecord:
    """Wire order of 0x1403b11b0; the item order is also the record order."""
    reader.struct(CAMERA_HEADER)
    head = reader.int32(), reader.fixed16(), reader.fixed16()
    triple = reader.triple_fixed42()
    doubles = tuple(reader.fixed42() for _ in range(15))
    ints = reader.int32(), reader.int32(), reader.int32()
    series = reader.fixed42_array(CAMERA_SERIES_LIMIT)
    return CameraRecord(*head, triple, doubles, *ints, series,
                        reader.triple_fixed42(), reader.triple_fixed42(), reader.fixed42())


def read_points(reader: Reader) -> tuple[Triple, ...]:
    """The response parser narrows each fixed-point double to float32."""
    return tuple(tuple(float32(n) for n in reader.triple_fixed42())
                 for _ in range(reader.array()))


def read_hardware_info(payload: bytes) -> HardwareInfo:
    """Response-parser stage only; the tracker's count limits are applied later."""
    reader = Reader(payload)
    cameras = tuple(read_camera(reader) for _ in range(reader.array()))
    points = read_points(reader)
    return HardwareInfo(cameras, points, reader.int32())


def parse_hardware_info(message: Message) -> HardwareInfo:
    if message.kind != KIND_RESPONSE or message.command != HARDWARE_INFO_COMMAND:
        raise ProtocolError("Not a hardware-info response")
    status = response_status(message)
    if status:
        raise ProtocolError(f"Hardware-info response status {status_name(status)}")
    info = read_hardware_info(message.payload)
    if len(info.cameras_0x8) > MAX_CAMERAS or len(info.points_0xbb0) > MAX_POINTS:
        raise ProtocolError("Hardware-info response exceeds the tracker's record limits")
    return info


def camera_fields(camera: CameraRecord) -> list:
    """Inverse of read_camera, for synthetic vectors; no device encoder was examined."""
    items = [struct_field(CAMERA_HEADER >> 16, CAMERA_HEADER & 0xFFFF),
             int32_field(camera.int32_0x00), fixed16_field(camera.float32_0x04),
             fixed16_field(camera.float32_0x08), *triple_fixed42_fields(camera.triple_0x10)]
    items += [fixed42_field(n) for n in camera.doubles_0x28]
    items += [int32_field(n) for n in (camera.int32_0xa0, camera.int32_0xa4, camera.int32_0xa8)]
    items.append(fixed42_array_field(camera.series_0xb0))
    items += triple_fixed42_fields(camera.triple_0x2b0) + triple_fixed42_fields(camera.triple_0x2c8)
    items.append(fixed42_field(camera.double_0x2e0))
    return items


def hardware_info_response(info: HardwareInfo, request_id: int, descriptors=(0, 0)) -> Message:
    items = array_fields(len(info.cameras_0x8), descriptors[0])
    for camera in info.cameras_0x8:
        items += camera_fields(camera)
    items += array_fields(len(info.points_0xbb0), descriptors[1])
    for point in info.points_0xbb0:
        items += triple_fixed42_fields(point)
    items.append(int32_field(info.int32_0xf70))
    return Message(KIND_RESPONSE, request_id, 0, HARDWARE_INFO_COMMAND, 0,
                   encode_payload(items))
