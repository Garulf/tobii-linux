"""Device-side gaze configuration of the Eye Tracker 5: calibration, persistent files and start checks.

Recovered from the platform runtime's tracker vtable 0x1410af508 and the
castor platform module that drives it; docs/gaze-device.md has the command
table and evidence. Every encoder here is compared with the original builder
in emulation by tools/gaze_device_oracle.py.

The runtime keeps the user calibration on the device. At start it reads the
device calibration id (status key 7) and compares it with the copy in
calibration.setpm; it applies the local copy (1110) only when they differ.
"""
from dataclasses import dataclass
from enum import IntEnum
import struct

from .ttp import ProtocolError
from .ttp_commands import blob_field, request, string_field, uint32_field
from .ttp_fields import StructHeader, array_fields, fixed42_field, triple_fixed42_fields


class Eye(IntEnum):
    """Eye selector shared by enabled-eye, per-eye calibration and 1011 (0x1402b5290)."""
    LEFT = 1
    RIGHT = 2
    BOTH = 3


class GazeCommand(IntEnum):
    CALIBRATION_START = 1010
    CALIBRATION_START_EYE = 1011
    CALIBRATION_STOP = 1020
    CALIBRATION_COLLECT_2D = 1030
    CALIBRATION_COLLECT_3D = 1031
    CALIBRATION_COLLECT_PER_EYE_2D = 1032
    CALIBRATION_CLEAR = 1060
    CALIBRATION_COMPUTE_AND_APPLY = 1070
    CALIBRATION_COMPUTE_AND_APPLY_PER_EYE = 1071
    CALIBRATION_DISCARD_2D = 1080
    CALIBRATION_DISCARD_3D = 1081
    CALIBRATION_RETRIEVE = 1100
    CALIBRATION_APPLY = 1110
    GET_CALIBRATION_STIMULUS_POINTS = 1120
    GET_CONFIGURATION_KEY = 1310
    SET_DISPLAY_AREA = 1440
    ENABLE_EXTENSION = 1800
    PERSISTENT_FILE_ENUMERATE = 3060
    PERSISTENT_FILE_WRITE = 3070
    PERSISTENT_FILE_READ = 3080
    PERSISTENT_FILE_ERASE = 3090
    SET_ENABLED_EYE = 3160


HEADER_ONLY = frozenset({
    GazeCommand.CALIBRATION_START, GazeCommand.CALIBRATION_STOP, GazeCommand.CALIBRATION_CLEAR,
    GazeCommand.CALIBRATION_COMPUTE_AND_APPLY, GazeCommand.CALIBRATION_COMPUTE_AND_APPLY_PER_EYE,
    GazeCommand.CALIBRATION_RETRIEVE, GazeCommand.GET_CALIBRATION_STIMULUS_POINTS,
    GazeCommand.PERSISTENT_FILE_ENUMERATE,
})

READ_ONLY = frozenset({
    GazeCommand.CALIBRATION_RETRIEVE, GazeCommand.GET_CALIBRATION_STIMULUS_POINTS,
    GazeCommand.GET_CONFIGURATION_KEY, GazeCommand.PERSISTENT_FILE_ENUMERATE,
    GazeCommand.PERSISTENT_FILE_READ,
})

DISPLAY_AREA_TRAILER = 0x3039
STATUS_STRUCT = 10000
STATUS_CALIBRATION_ID = 7
SETTING_FILE_VERSION = 4
SETTING_FILE_HEADER = struct.Struct('<3I')
CALIBRATION_ID = struct.Struct('<I')


def header_only(request_id: int, command: GazeCommand):
    if command not in HEADER_ONLY:
        raise ProtocolError('Command has fields; use its encoder')
    return request(request_id, command)


def calibration_start_eye(request_id: int, eye: Eye):
    """Command 1011 from vtable slot 0x1c0; no log name, so the name is inferred."""
    return request(request_id, GazeCommand.CALIBRATION_START_EYE, (uint32_field(int(Eye(eye))),))


def _point_request(request_id, command, coordinates, eye):
    fields = [fixed42_field(struct.unpack('<f', struct.pack('<f', c))[0]) for c in coordinates]
    return request(request_id, command, fields + [uint32_field(int(Eye(eye)))])


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
    return request(request_id, GazeCommand.CALIBRATION_APPLY, (blob_field(calibration),))


def set_display_area(request_id: int, top_left, top_right, bottom_left):
    """1440 (0x1402c6f40): three float32 points as fixed42 triples, then an empty array of type 0x3039."""
    points = [[struct.unpack('<f', struct.pack('<f', c))[0] for c in p] for p in (top_left, top_right, bottom_left)]
    fields = [f for p in points for f in triple_fixed42_fields(p)] + array_fields(0, DISPLAY_AREA_TRAILER)
    return request(request_id, GazeCommand.SET_DISPLAY_AREA, fields)


def set_enabled_eye(request_id: int, eye: Eye):
    return request(request_id, GazeCommand.SET_ENABLED_EYE, (uint32_field(int(Eye(eye))),))


def enable_extension(request_id: int, extension: int):
    """1800 from vtable slot 0x2e0; no log name, the name is inferred from 1810/1820."""
    return request(request_id, GazeCommand.ENABLE_EXTENSION, (uint32_field(extension),))


def get_configuration_key(request_id: int, name: str):
    return request(request_id, GazeCommand.GET_CONFIGURATION_KEY, (string_field(name),))


def persistent_file_read(request_id: int, name: str):
    return request(request_id, GazeCommand.PERSISTENT_FILE_READ, (string_field(name),))


def persistent_file_write(request_id: int, name: str, data: bytes):
    return request(request_id, GazeCommand.PERSISTENT_FILE_WRITE, (string_field(name), blob_field(data)))


def persistent_file_erase(request_id: int, name: str):
    return request(request_id, GazeCommand.PERSISTENT_FILE_ERASE, (string_field(name),))


def status_entries(values) -> dict:
    """Key/value pairs of a 1490 (or 1330) reply decoded by session.reply_values."""
    entries, index = {}, 0
    while index < len(values):
        item = values[index]
        if not isinstance(item, StructHeader) or item != StructHeader(2, STATUS_STRUCT):
            raise ProtocolError('Expected a status entry header')
        if index + 2 >= len(values):
            raise ProtocolError('Truncated status entry')
        entries[values[index + 1]] = bytes(values[index + 2])
        index += 3
    return entries


def device_calibration_id(values) -> int | None:
    """Calibration id from status key 7, parsed with strtoul as 0x1402b9360 does; None when absent."""
    text = status_entries(values).get(STATUS_CALIBRATION_ID)
    return int(text.decode(), 10) & 0xFFFFFFFF if text else None


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
    if len(data) < SETTING_FILE_HEADER.size:
        raise ProtocolError('Setting file is shorter than its header')
    version, _, size = SETTING_FILE_HEADER.unpack_from(data)
    if version != SETTING_FILE_VERSION or size != len(data) - SETTING_FILE_HEADER.size:
        raise ProtocolError('Unexpected setting file version or size')
    return data[SETTING_FILE_HEADER.size:]


def parse_calibration_setting(data: bytes) -> LocalCalibration:
    payload = setting_file_payload(data)
    if len(payload) <= CALIBRATION_ID.size:
        raise ProtocolError('Calibration setting holds no blob')
    return LocalCalibration(CALIBRATION_ID.unpack_from(payload)[0], payload[CALIBRATION_ID.size:])


def screen_plane_setting(data: bytes):
    """screenplane.setpm: bottom-left, top-left, top-right corners in millimetres."""
    payload = setting_file_payload(data)
    if len(payload) != 36:
        raise ProtocolError('Screen plane setting must hold three points')
    values = struct.unpack('<9f', payload)
    return tuple(tuple(values[i:i + 3]) for i in (0, 3, 6))


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
    from .ttp_commands import Command, header_only_request
    status = session.reply_values(session.call(lambda rid: header_only_request(rid, Command.GET_DEVICE_STATUS)))
    device_id = device_calibration_id(status) or 0
    if local is None or local.calibration_id == 0:
        return CalibrationState.DEVICE_ONLY if device_id else CalibrationState.IN_SYNC
    if device_id == local.calibration_id:
        return CalibrationState.IN_SYNC
    if not allow_apply:
        return CalibrationState.APPLY_NEEDED
    session.call(lambda rid: calibration_apply(rid, local.blob))
    return CalibrationState.APPLIED


def retrieve_calibration(session) -> bytes:
    """Read-only 1100: the device's current calibration blob."""
    reply = session.reply_values(session.call(lambda rid: header_only(rid, GazeCommand.CALIBRATION_RETRIEVE)))
    blobs = [v for v in reply if isinstance(v, (bytes, bytearray))]
    if len(blobs) != 1:
        raise ProtocolError('Calibration reply must hold one blob')
    return bytes(blobs[0])
