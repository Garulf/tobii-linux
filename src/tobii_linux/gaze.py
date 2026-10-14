"""Host-side gaze path of the Windows runtime: device gaze stream 0x500 to PRP stream records.

The ET5 computes gaze on the device. The runtime only routes each 0x500 message:

1. the column dispatcher 0x1402c1c30 copies 40 typed columns into one record and
   drops the whole message when a known column has an unexpected type;
2. receive_gaze_data 0x140265d70 derives six platmod records (gaze point,
   responsive gaze point, eye openness, gaze origin, gaze data, user position
   guide), turning validity codes into 0/1 and clamping the position guide;
3. the service's stream callbacks (0x1400fc170, 0x1400fcdd0, 0x1400fc040,
   0x1400faf30 with 0x1400f9f00, 0x1400fae20, 0x1400fd0f0) copy those into PRP
   stream records, keeping a validity only when it equals 1.

There is no filtering, eye combination or coordinate transform on the host: the
device's combined point (column 28) is the gaze point the game receives.
tools/gaze_oracle.py runs the original code for every step; docs/gaze-host.md
has the evidence.
"""
from dataclasses import dataclass, fields as dataclass_fields, replace
import json
import struct

from .frames import FRAME_STRUCT, COLUMN_STRUCT, stream_columns, struct_code
from .prp import GazePoint, ProtocolError as PrpError, encode_envelope, encode_gaze_body
from .ttp import Field, Message, ProtocolError
from .ttp_commands import (FIELD_FIXED16, FIELD_FIXED42, FIELD_INT32, FIELD_INT64, FIELD_STRUCT,
                           FIELD_UINT32, STREAM_DATA_KIND, Stream, wire_stream_id)
from .ttp_fields import FIXED16_SCALE, FIXED42_SCALE, float32

PARSED_UINT32 = 1
PARSED_INT32 = 2
PARSED_INT64 = 4
PARSED_FLOAT = 5
PARSED_VEC2 = 9
PARSED_VEC3 = 10
VEC2_STRUCT = 0x1F40
VEC3_STRUCT = 0x1F41

OPERATION_STREAM = 1
OPERATION_COMPOUND_STREAM = 0xB
PRP_GAZE_POINT = 3
PRP_GAZE_ORIGIN = 4
PRP_GAZE_DATA = 6
PRP_RESPONSIVE_GAZE_POINT = 0x1D
PRP_EYE_OPENNESS = 0x21
PRP_COMPOUND_USER_POSITION_GUIDE = 2


@dataclass(frozen=True)
class DeviceGaze:
    """The 0x150-byte record of the column dispatcher; absent columns stay zero."""
    timestamp: int = 0
    left_origin_tracker: tuple = (0.0, 0.0, 0.0)
    left_origin_display: tuple = (0.0, 0.0, 0.0)
    left_position_relative_validity: int = 0
    left_position_relative: tuple = (0.0, 0.0, 0.0)
    left_gaze_point_3d: tuple = (0.0, 0.0, 0.0)
    left_gaze_point_2d: tuple = (0.0, 0.0)
    left_pupil_diameter: float = 0.0
    left_validity: int = 0
    right_origin_tracker: tuple = (0.0, 0.0, 0.0)
    right_origin_display: tuple = (0.0, 0.0, 0.0)
    right_position_relative_validity: int = 0
    right_position_relative: tuple = (0.0, 0.0, 0.0)
    right_gaze_point_3d: tuple = (0.0, 0.0, 0.0)
    right_gaze_point_2d: tuple = (0.0, 0.0)
    right_pupil_diameter: float = 0.0
    right_validity: int = 0
    combined_point: tuple = (0.0, 0.0)
    combined_validity: int = 0
    responsive_point: tuple = (0.0, 0.0)
    responsive_validity: int = 0
    left_position_guide_validity: int = 0
    left_position_guide: tuple = (0.0, 0.0, 0.0)
    right_position_guide_validity: int = 0
    right_position_guide: tuple = (0.0, 0.0, 0.0)
    left_origin_tracker_validity: int = 0
    left_origin_display_validity: int = 0
    right_origin_tracker_validity: int = 0
    right_origin_display_validity: int = 0
    left_eyeball_present: int = 0
    left_eyeball_centre: tuple = (0.0, 0.0, 0.0)
    right_eyeball_present: int = 0
    right_eyeball_centre: tuple = (0.0, 0.0, 0.0)
    left_openness: float = 0.0
    left_openness_validity: int = 0
    right_openness: float = 0.0
    right_openness_validity: int = 0


# Column id: (record field, parsed type, field set when the column is present).
COLUMNS = {
    1: ('timestamp', PARSED_INT64, None),
    2: ('left_origin_tracker', PARSED_VEC3, None),
    3: ('left_position_relative', PARSED_VEC3, None),
    4: ('left_gaze_point_3d', PARSED_VEC3, None),
    5: ('left_gaze_point_2d', PARSED_VEC2, None),
    6: ('left_pupil_diameter', PARSED_FLOAT, None),
    7: ('left_validity', PARSED_UINT32, None),
    8: ('right_origin_tracker', PARSED_VEC3, None),
    9: ('right_position_relative', PARSED_VEC3, None),
    10: ('right_gaze_point_3d', PARSED_VEC3, None),
    11: ('right_gaze_point_2d', PARSED_VEC2, None),
    12: ('right_pupil_diameter', PARSED_FLOAT, None),
    13: ('right_validity', PARSED_UINT32, None),
    14: (None, PARSED_UINT32, None),
    17: (None, PARSED_UINT32, None),
    20: (None, PARSED_UINT32, None),
    21: ('right_origin_tracker_validity', PARSED_UINT32, None),
    22: ('left_origin_tracker_validity', PARSED_UINT32, None),
    23: ('left_eyeball_centre', PARSED_VEC3, 'left_eyeball_present'),
    24: ('right_eyeball_centre', PARSED_VEC3, 'right_eyeball_present'),
    27: ('combined_validity', PARSED_UINT32, None),
    28: ('combined_point', PARSED_VEC2, None),
    29: ('left_position_relative_validity', PARSED_UINT32, None),
    30: ('right_position_relative_validity', PARSED_UINT32, None),
    31: ('responsive_validity', PARSED_UINT32, None),
    32: ('responsive_point', PARSED_VEC2, None),
    33: ('left_origin_display_validity', PARSED_UINT32, None),
    34: ('left_origin_display', PARSED_VEC3, None),
    35: ('right_origin_display_validity', PARSED_UINT32, None),
    36: ('right_origin_display', PARSED_VEC3, None),
    37: ('left_position_guide', PARSED_VEC3, None),
    38: ('left_position_guide_validity', PARSED_UINT32, None),
    39: ('right_position_guide', PARSED_VEC3, None),
    40: ('right_position_guide_validity', PARSED_UINT32, None),
    41: ('left_openness', PARSED_FLOAT, None),
    42: ('left_openness_validity', PARSED_UINT32, None),
    43: ('right_openness', PARSED_FLOAT, None),
    44: ('right_openness_validity', PARSED_UINT32, None),
}
VALIDITY_FIELDS = tuple(f.name for f in dataclass_fields(DeviceGaze)
                        if f.name.endswith(('validity', '_present')))


def _fixed42_float(field: Field) -> float:
    return float32(struct.unpack('>q', field.data)[0] * FIXED42_SCALE)


def _fixed16_float(field: Field) -> float:
    return float32(float32(struct.unpack('>i', field.data)[0]) * FIXED16_SCALE)


def parsed_value(items: tuple) -> tuple:
    """(parsed type, value) of one column value, as the runtime's column reader types it."""
    first = items[0]
    if len(items) == 1:
        if first.tag == FIELD_INT64 and len(first.data) == 8:
            return PARSED_INT64, struct.unpack('>q', first.data)[0]
        if first.tag == FIELD_UINT32 and len(first.data) == 4:
            return PARSED_UINT32, struct.unpack('>I', first.data)[0]
        if first.tag == FIELD_INT32 and len(first.data) == 4:
            return PARSED_INT32, struct.unpack('>i', first.data)[0]
        if first.tag == FIELD_FIXED16 and len(first.data) == 4:
            return PARSED_FLOAT, _fixed16_float(first)
    if first.tag == FIELD_STRUCT and all(item.tag == FIELD_FIXED42 and len(item.data) == 8
                                         for item in items[1:]):
        shape = struct_code(first)
        if shape == (2, VEC2_STRUCT) and len(items) == 3:
            return PARSED_VEC2, tuple(_fixed42_float(item) for item in items[1:])
        if shape == (3, VEC3_STRUCT) and len(items) == 4:
            return PARSED_VEC3, tuple(_fixed42_float(item) for item in items[1:])
    return None, None


def decode_gaze_message(payload: bytes) -> DeviceGaze | None:
    """Dispatcher 0x1402c1c30: None when the runtime would drop the message.

    Unknown columns are skipped, a repeated column overwrites the earlier one and
    a known column with another type drops the message without any callback.
    """
    values = {}
    for column in stream_columns(payload):
        if column.column_id not in COLUMNS:
            continue
        name, expected, present = COLUMNS[column.column_id]
        parsed, value = parsed_value(column.value)
        if parsed != expected:
            return None
        if name is not None:
            values[name] = value
        if present is not None:
            values[present] = 1
    return DeviceGaze(**values)


def gaze_message(message: Message) -> DeviceGaze | None:
    if message.kind != STREAM_DATA_KIND or message.command != wire_stream_id(Stream.GAZE):
        raise ProtocolError('Not a gaze stream message')
    return decode_gaze_message(message.payload)


# Platmod records (receive_gaze_data 0x140265d70) -------------------------------------------


@dataclass(frozen=True)
class PointRecord:
    timestamp: int
    valid: int
    point: tuple

    def pack(self) -> bytes:
        return struct.pack('<qI2f', self.timestamp, self.valid, *self.point)


@dataclass(frozen=True)
class PairRecord:
    """Left and right eye values with their validities (gaze origin, position guide)."""
    timestamp: int
    left_valid: int
    left: tuple
    right_valid: int
    right: tuple

    def pack(self) -> bytes:
        return struct.pack('<qI3fI3f', self.timestamp, self.left_valid, *self.left,
                           self.right_valid, *self.right)


@dataclass(frozen=True)
class OpennessRecord:
    timestamp: int
    left_valid: int
    left: float
    right_valid: int
    right: float

    def pack(self) -> bytes:
        return struct.pack('<qIfIf', self.timestamp, self.left_valid, self.left,
                           self.right_valid, self.right)


@dataclass(frozen=True)
class EyeData:
    origin_valid: int
    origin: tuple
    position_valid: int
    position: tuple
    eye_valid: int
    gaze_point_3d: tuple
    gaze_point_2d: tuple
    eyeball_valid: int
    eyeball_centre: tuple
    pupil_valid: int
    pupil_diameter: float

    PACKED = struct.Struct('<I3fI3fI3f2fI3fIf')

    def pack(self) -> bytes:
        return self.PACKED.pack(self.origin_valid, *self.origin, self.position_valid,
                                *self.position, self.eye_valid, *self.gaze_point_3d,
                                *self.gaze_point_2d, self.eyeball_valid, *self.eyeball_centre,
                                self.pupil_valid, self.pupil_diameter)

    @classmethod
    def unpack(cls, data: bytes) -> 'EyeData':
        v = cls.PACKED.unpack(data)
        return cls(v[0], v[1:4], v[4], v[5:8], v[8], v[9:12], v[12:14], v[14], v[15:18],
                   v[18], v[19])


@dataclass(frozen=True)
class GazeDataRecord:
    timestamp: int
    left: EyeData
    right: EyeData

    def pack(self) -> bytes:
        return struct.pack('<q', self.timestamp) + self.left.pack() + self.right.pack()


def _flag(condition: bool) -> int:
    return 1 if condition else 0


def _eye_valid(validity: int) -> int:
    return _flag(validity < 2)


def _unit_clamp(value: float) -> float:
    if value < 0.0:
        value = 0.0
    if 1.0 < value:
        value = 1.0
    return value


def _eye_data(origin_valid, origin, position_valid, position, validity, point_3d, point_2d,
              eyeball_present, eyeball, pupil) -> EyeData:
    valid = _eye_valid(validity)
    return EyeData(_flag(origin_valid == 1), origin, _flag(position_valid == 1), position,
                   valid, point_3d, point_2d, valid if eyeball_present == 1 else 0, eyeball,
                   valid, pupil)


def gaze_point_record(g: DeviceGaze) -> PointRecord:
    return PointRecord(g.timestamp, _flag(g.combined_validity != 0), g.combined_point)


def responsive_gaze_point_record(g: DeviceGaze) -> PointRecord:
    return PointRecord(g.timestamp, _flag(g.responsive_validity != 0), g.responsive_point)


def eye_openness_record(g: DeviceGaze) -> OpennessRecord:
    return OpennessRecord(g.timestamp, _flag(g.left_openness_validity != 0), g.left_openness,
                          _flag(g.right_openness_validity != 0), g.right_openness)


def gaze_origin_record(g: DeviceGaze) -> PairRecord:
    return PairRecord(g.timestamp, _flag(g.left_origin_display_validity == 1),
                      g.left_origin_display, _flag(g.right_origin_display_validity == 1),
                      g.right_origin_display)


def gaze_data_record(g: DeviceGaze) -> GazeDataRecord:
    left = _eye_data(g.left_origin_tracker_validity, g.left_origin_tracker,
                     g.left_position_relative_validity, g.left_position_relative,
                     g.left_validity, g.left_gaze_point_3d, g.left_gaze_point_2d,
                     g.left_eyeball_present, g.left_eyeball_centre, g.left_pupil_diameter)
    right = _eye_data(g.right_origin_tracker_validity, g.right_origin_tracker,
                      g.right_position_relative_validity, g.right_position_relative,
                      g.right_validity, g.right_gaze_point_3d, g.right_gaze_point_2d,
                      g.right_eyeball_present, g.right_eyeball_centre, g.right_pupil_diameter)
    return GazeDataRecord(g.timestamp, left, right)


def user_position_guide_record(g: DeviceGaze) -> PairRecord:
    return PairRecord(g.timestamp, _flag(g.left_position_guide_validity == 1),
                      tuple(_unit_clamp(v) for v in g.left_position_guide),
                      _flag(g.right_position_guide_validity == 1),
                      tuple(_unit_clamp(v) for v in g.right_position_guide))


DEVICE_GAZE_RECORDS = {
    'gaze_point': gaze_point_record,
    'responsive_gaze_point': responsive_gaze_point_record,
    'eye_openness': eye_openness_record,
    'gaze_origin': gaze_origin_record,
    'gaze_data': gaze_data_record,
}


def platmod_records(g: DeviceGaze, host_gaze: bool = False) -> dict[str, bytes]:
    """Packed records receive_gaze_data hands to each subscriber.

    With the host gaze setting (settings +0x188) the device's gaze columns are
    ignored and only the user position guide is published; the ET5 runs gaze on
    the device, so the runtime takes the other branch.
    """
    records = {} if host_gaze else {name: build(g).pack()
                                     for name, build in DEVICE_GAZE_RECORDS.items()}
    records['user_position_guide'] = user_position_guide_record(g).pack()
    return records


def platmod_from_bytes(name: str, data: bytes):
    if name in ('gaze_point', 'responsive_gaze_point'):
        t, valid, x, y = struct.unpack('<qI2f', data[:0x14])
        return PointRecord(t, valid, (x, y))
    if name == 'eye_openness':
        return OpennessRecord(*struct.unpack('<qIfIf', data[:0x18]))
    if name in ('gaze_origin', 'user_position_guide'):
        v = struct.unpack('<qI3fI3f', data[:0x28])
        return PairRecord(v[0], v[1], v[2:5], v[5], v[6:9])
    if name == 'gaze_data':
        size = EyeData.PACKED.size
        return GazeDataRecord(struct.unpack_from('<q', data)[0], EyeData.unpack(data[8:8 + size]),
                              EyeData.unpack(data[8 + size:8 + 2 * size]))
    raise ValueError(f'Unknown platmod record {name!r}')


# PRP stream records (service callbacks) -----------------------------------------------------


@dataclass(frozen=True)
class PrpRecord:
    """Operation, stream id and the native stream data the PRP serializer reads."""
    kind: int
    stream: int
    native: bytes


def _bool(value: int) -> int:
    return 1 if value == 1 else 0


def _pair_native(r: PairRecord) -> bytes:
    return struct.pack('<qB3x3fB3x3f', r.timestamp, _bool(r.left_valid), *r.left,
                       _bool(r.right_valid), *r.right)


def _eye_native(e: EyeData) -> bytes:
    return struct.pack('<B3x3fB3x3fB3x3f2fB3x3fB3xf', _bool(e.origin_valid), *e.origin,
                       _bool(e.position_valid), *e.position, _bool(e.eye_valid),
                       *e.gaze_point_3d, *e.gaze_point_2d, _bool(e.eyeball_valid),
                       *e.eyeball_centre, _bool(e.pupil_valid), e.pupil_diameter)


PRP_STREAMS = {
    'gaze_point': (OPERATION_STREAM, PRP_GAZE_POINT),
    'responsive_gaze_point': (OPERATION_STREAM, PRP_RESPONSIVE_GAZE_POINT),
    'gaze_origin': (OPERATION_STREAM, PRP_GAZE_ORIGIN),
    'gaze_data': (OPERATION_STREAM, PRP_GAZE_DATA),
    'eye_openness': (OPERATION_STREAM, PRP_EYE_OPENNESS),
    'user_position_guide': (OPERATION_COMPOUND_STREAM, PRP_COMPOUND_USER_POSITION_GUIDE),
}


def prp_record(name: str, record) -> PrpRecord:
    kind, stream = PRP_STREAMS[name]
    if name in ('gaze_point', 'responsive_gaze_point'):
        native = struct.pack('<qB3x2f', record.timestamp, _bool(record.valid), *record.point)
    elif name == 'eye_openness':
        native = struct.pack('<qB3xfB3xf', record.timestamp, _bool(record.left_valid),
                             record.left, _bool(record.right_valid), record.right)
    elif name in ('gaze_origin', 'user_position_guide'):
        native = _pair_native(record)
    else:
        native = struct.pack('<q', record.timestamp) + _eye_native(record.left) + _eye_native(record.right)
    return PrpRecord(kind, stream, native)


# PRP stream bodies and packets (game DLL serializer 0x180052e60, packet 0x18004f730) ---------


def _timestamp_words(timestamp: int) -> tuple[int, int]:
    value = timestamp & 0xFFFFFFFFFFFFFFFF
    return value >> 32, value & 0xFFFFFFFF


def pair_body(native: bytes) -> bytes:
    """Stream 4 (gaze origin) and compound stream 2 (user position guide) share this body."""
    t, lv, lx, ly, lz, rv, rx, ry, rz = struct.unpack('<qB3x3fB3x3f', native)
    return struct.pack('<IIII3fI3f', 44, *_timestamp_words(t), lv, lx, ly, lz, rv, rx, ry, rz)


def _eye_words(native: bytes) -> tuple[bytes, int]:
    v = struct.unpack('<B3x3fB3x3fB3x3f2fB3x3fB3xf', native)
    origin_valid, origin, position_valid, position = v[0], v[1:4], v[4], v[5:8]
    eye_valid, point_3d, point_2d = v[8], v[9:12], v[12:14]
    eyeball_valid, eyeball, pupil_valid, pupil = v[14], v[15:18], v[18], v[19]
    words = struct.pack('<I3f3fI3f2fI3fIf', origin_valid, *origin, *position, eye_valid,
                        *point_3d, *point_2d, eyeball_valid, *eyeball, pupil_valid, pupil)
    return words, position_valid


def gaze_data_body(native: bytes, extended: bool) -> bytes:
    """Stream 6: 0xa4 bytes, or 0xac with the two position validities a newer peer reads."""
    t = struct.unpack_from('<q', native)[0]
    eye = struct.calcsize('<B3x3fB3x3fB3x3f2fB3x3fB3xf')
    left, left_position_valid = _eye_words(native[8:8 + eye])
    right, right_position_valid = _eye_words(native[8 + eye:8 + 2 * eye])
    body = struct.pack('<III', 0xAC if extended else 0xA4, *_timestamp_words(t)) + left + right
    return body + (struct.pack('<II', left_position_valid, right_position_valid) if extended else b'')


def stream_body(record: PrpRecord, extended: bool = True) -> bytes | None:
    """Body the game DLL's serializer writes for this record, None where it has no case."""
    if record.kind == OPERATION_COMPOUND_STREAM:
        return pair_body(record.native) if record.stream == PRP_COMPOUND_USER_POSITION_GUIDE else None
    if record.stream in (PRP_GAZE_POINT, PRP_RESPONSIVE_GAZE_POINT):
        t, valid, x, y = struct.unpack('<qB3x2f', record.native)
        return encode_gaze_body(GazePoint(t, bool(valid), x, y))
    if record.stream == PRP_GAZE_ORIGIN:
        return pair_body(record.native)
    if record.stream == PRP_GAZE_DATA:
        return gaze_data_body(record.native, extended)
    return None


def stream_packet(record: PrpRecord, message_tag: int = 0, extended: bool = True) -> bytes | None:
    """Stream package with empty extensions, the layout the real service sends.

    Operation 1 for streams, 11 for compound streams; the framing is otherwise the same.
    """
    body = stream_body(record, extended)
    if body is None:
        return None
    if not 0 <= message_tag <= 0xFFFFFFFF:
        raise PrpError('message tag is outside unsigned 32-bit range')
    header = struct.pack('<7I', 12, message_tag, record.kind, 4, 8, record.stream, 4)
    return encode_envelope(header + body + struct.pack('<II', 4, 0))


# One device message to every gaze output ----------------------------------------------------


@dataclass(frozen=True)
class GazeFrame:
    """Everything the runtime publishes for one device gaze message."""
    device: DeviceGaze
    gaze_point: PointRecord
    responsive_gaze_point: PointRecord
    gaze_origin: PairRecord
    gaze_data: GazeDataRecord
    eye_openness: OpennessRecord
    user_position_guide: PairRecord

    @property
    def timestamp_us(self) -> int:
        return self.device.timestamp

    def prp(self, name: str) -> PrpRecord:
        return prp_record(name, getattr(self, name))

    def with_timestamp(self, timestamp_us: int) -> 'GazeFrame':
        """Same outputs restamped, for moving device time onto the service clock."""
        return gaze_frame(replace(self.device, timestamp=timestamp_us))


def gaze_frame(g: DeviceGaze) -> GazeFrame:
    return GazeFrame(g, gaze_point_record(g), responsive_gaze_point_record(g),
                     gaze_origin_record(g), gaze_data_record(g), eye_openness_record(g),
                     user_position_guide_record(g))


PRP_STREAM_RECORDS = {PRP_GAZE_POINT: 'gaze_point', PRP_RESPONSIVE_GAZE_POINT: 'responsive_gaze_point',
                      PRP_GAZE_ORIGIN: 'gaze_origin', PRP_GAZE_DATA: 'gaze_data'}


def frame_packet(frame: GazeFrame, stream: int, message_tag: int = 0,
                 extended: bool = True) -> bytes:
    return stream_packet(frame.prp(PRP_STREAM_RECORDS[stream]), message_tag, extended)


# User presence (device stream 0x504) --------------------------------------------------------

PRESENCE_UNKNOWN, PRESENCE_AWAY, PRESENCE_PRESENT = 1, 2, 3
PLATMOD_UNKNOWN, PLATMOD_AWAY, PLATMOD_PRESENT = 0, 1, 2
PROPERTY_USER_PRESENCE = 0xB


def platmod_presence(device_value: int) -> int:
    """receive_presence_data 0x140266b60: device values 0, 1, 2 pass, anything else becomes 0."""
    return device_value if device_value in (0, 1, 2) else 0


def prp_presence(platmod_value: int) -> int:
    """Service callback 0x1400fa2b0: the property 0xB value the client maps to away or present."""
    return {1: PRESENCE_AWAY, 2: PRESENCE_PRESENT}.get(platmod_value, PRESENCE_UNKNOWN)


PRESENCE_COLUMN_TYPES = {1: PARSED_INT64, 2: PARSED_INT32}


def presence_message(payload: bytes) -> tuple[int, int] | None:
    """(timestamp, device presence value) of one 0x504 message, or None when the runtime drops it.

    parse_presence 0x1402cba80 reads column 1 as int64 and column 2 as int32 (field
    tag 1, which the ET5 sends) and skips other columns; a wrong type fails the parse.
    The dispatcher 0x1402c2700 (index 5) leaves absent columns zero.
    """
    values = {}
    for column in stream_columns(payload):
        expected = PRESENCE_COLUMN_TYPES.get(column.column_id)
        if expected is None:
            continue
        parsed, value = parsed_value(column.value)
        if parsed != expected:
            return None
        values[column.column_id] = value
    return values.get(1, 0), values.get(2, 0)


# Building messages (tests and oracle edge cases) --------------------------------------------


def message_columns(payload: bytes) -> dict[int, tuple]:
    return {column.column_id: column.value for column in stream_columns(payload)}


def _field(tag: int, form: str, value) -> Field:
    return Field(tag, struct.pack(form, value))


def uint32_column(value: int) -> tuple:
    return (_field(FIELD_UINT32, '>I', value),)


def int32_column(value: int) -> tuple:
    return (_field(FIELD_INT32, '>i', value),)


def int64_column(value: int) -> tuple:
    return (_field(FIELD_INT64, '>q', value),)


def fixed16_column(value: float) -> tuple:
    return (_field(FIELD_FIXED16, '>i', int(float32(value) * 65536.0)),)


def _fixed42(value: float) -> Field:
    return _field(FIELD_FIXED42, '>q', int(value * 2.0 ** 42))


def vec2_column(values) -> tuple:
    return (_field(FIELD_STRUCT, '>I', 2 << 16 | VEC2_STRUCT),) + tuple(_fixed42(v) for v in values)


def vec3_column(values) -> tuple:
    return (_field(FIELD_STRUCT, '>I', 3 << 16 | VEC3_STRUCT),) + tuple(_fixed42(v) for v in values)


def columns_payload(columns: dict) -> bytes:
    """Column set payload; columns mapped to None are left out."""
    present = [(column_id, value) for column_id, value in columns.items() if value is not None]
    items = [_field(FIELD_STRUCT, '>I', len(present) << 16 | FRAME_STRUCT)]
    for column_id, value in present:
        items += [_field(FIELD_STRUCT, '>I', 2 << 16 | COLUMN_STRUCT),
                  _field(FIELD_UINT32, '>I', column_id), *value]
    return b'\0\0' + b''.join(item.encode() for item in items)


def gaze_stream_message(columns: dict) -> Message:
    return Message(STREAM_DATA_KIND, 0, 1, wire_stream_id(Stream.GAZE), 0, columns_payload(columns))


# Offline conversion of recorded device messages ----------------------------------------------


def recorded_gaze_messages(path):
    """(host receipt time, Message) for every 0x500 message in a capture JSONL file.

    Accepts the rows written by tools/linux_stream_capture.py and
    tools/gaze_device_capture.py: kind "message" with the TTP command, header words
    and the payload in hex. A row without "message_kind" is a stream message.
    """
    with open(path) as source:
        for line in source:
            row = json.loads(line)
            if row.get('kind') != 'message' or row.get('command') != wire_stream_id(Stream.GAZE):
                continue
            yield row.get('time'), Message(row.get('message_kind', STREAM_DATA_KIND), 0,
                                           row.get('word2', 0), row['command'],
                                           row.get('word4', 0), bytes.fromhex(row['payload']))


def convert_messages(messages):
    """GazeFrame per message the runtime would publish; dropped messages are skipped."""
    for received, message in messages:
        device = gaze_message(message)
        if device is not None:
            yield received, gaze_frame(device)


def frame_summary(frame: GazeFrame) -> dict:
    """JSON-friendly view of the outputs a Stream Engine client would receive."""
    point, responsive = frame.gaze_point, frame.responsive_gaze_point
    origin, guide, openness = frame.gaze_origin, frame.user_position_guide, frame.eye_openness
    return {'timestamp_us': frame.timestamp_us,
            'gaze_point': {'valid': bool(point.valid), 'xy': list(point.point)},
            'responsive_gaze_point': {'valid': bool(responsive.valid), 'xy': list(responsive.point)},
            'gaze_origin': {'left_valid': bool(origin.left_valid), 'left_mm': list(origin.left),
                            'right_valid': bool(origin.right_valid), 'right_mm': list(origin.right)},
            'user_position_guide': {'left_valid': bool(guide.left_valid), 'left': list(guide.left),
                                    'right_valid': bool(guide.right_valid),
                                    'right': list(guide.right)},
            'eye_openness': {'left_valid': bool(openness.left_valid), 'left': openness.left,
                             'right_valid': bool(openness.right_valid), 'right': openness.right}}


def main(argv=None) -> None:
    import argparse
    parser = argparse.ArgumentParser(
        description='Convert recorded 0x500 gaze messages into the outputs the Windows runtime publishes.')
    parser.add_argument('capture', help='JSONL capture with "message" rows (stream 0x500)')
    args = parser.parse_args(argv)
    for received, frame in convert_messages(recorded_gaze_messages(args.capture)):
        print(json.dumps({'received': received, **frame_summary(frame)}))


if __name__ == '__main__':
    main()
