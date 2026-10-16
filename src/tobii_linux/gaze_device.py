"""Device-side gaze configuration of the Eye Tracker 5: calibration, persistent files and start checks.

Recovered from the platform runtime's tracker vtable 0x1410af508 and the
castor platform module that drives it; docs/gaze-device.md has the command
table and evidence. Every encoder here is compared with the original builder
in emulation by tools/gaze_device_oracle.py.

The runtime keeps the user calibration on the device. At start it reads the
device calibration id (status key 7) and compares it with the copy in
calibration.setpm; it applies the local copy (1110) only when they differ.

The encoders, decoders and device procedures are crates/tobii-gaze/src/device.rs;
the dataclasses and enums here carry their results.
"""
from dataclasses import dataclass
from enum import IntEnum
import struct
import time

from ._native import gaze_device as _gd
from .ttp import ProtocolError  # noqa: F401  (raised by this module's functions)


class Eye(IntEnum):
    """Eye selector shared by enabled-eye, per-eye calibration and 1011 (0x1402b5290)."""
    LEFT = 1
    RIGHT = 2
    BOTH = 3


GazeCommand = IntEnum('GazeCommand', _gd.GAZE_COMMANDS, module=__name__)

HEADER_ONLY = frozenset(GazeCommand(c) for c in _gd.HEADER_ONLY)
READ_ONLY = frozenset(GazeCommand(c) for c in _gd.READ_ONLY)

DISPLAY_AREA_TRAILER = _gd.DISPLAY_AREA_TRAILER
STATUS_STRUCT = _gd.STATUS_STRUCT
STATUS_CALIBRATION_ID = _gd.STATUS_CALIBRATION_ID
SETTING_FILE_VERSION = _gd.SETTING_FILE_VERSION
SETTING_FILE_HEADER = struct.Struct('<3I')
CALIBRATION_ID = struct.Struct('<I')


def drive(operation, session):
    """Run a native device operation that only sends requests on `session`.

The operation decides every request; this loop sends them with
session.call, so sessions written in Python (dry runs, fakes) work and no
Rust code waits on Python. A failed call raises from here unchanged.
"""
    step = operation.start(time.monotonic())
    while step[0] == 'call':
        timeout_s = step[1]
        if timeout_s is None:
            reply = session.call(operation.request)
        else:
            reply = session.call(operation.request, timeout_s=timeout_s)
        step = operation.replied(session.reply_values(reply), time.monotonic())
    return step[1]


def header_only(request_id: int, command: GazeCommand):
    return _gd.header_only(request_id, command)


def calibration_start_eye(request_id: int, eye: Eye):
    """Command 1011 from vtable slot 0x1c0; no log name, so the name is inferred."""
    return _gd.calibration_start_eye(request_id, int(Eye(eye)))


def _point_request(request_id, command, coordinates, eye):
    return _gd.point_request(request_id, command, list(coordinates), int(Eye(eye)))


def calibration_collect_2d(request_id: int, x: float, y: float, eye: Eye = Eye.BOTH):
    """1030; the tracker (0x1402b42c0) always passes eye 3."""
    return _point_request(request_id, GazeCommand.CALIBRATION_COLLECT_2D, (x, y), eye)


def calibration_collect_3d(request_id: int, x: float, y: float, z: float, eye: Eye = Eye.BOTH):
    return _point_request(request_id, GazeCommand.CALIBRATION_COLLECT_3D, (x, y, z), eye)


def calibration_collect_per_eye_2d(request_id: int, x: float, y: float, eye: Eye):
    return _point_request(request_id, GazeCommand.CALIBRATION_COLLECT_PER_EYE_2D, (x, y), eye)


def calibration_discard_2d(request_id: int, x: float, y: float, eye: Eye = Eye.BOTH):
    return _point_request(request_id, GazeCommand.CALIBRATION_DISCARD_2D, (x, y), eye)


def calibration_discard_3d(request_id: int, x: float, y: float, z: float, eye: Eye = Eye.BOTH):
    return _point_request(request_id, GazeCommand.CALIBRATION_DISCARD_3D, (x, y, z), eye)


def calibration_apply(request_id: int, calibration: bytes):
    """1110 with the opaque blob that 1100 returned (0x1402c5050)."""
    return _gd.calibration_apply(request_id, calibration)


def set_display_area(request_id: int, top_left, top_right, bottom_left):
    """1440 (0x1402c6f40): three float32 points as fixed42 triples, then an empty array of type 0x3039."""
    return _gd.set_display_area(request_id, top_left, top_right, bottom_left)


def set_enabled_eye(request_id: int, eye: Eye):
    return _gd.set_enabled_eye(request_id, int(Eye(eye)))


def enable_extension(request_id: int, extension: int):
    """1800 from vtable slot 0x2e0; no log name, the name is inferred from 1810/1820."""
    return _gd.enable_extension(request_id, extension)


def get_configuration_key(request_id: int, name: str):
    return _gd.get_configuration_key(request_id, name)


def persistent_file_read(request_id: int, name: str):
    return _gd.persistent_file_read(request_id, name)


def persistent_file_write(request_id: int, name: str, data: bytes):
    return _gd.persistent_file_write(request_id, name, data)


def persistent_file_erase(request_id: int, name: str):
    return _gd.persistent_file_erase(request_id, name)


def status_entries(values) -> dict:
    """Key/value pairs of a 1490 (or 1330) reply decoded by session.reply_values."""
    return dict(_gd.status_entries(values))


def device_calibration_id(values) -> int | None:
    """Calibration id from status key 7, parsed with strtoul as 0x1402b9360 does; None when absent."""
    return _gd.device_calibration_id(values)


@dataclass(frozen=True)
class LocalCalibration:
    """calibration.setpm as written by calibration_receive_and_commit (0x140267cf0).

The payload is the calibration id followed by the 1100 reply blob verbatim;
1110 sends that blob back unchanged.
"""
    calibration_id: int
    blob: bytes


def setting_file_payload(data: bytes) -> bytes:
    """Strip the 12-byte setting-file header: version 4, kind 1, payload size."""
    return _gd.setting_file_payload(data)


def setting_file(payload: bytes, kind: int = 1) -> bytes:
    return _gd.setting_file(payload, kind)


def calibration_setting(calibration: LocalCalibration) -> bytes:
    """calibration.setpm bytes for a calibration, the inverse of parse_calibration_setting."""
    return _gd.calibration_setting(calibration)


def parse_calibration_setting(data: bytes) -> LocalCalibration:
    return LocalCalibration(*_gd.parse_calibration_setting(data))


def screen_plane_setting(data: bytes):
    """screenplane.setpm: bottom-left, top-left, top-right corners in millimetres."""
    return _gd.screen_plane_setting(data)


class CalibrationState(IntEnum):
    IN_SYNC = 0
    APPLIED = 1
    APPLY_NEEDED = 2
    DEVICE_ONLY = 3


def synchronize_calibration(session, local: LocalCalibration | None, allow_apply=False):
    """start_synchronize_calibrations (0x140263010) without its destructive branch.

The runtime applies the local calibration when the ids differ, and clears the
device (1010 then 1020) when there is no local calibration; here the clear is
never sent and DEVICE_ONLY is returned instead. Applying needs allow_apply.
"""
    return CalibrationState(drive(_gd.synchronize_calibration(local, allow_apply), session))


def retrieve_calibration(session) -> bytes:
    """Read-only 1100: the device's current calibration blob."""
    return drive(_gd.retrieve_calibration(), session)


STIMULUS_POINT_STRUCT = _gd.STIMULUS_POINT_STRUCT
STIMULUS_POSITION_STRUCT = _gd.STIMULUS_POSITION_STRUCT
STIMULUS_POINT_ITEMS = _gd.STIMULUS_POINT_ITEMS


class PointStatus(IntEnum):
    """tobii_calibration_stimulus_point_status_t and tobii_calibration_point_status_t."""
    FAILED_OR_INVALID = 0
    VALID_BUT_NOT_USED = 1
    VALID_AND_USED = 2


assert [(s.name, s.value) for s in PointStatus] == _gd.POINT_STATUSES


@dataclass(frozen=True)
class EyeQuality:
    status: int
    bias: float
    precision: float


@dataclass(frozen=True)
class StimulusPoint:
    """One record of the 1120 reply (struct 13200), copied in wire order by 0x1402b7830.

Field names follow Stream Engine's tobii_calibration_stimulus_point_data_t
(point, left status/bias/precision, right status/bias/precision). Bias is the
distance from the target to the mean calibrated gaze point and precision its
spread, both in normalized display units.
"""
    point: tuple
    left: EyeQuality
    right: EyeQuality


def stimulus_point(record) -> StimulusPoint:
    """A StimulusPoint from its native (point, left, right) tuple."""
    point, left, right = record
    return StimulusPoint(point, *(EyeQuality(PointStatus(status), bias, precision)
                                  for status, bias, precision in (left, right)))


def stimulus_points(values) -> tuple[StimulusPoint, ...]:
    """Decode the reply values of 1120 (get_calibration_stimulus_pts)."""
    return tuple(stimulus_point(record) for record in _gd.stimulus_points(values))


def tracker_point_status(raw: int) -> PointStatus:
    """0x1402b7830 keeps 0, 1 and 2 and turns any other device status into 0."""
    return PointStatus(raw) if raw in (0, 1, 2) else PointStatus.FAILED_OR_INVALID


def stimulus_points_payload(points) -> list:
    """Reply fields of 1120 for the given records, in the layout stimulus_points reads."""
    return _gd.stimulus_points_payload(points)


@dataclass(frozen=True)
class CalibrationPointData:
    """tobii_calibration_point_data_t as tobii_calibration_parse (0x18007e9b0) reports it."""
    point: tuple
    left_status: PointStatus
    left_mapping: tuple
    right_status: PointStatus
    right_mapping: tuple


CALIBRATION_RECORD = struct.Struct('<2f2fi4x2fi4x')
CALIBRATION_TRAILER = struct.Struct('<4xi')
BLOB_STATUS = {-1: PointStatus.FAILED_OR_INVALID, 0: PointStatus.VALID_BUT_NOT_USED,
               1: PointStatus.VALID_AND_USED}


def calibration_point(record) -> CalibrationPointData:
    point, left_status, left_mapping, right_status, right_mapping = record
    return CalibrationPointData(point, PointStatus(left_status), left_mapping, PointStatus(right_status), right_mapping)


def calibration_points(blob: bytes) -> tuple[CalibrationPointData, ...]:
    """The per-sample point table of a 1100 blob, read as Stream Engine's tobii_calibration_parse does.

Word 0 of the blob is the offset of a trailer whose second word is the record
count; each 40-byte record holds the target, then per eye the mapped gaze point
and a status (-1 failed, 0 valid but unused, 1 valid and used).
"""
    return tuple(calibration_point(record) for record in _gd.calibration_points(blob))


def blob_calibration_id(blob: bytes) -> int:
    """Word 5 of a 1100 blob; on albedo it equals status key 7 and the calibration.setpm id."""
    return _gd.blob_calibration_id(blob)
