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
has the evidence. Steps 1 and 2 and presence decoding are implemented in
crates/tobii-protocol/src/gaze.rs; the dataclasses here carry their results.
"""
from dataclasses import dataclass, fields as dataclass_fields, replace
import json
import struct

from ._native import gaze as _gaze
from .frames import stream_columns
from .prp import GazePoint, ProtocolError as PrpError, encode_envelope, encode_gaze_body
from .ttp import Message
from .ttp_commands import STREAM_DATA_KIND, Stream, wire_stream_id

PARSED_UINT32 = 1
PARSED_INT32 = 2
PARSED_INT64 = 4
PARSED_FLOAT = 5
PARSED_VEC2 = 9
PARSED_VEC3 = 10
VEC2_STRUCT = _gaze.VEC2_STRUCT
VEC3_STRUCT = _gaze.VEC3_STRUCT

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
COLUMNS = dict(_gaze.COLUMNS)
VALIDITY_FIELDS = tuple(f.name for f in dataclass_fields(DeviceGaze)
                        if f.name.endswith(('validity', '_present')))
assert tuple(f.name for f in dataclass_fields(DeviceGaze)) == _gaze.DEVICE_GAZE_FIELDS


def _device(values) -> DeviceGaze | None:
    return None if values is None else DeviceGaze(*values)


def parsed_value(items: tuple) -> tuple:
    """(parsed type, value) of one column value, as the runtime's column reader types it."""
    return _gaze.parsed_value(items)


def decode_gaze_message(payload: bytes) -> DeviceGaze | None:
    """Dispatcher 0x1402c1c30: None when the runtime would drop the message.

    Unknown columns are skipped, a repeated column overwrites the earlier one and
    a known column with another type drops the message without any callback.
    """
    return _device(_gaze.decode_gaze_message(payload))


def gaze_message(message: Message) -> DeviceGaze | None:
    return _device(_gaze.gaze_message(message))


# Platmod records (receive_gaze_data 0x140265d70) -------------------------------------------


@dataclass(frozen=True)
class PointRecord:
    timestamp: int
    valid: int
    point: tuple

    def pack(self) -> bytes:
        return _gaze.pack('point', self)


@dataclass(frozen=True)
class PairRecord:
    """Left and right eye values with their validities (gaze origin, position guide)."""
    timestamp: int
    left_valid: int
    left: tuple
    right_valid: int
    right: tuple

    def pack(self) -> bytes:
        return _gaze.pack('pair', self)


@dataclass(frozen=True)
class OpennessRecord:
    timestamp: int
    left_valid: int
    left: float
    right_valid: int
    right: float

    def pack(self) -> bytes:
        return _gaze.pack('openness', self)


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
        return _gaze.pack('eye', self)

    @classmethod
    def unpack(cls, data: bytes) -> 'EyeData':
        return cls(*_gaze.platmod_from_bytes('eye', data))


@dataclass(frozen=True)
class GazeDataRecord:
    timestamp: int
    left: EyeData
    right: EyeData

    def pack(self) -> bytes:
        return _gaze.pack('gaze_data', self)


def _gaze_data(values) -> GazeDataRecord:
    timestamp, left, right = values
    return GazeDataRecord(timestamp, EyeData(*left), EyeData(*right))


def _records(g: DeviceGaze):
    _, point, responsive, origin, data, openness, guide = _gaze.gaze_frame(g)
    return (PointRecord(*point), PointRecord(*responsive), PairRecord(*origin), _gaze_data(data),
            OpennessRecord(*openness), PairRecord(*guide))


def gaze_point_record(g: DeviceGaze) -> PointRecord:
    return _records(g)[0]


def responsive_gaze_point_record(g: DeviceGaze) -> PointRecord:
    return _records(g)[1]


def gaze_origin_record(g: DeviceGaze) -> PairRecord:
    return _records(g)[2]


def gaze_data_record(g: DeviceGaze) -> GazeDataRecord:
    return _records(g)[3]


def eye_openness_record(g: DeviceGaze) -> OpennessRecord:
    return _records(g)[4]


def user_position_guide_record(g: DeviceGaze) -> PairRecord:
    return _records(g)[5]


def platmod_records(g: DeviceGaze, host_gaze: bool = False) -> dict[str, bytes]:
    """Packed records receive_gaze_data hands to each subscriber.

    With the host gaze setting (settings +0x188) the device's gaze columns are
    ignored and only the user position guide is published; the ET5 runs gaze on
    the device, so the runtime takes the other branch.
    """
    return _gaze.platmod_records(g, host_gaze)


def platmod_from_bytes(name: str, data: bytes):
    values = _gaze.platmod_from_bytes(name, data)
    if name in ('gaze_point', 'responsive_gaze_point'):
        return PointRecord(*values)
    if name == 'eye_openness':
        return OpennessRecord(*values)
    if name in ('gaze_origin', 'user_position_guide'):
        return PairRecord(*values)
    return _gaze_data(values)


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
    return GazeFrame(g, *_records(g))


def frame_from_native(values) -> GazeFrame:
    """GazeFrame of the tuple the native stream reader produces for one gaze message."""
    device, point, responsive, origin, data, openness, guide = values
    return GazeFrame(DeviceGaze(*device), PointRecord(*point), PointRecord(*responsive), PairRecord(*origin),
                     _gaze_data(data), OpennessRecord(*openness), PairRecord(*guide))


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
    return _gaze.platmod_presence(device_value)


def prp_presence(platmod_value: int) -> int:
    """Service callback 0x1400fa2b0: the property 0xB value the client maps to away or present."""
    return _gaze.prp_presence(platmod_value)


PRESENCE_COLUMN_TYPES = {1: PARSED_INT64, 2: PARSED_INT32}


def presence_message(payload: bytes) -> tuple[int, int] | None:
    """(timestamp, device presence value) of one 0x504 message, or None when the runtime drops it.

    parse_presence 0x1402cba80 reads column 1 as int64 and column 2 as int32 (field
    tag 1, which the ET5 sends) and skips other columns; a wrong type fails the parse.
    The dispatcher 0x1402c2700 (index 5) leaves absent columns zero.
    """
    return _gaze.presence_message(payload)


# Building messages (tests and oracle edge cases) --------------------------------------------


def message_columns(payload: bytes) -> dict[int, tuple]:
    return {column.column_id: column.value for column in stream_columns(payload)}


def uint32_column(value: int) -> tuple:
    return tuple(_gaze.uint32_column(value))


def int32_column(value: int) -> tuple:
    return tuple(_gaze.int32_column(value))


def int64_column(value: int) -> tuple:
    return tuple(_gaze.int64_column(value))


def fixed16_column(value: float) -> tuple:
    return tuple(_gaze.fixed16_column(value))


def vec2_column(values) -> tuple:
    return tuple(_gaze.vec2_column(values))


def vec3_column(values) -> tuple:
    return tuple(_gaze.vec3_column(values))


def columns_payload(columns: dict) -> bytes:
    """Column set payload; columns mapped to None are left out."""
    return _gaze.columns_payload([(column_id, value) for column_id, value in columns.items() if value is not None])


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
