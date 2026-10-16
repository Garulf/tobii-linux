"""Camera hardware-info response (command 0x848) recovered from the runtime.

See docs/ttp-replies.md. Field names give the byte offset at which the
runtime stores each value; no meaning has been established for them.
docs/head-pose-output.md lists which offsets the head-pose host code consumes.
The implementation is crates/tobii-protocol/src/camera_info.rs.
"""
from dataclasses import dataclass

from ._native import camera_info as _camera
from .ttp import Message
from .ttp_fields import Reader

CAMERA_HEADER = _camera.CAMERA_HEADER
CAMERA_RECORD_SIZE = _camera.CAMERA_RECORD_SIZE
CAMERA_SERIES_LIMIT = _camera.CAMERA_SERIES_LIMIT
MAX_CAMERAS = _camera.MAX_CAMERAS
MAX_POINTS = _camera.MAX_POINTS
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


def _camera_record(values) -> CameraRecord:
    head, triple, doubles, ints, series, tail = values[:3], values[3], values[4], values[5:8], values[8], values[9:]
    return CameraRecord(*head, tuple(triple), tuple(doubles), *ints, tuple(series), *(tuple(t) for t in tail[:2]),
                        tail[2])


def _hardware_info(values) -> HardwareInfo:
    cameras, points, trailer = values
    return HardwareInfo(tuple(_camera_record(c) for c in cameras), tuple(tuple(p) for p in points), trailer)


def read_camera(reader: Reader) -> CameraRecord:
    """Wire order of 0x1403b11b0; the item order is also the record order."""
    return _camera_record(_camera.read_camera(reader))


def read_points(reader: Reader) -> tuple[Triple, ...]:
    """The response parser narrows each fixed-point double to float32."""
    return tuple(_camera.read_points(reader))


def read_hardware_info(payload: bytes) -> HardwareInfo:
    """Response-parser stage only; the tracker's count limits are applied later."""
    return _hardware_info(_camera.read_hardware_info(payload))


def parse_hardware_info(message: Message) -> HardwareInfo:
    return _hardware_info(_camera.parse_hardware_info(message))


def camera_fields(camera: CameraRecord) -> list:
    """Inverse of read_camera, for synthetic vectors; no device encoder was examined."""
    return _camera.camera_fields(camera)


def hardware_info_response(info: HardwareInfo, request_id: int, descriptors=(0, 0)) -> Message:
    return _camera.hardware_info_response(info, request_id, tuple(descriptors))
