//! Host side of the gaze path: device gaze stream 0x500 to the platmod records, and
//! user presence stream 0x504.
//!
//! The ET5 computes gaze on the device; the runtime only routes each message:
//!
//! 1. the column dispatcher 0x1402c1c30 copies 40 typed columns into one record and
//!    drops the whole message when a known column has an unexpected type;
//! 2. receive_gaze_data 0x140265d70 derives six platmod records, turning validity codes
//!    into 0 or 1 and clamping the user position guide.
//!
//! There is no filtering, eye combination or coordinate transform on the host.
//! tools/gaze_oracle.py runs the original code for every step; docs/gaze-host.md has the
//! evidence.
//!
//! The runtime stores every gaze value in float32. Decoding rounds to float32 exactly where
//! the runtime does; the records then only copy, compare and clamp to 0 or 1, which give
//! the same result in double precision, so values are kept as `f64` holding float32
//! numbers. Packing narrows them back to float32.

use crate::commands::{STREAM_DATA_KIND, Stream};
use crate::error::{Result, fail};
use crate::fields::wire;
use crate::frames::{COLUMN_STRUCT, FRAME_STRUCT, stream_columns, struct_code};
use crate::numeric::{fixed16_f32, fixed42_f32, round_f32};
use crate::ttp::{Field, Message, be_u32, encode_payload};

pub const VEC2_STRUCT: u32 = 0x1F40;
pub const VEC3_STRUCT: u32 = 0x1F41;

pub const PRESENCE_UNKNOWN: u32 = 1;
pub const PRESENCE_AWAY: u32 = 2;
pub const PRESENCE_PRESENT: u32 = 3;
pub const PLATMOD_UNKNOWN: i32 = 0;
pub const PLATMOD_AWAY: i32 = 1;
pub const PLATMOD_PRESENT: i32 = 2;
pub const PROPERTY_USER_PRESENCE: u32 = 0xB;

pub type Vec2 = [f64; 2];
pub type Vec3 = [f64; 3];

/// A column value as the runtime's column reader types it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Parsed {
    UInt32(u32),
    Int32(i32),
    Int64(i64),
    Float(f64),
    Vec2(Vec2),
    Vec3(Vec3),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedType {
    UInt32 = 1,
    Int32 = 2,
    Int64 = 4,
    Float = 5,
    Vec2 = 9,
    Vec3 = 10,
}

impl Parsed {
    pub fn kind(&self) -> ParsedType {
        match self {
            Parsed::UInt32(_) => ParsedType::UInt32,
            Parsed::Int32(_) => ParsedType::Int32,
            Parsed::Int64(_) => ParsedType::Int64,
            Parsed::Float(_) => ParsedType::Float,
            Parsed::Vec2(_) => ParsedType::Vec2,
            Parsed::Vec3(_) => ParsedType::Vec3,
        }
    }
}

fn be_i64(data: &[u8]) -> i64 {
    i64::from_be_bytes(data[..8].try_into().expect("eight bytes"))
}

fn fixed42_item(item: &Field) -> f64 {
    fixed42_f32(be_i64(&item.data))
}

/// `None` where the runtime's column reader finds no type it knows.
pub fn parsed_value(items: &[Field]) -> Result<Option<Parsed>> {
    let first = &items[0];
    if items.len() == 1 {
        let data = &first.data;
        match (first.tag, data.len()) {
            (wire::INT64, 8) => return Ok(Some(Parsed::Int64(be_i64(data)))),
            (wire::UINT32, 4) => return Ok(Some(Parsed::UInt32(be_u32(data, 0)))),
            (wire::INT32, 4) => return Ok(Some(Parsed::Int32(be_u32(data, 0) as i32))),
            (wire::FIXED16, 4) => return Ok(Some(Parsed::Float(fixed16_f32(be_u32(data, 0) as i32)))),
            _ => {}
        }
    }
    if first.tag == wire::STRUCT && items[1..].iter().all(|item| item.tag == wire::FIXED42 && item.data.len() == 8) {
        let shape = struct_code(first)?;
        if shape == (2, VEC2_STRUCT) && items.len() == 3 {
            return Ok(Some(Parsed::Vec2([fixed42_item(&items[1]), fixed42_item(&items[2])])));
        }
        if shape == (3, VEC3_STRUCT) && items.len() == 4 {
            return Ok(Some(Parsed::Vec3([fixed42_item(&items[1]), fixed42_item(&items[2]), fixed42_item(&items[3])])));
        }
    }
    Ok(None)
}

/// The 0x150-byte record of the column dispatcher; absent columns stay zero.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceGaze {
    pub timestamp: i64,
    pub left_origin_tracker: Vec3,
    pub left_origin_display: Vec3,
    pub left_position_relative_validity: u32,
    pub left_position_relative: Vec3,
    pub left_gaze_point_3d: Vec3,
    pub left_gaze_point_2d: Vec2,
    pub left_pupil_diameter: f64,
    pub left_validity: u32,
    pub right_origin_tracker: Vec3,
    pub right_origin_display: Vec3,
    pub right_position_relative_validity: u32,
    pub right_position_relative: Vec3,
    pub right_gaze_point_3d: Vec3,
    pub right_gaze_point_2d: Vec2,
    pub right_pupil_diameter: f64,
    pub right_validity: u32,
    pub combined_point: Vec2,
    pub combined_validity: u32,
    pub responsive_point: Vec2,
    pub responsive_validity: u32,
    pub left_position_guide_validity: u32,
    pub left_position_guide: Vec3,
    pub right_position_guide_validity: u32,
    pub right_position_guide: Vec3,
    pub left_origin_tracker_validity: u32,
    pub left_origin_display_validity: u32,
    pub right_origin_tracker_validity: u32,
    pub right_origin_display_validity: u32,
    pub left_eyeball_present: u32,
    pub left_eyeball_centre: Vec3,
    pub right_eyeball_present: u32,
    pub right_eyeball_centre: Vec3,
    pub left_openness: f64,
    pub left_openness_validity: u32,
    pub right_openness: f64,
    pub right_openness_validity: u32,
}

/// Every column the dispatcher (0x1402c1c30) knows: id, record field, parsed type and
/// the presence flag it sets. Columns without a field are type-checked and dropped.
pub const COLUMNS: [(u32, Option<&str>, ParsedType, Option<&str>); 38] = {
    use ParsedType::*;
    [
        (1, Some("timestamp"), Int64, None),
        (2, Some("left_origin_tracker"), Vec3, None),
        (3, Some("left_position_relative"), Vec3, None),
        (4, Some("left_gaze_point_3d"), Vec3, None),
        (5, Some("left_gaze_point_2d"), Vec2, None),
        (6, Some("left_pupil_diameter"), Float, None),
        (7, Some("left_validity"), UInt32, None),
        (8, Some("right_origin_tracker"), Vec3, None),
        (9, Some("right_position_relative"), Vec3, None),
        (10, Some("right_gaze_point_3d"), Vec3, None),
        (11, Some("right_gaze_point_2d"), Vec2, None),
        (12, Some("right_pupil_diameter"), Float, None),
        (13, Some("right_validity"), UInt32, None),
        (14, None, UInt32, None),
        (17, None, UInt32, None),
        (20, None, UInt32, None),
        (21, Some("right_origin_tracker_validity"), UInt32, None),
        (22, Some("left_origin_tracker_validity"), UInt32, None),
        (23, Some("left_eyeball_centre"), Vec3, Some("left_eyeball_present")),
        (24, Some("right_eyeball_centre"), Vec3, Some("right_eyeball_present")),
        (27, Some("combined_validity"), UInt32, None),
        (28, Some("combined_point"), Vec2, None),
        (29, Some("left_position_relative_validity"), UInt32, None),
        (30, Some("right_position_relative_validity"), UInt32, None),
        (31, Some("responsive_validity"), UInt32, None),
        (32, Some("responsive_point"), Vec2, None),
        (33, Some("left_origin_display_validity"), UInt32, None),
        (34, Some("left_origin_display"), Vec3, None),
        (35, Some("right_origin_display_validity"), UInt32, None),
        (36, Some("right_origin_display"), Vec3, None),
        (37, Some("left_position_guide"), Vec3, None),
        (38, Some("left_position_guide_validity"), UInt32, None),
        (39, Some("right_position_guide"), Vec3, None),
        (40, Some("right_position_guide_validity"), UInt32, None),
        (41, Some("left_openness"), Float, None),
        (42, Some("left_openness_validity"), UInt32, None),
        (43, Some("right_openness"), Float, None),
        (44, Some("right_openness_validity"), UInt32, None),
    ]
};

/// Parsed type the dispatcher expects for a column it knows.
pub fn expected_column_type(column_id: u32) -> Option<ParsedType> {
    COLUMNS.iter().find(|(id, ..)| *id == column_id).map(|(_, _, kind, _)| *kind)
}

impl DeviceGaze {
    /// Store one column whose type was already checked against [`expected_column_type`].
    fn store(&mut self, column_id: u32, value: Parsed) {
        use Parsed::*;
        match (column_id, value) {
            (1, Int64(v)) => self.timestamp = v,
            (2, Vec3(v)) => self.left_origin_tracker = v,
            (3, Vec3(v)) => self.left_position_relative = v,
            (4, Vec3(v)) => self.left_gaze_point_3d = v,
            (5, Vec2(v)) => self.left_gaze_point_2d = v,
            (6, Float(v)) => self.left_pupil_diameter = v,
            (7, UInt32(v)) => self.left_validity = v,
            (8, Vec3(v)) => self.right_origin_tracker = v,
            (9, Vec3(v)) => self.right_position_relative = v,
            (10, Vec3(v)) => self.right_gaze_point_3d = v,
            (11, Vec2(v)) => self.right_gaze_point_2d = v,
            (12, Float(v)) => self.right_pupil_diameter = v,
            (13, UInt32(v)) => self.right_validity = v,
            (21, UInt32(v)) => self.right_origin_tracker_validity = v,
            (22, UInt32(v)) => self.left_origin_tracker_validity = v,
            (23, Vec3(v)) => {
                self.left_eyeball_centre = v;
                self.left_eyeball_present = 1;
            }
            (24, Vec3(v)) => {
                self.right_eyeball_centre = v;
                self.right_eyeball_present = 1;
            }
            (27, UInt32(v)) => self.combined_validity = v,
            (28, Vec2(v)) => self.combined_point = v,
            (29, UInt32(v)) => self.left_position_relative_validity = v,
            (30, UInt32(v)) => self.right_position_relative_validity = v,
            (31, UInt32(v)) => self.responsive_validity = v,
            (32, Vec2(v)) => self.responsive_point = v,
            (33, UInt32(v)) => self.left_origin_display_validity = v,
            (34, Vec3(v)) => self.left_origin_display = v,
            (35, UInt32(v)) => self.right_origin_display_validity = v,
            (36, Vec3(v)) => self.right_origin_display = v,
            (37, Vec3(v)) => self.left_position_guide = v,
            (38, UInt32(v)) => self.left_position_guide_validity = v,
            (39, Vec3(v)) => self.right_position_guide = v,
            (40, UInt32(v)) => self.right_position_guide_validity = v,
            (41, Float(v)) => self.left_openness = v,
            (42, UInt32(v)) => self.left_openness_validity = v,
            (43, Float(v)) => self.right_openness = v,
            (44, UInt32(v)) => self.right_openness_validity = v,
            _ => {}
        }
    }
}

/// Dispatcher 0x1402c1c30: `None` when the runtime would drop the message.
///
/// Unknown columns are skipped, a repeated column overwrites the earlier one and a known
/// column with another type drops the message without any callback.
pub fn decode_gaze_message(payload: &[u8]) -> Result<Option<DeviceGaze>> {
    let mut gaze = DeviceGaze::default();
    for column in stream_columns(payload)? {
        let Some(expected) = expected_column_type(column.column_id) else {
            continue;
        };
        match parsed_value(&column.value)? {
            Some(parsed) if parsed.kind() == expected => gaze.store(column.column_id, parsed),
            _ => return Ok(None),
        }
    }
    Ok(Some(gaze))
}

pub fn gaze_message(message: &Message) -> Result<Option<DeviceGaze>> {
    if message.kind != STREAM_DATA_KIND || message.command != Stream::Gaze.wire_id() {
        return fail("Not a gaze stream message");
    }
    decode_gaze_message(&message.payload)
}

/// Platmod records (receive_gaze_data 0x140265d70).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointRecord {
    pub timestamp: i64,
    pub valid: u32,
    pub point: Vec2,
}

/// Left and right eye values with their validities (gaze origin, user position guide).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairRecord {
    pub timestamp: i64,
    pub left_valid: u32,
    pub left: Vec3,
    pub right_valid: u32,
    pub right: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpennessRecord {
    pub timestamp: i64,
    pub left_valid: u32,
    pub left: f64,
    pub right_valid: u32,
    pub right: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeData {
    pub origin_valid: u32,
    pub origin: Vec3,
    pub position_valid: u32,
    pub position: Vec3,
    pub eye_valid: u32,
    pub gaze_point_3d: Vec3,
    pub gaze_point_2d: Vec2,
    pub eyeball_valid: u32,
    pub eyeball_centre: Vec3,
    pub pupil_valid: u32,
    pub pupil_diameter: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GazeDataRecord {
    pub timestamp: i64,
    pub left: EyeData,
    pub right: EyeData,
}

/// Little-endian packing of platmod records, narrowing every value to float32.
#[derive(Debug, Default)]
pub struct Packer(Vec<u8>);

impl Packer {
    pub fn i64(mut self, value: i64) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn u32(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn f32(mut self, value: f64) -> Self {
        self.0.extend_from_slice(&(value as f32).to_le_bytes());
        self
    }

    pub fn floats(self, values: &[f64]) -> Self {
        values.iter().fold(self, |packer, value| packer.f32(*value))
    }

    pub fn bytes(mut self, data: &[u8]) -> Self {
        self.0.extend_from_slice(data);
        self
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

impl PointRecord {
    pub const SIZE: usize = 0x14;

    pub fn pack(&self) -> Vec<u8> {
        Packer::default().i64(self.timestamp).u32(self.valid).floats(&self.point).finish()
    }
}

impl PairRecord {
    pub const SIZE: usize = 0x28;

    pub fn pack(&self) -> Vec<u8> {
        Packer::default()
            .i64(self.timestamp)
            .u32(self.left_valid)
            .floats(&self.left)
            .u32(self.right_valid)
            .floats(&self.right)
            .finish()
    }
}

impl OpennessRecord {
    pub const SIZE: usize = 0x18;

    pub fn pack(&self) -> Vec<u8> {
        Packer::default()
            .i64(self.timestamp)
            .u32(self.left_valid)
            .f32(self.left)
            .u32(self.right_valid)
            .f32(self.right)
            .finish()
    }
}

impl EyeData {
    pub const SIZE: usize = 80;

    pub fn pack(&self) -> Vec<u8> {
        Packer::default()
            .u32(self.origin_valid)
            .floats(&self.origin)
            .u32(self.position_valid)
            .floats(&self.position)
            .u32(self.eye_valid)
            .floats(&self.gaze_point_3d)
            .floats(&self.gaze_point_2d)
            .u32(self.eyeball_valid)
            .floats(&self.eyeball_centre)
            .u32(self.pupil_valid)
            .f32(self.pupil_diameter)
            .finish()
    }
}

impl GazeDataRecord {
    pub const SIZE: usize = 8 + 2 * EyeData::SIZE;

    pub fn pack(&self) -> Vec<u8> {
        Packer::default().i64(self.timestamp).bytes(&self.left.pack()).bytes(&self.right.pack()).finish()
    }
}

fn flag(condition: bool) -> u32 {
    u32::from(condition)
}

fn unit_clamp(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

pub fn gaze_point_record(g: &DeviceGaze) -> PointRecord {
    PointRecord { timestamp: g.timestamp, valid: flag(g.combined_validity != 0), point: g.combined_point }
}

pub fn responsive_gaze_point_record(g: &DeviceGaze) -> PointRecord {
    PointRecord { timestamp: g.timestamp, valid: flag(g.responsive_validity != 0), point: g.responsive_point }
}

pub fn eye_openness_record(g: &DeviceGaze) -> OpennessRecord {
    OpennessRecord {
        timestamp: g.timestamp,
        left_valid: flag(g.left_openness_validity != 0),
        left: g.left_openness,
        right_valid: flag(g.right_openness_validity != 0),
        right: g.right_openness,
    }
}

pub fn gaze_origin_record(g: &DeviceGaze) -> PairRecord {
    PairRecord {
        timestamp: g.timestamp,
        left_valid: flag(g.left_origin_display_validity == 1),
        left: g.left_origin_display,
        right_valid: flag(g.right_origin_display_validity == 1),
        right: g.right_origin_display,
    }
}

struct EyeColumns {
    origin_valid: u32,
    origin: Vec3,
    position_valid: u32,
    position: Vec3,
    validity: u32,
    point_3d: Vec3,
    point_2d: Vec2,
    eyeball_present: u32,
    eyeball: Vec3,
    pupil: f64,
}

fn eye_data(eye: EyeColumns) -> EyeData {
    let valid = flag(eye.validity < 2);
    EyeData {
        origin_valid: flag(eye.origin_valid == 1),
        origin: eye.origin,
        position_valid: flag(eye.position_valid == 1),
        position: eye.position,
        eye_valid: valid,
        gaze_point_3d: eye.point_3d,
        gaze_point_2d: eye.point_2d,
        eyeball_valid: if eye.eyeball_present == 1 { valid } else { 0 },
        eyeball_centre: eye.eyeball,
        pupil_valid: valid,
        pupil_diameter: eye.pupil,
    }
}

pub fn gaze_data_record(g: &DeviceGaze) -> GazeDataRecord {
    let left = eye_data(EyeColumns {
        origin_valid: g.left_origin_tracker_validity,
        origin: g.left_origin_tracker,
        position_valid: g.left_position_relative_validity,
        position: g.left_position_relative,
        validity: g.left_validity,
        point_3d: g.left_gaze_point_3d,
        point_2d: g.left_gaze_point_2d,
        eyeball_present: g.left_eyeball_present,
        eyeball: g.left_eyeball_centre,
        pupil: g.left_pupil_diameter,
    });
    let right = eye_data(EyeColumns {
        origin_valid: g.right_origin_tracker_validity,
        origin: g.right_origin_tracker,
        position_valid: g.right_position_relative_validity,
        position: g.right_position_relative,
        validity: g.right_validity,
        point_3d: g.right_gaze_point_3d,
        point_2d: g.right_gaze_point_2d,
        eyeball_present: g.right_eyeball_present,
        eyeball: g.right_eyeball_centre,
        pupil: g.right_pupil_diameter,
    });
    GazeDataRecord { timestamp: g.timestamp, left, right }
}

pub fn user_position_guide_record(g: &DeviceGaze) -> PairRecord {
    PairRecord {
        timestamp: g.timestamp,
        left_valid: flag(g.left_position_guide_validity == 1),
        left: g.left_position_guide.map(unit_clamp),
        right_valid: flag(g.right_position_guide_validity == 1),
        right: g.right_position_guide.map(unit_clamp),
    }
}

/// Everything the runtime publishes for one device gaze message.
#[derive(Debug, Clone, PartialEq)]
pub struct GazeFrame {
    pub device: DeviceGaze,
    pub gaze_point: PointRecord,
    pub responsive_gaze_point: PointRecord,
    pub gaze_origin: PairRecord,
    pub gaze_data: GazeDataRecord,
    pub eye_openness: OpennessRecord,
    pub user_position_guide: PairRecord,
}

impl GazeFrame {
    pub fn new(device: DeviceGaze) -> Self {
        Self {
            gaze_point: gaze_point_record(&device),
            responsive_gaze_point: responsive_gaze_point_record(&device),
            gaze_origin: gaze_origin_record(&device),
            gaze_data: gaze_data_record(&device),
            eye_openness: eye_openness_record(&device),
            user_position_guide: user_position_guide_record(&device),
            device,
        }
    }

    pub fn timestamp_us(&self) -> i64 {
        self.device.timestamp
    }

    /// Same outputs restamped, for moving device time onto the service clock.
    pub fn with_timestamp(&self, timestamp_us: i64) -> Self {
        Self::new(DeviceGaze { timestamp: timestamp_us, ..self.device.clone() })
    }
}

pub const PLATMOD_RECORD_NAMES: [&str; 6] =
    ["gaze_point", "responsive_gaze_point", "eye_openness", "gaze_origin", "gaze_data", "user_position_guide"];

/// Packed records receive_gaze_data hands to each subscriber, in publication order.
///
/// With the host gaze setting (settings +0x188) the device's gaze columns are ignored and
/// only the user position guide is published; the ET5 runs gaze on the device, so the
/// runtime takes the other branch.
pub fn platmod_records(g: &DeviceGaze, host_gaze: bool) -> Vec<(&'static str, Vec<u8>)> {
    let mut records = Vec::new();
    if !host_gaze {
        records.push(("gaze_point", gaze_point_record(g).pack()));
        records.push(("responsive_gaze_point", responsive_gaze_point_record(g).pack()));
        records.push(("eye_openness", eye_openness_record(g).pack()));
        records.push(("gaze_origin", gaze_origin_record(g).pack()));
        records.push(("gaze_data", gaze_data_record(g).pack()));
    }
    records.push(("user_position_guide", user_position_guide_record(g).pack()));
    records
}

/// Little-endian reader for packed platmod records.
#[derive(Debug)]
pub struct Unpacker<'a>(&'a [u8]);

impl<'a> Unpacker<'a> {
    pub fn new(data: &'a [u8], size: usize) -> Result<Self> {
        if data.len() < size {
            return fail("Packed record is too short");
        }
        Ok(Self(data))
    }

    fn take<const N: usize>(&mut self) -> [u8; N] {
        let (head, rest) = self.0.split_at(N);
        self.0 = rest;
        head.try_into().expect("length checked")
    }

    pub fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take())
    }

    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }

    pub fn f32(&mut self) -> f64 {
        f64::from(f32::from_le_bytes(self.take()))
    }

    pub fn vec2(&mut self) -> Vec2 {
        [self.f32(), self.f32()]
    }

    pub fn vec3(&mut self) -> Vec3 {
        [self.f32(), self.f32(), self.f32()]
    }
}

impl PointRecord {
    pub fn unpack(data: &[u8]) -> Result<Self> {
        let mut r = Unpacker::new(data, Self::SIZE)?;
        Ok(Self { timestamp: r.i64(), valid: r.u32(), point: r.vec2() })
    }
}

impl PairRecord {
    pub fn unpack(data: &[u8]) -> Result<Self> {
        let mut r = Unpacker::new(data, Self::SIZE)?;
        Ok(Self { timestamp: r.i64(), left_valid: r.u32(), left: r.vec3(), right_valid: r.u32(), right: r.vec3() })
    }
}

impl OpennessRecord {
    pub fn unpack(data: &[u8]) -> Result<Self> {
        let mut r = Unpacker::new(data, Self::SIZE)?;
        Ok(Self { timestamp: r.i64(), left_valid: r.u32(), left: r.f32(), right_valid: r.u32(), right: r.f32() })
    }
}

impl EyeData {
    fn read(r: &mut Unpacker) -> Self {
        Self {
            origin_valid: r.u32(),
            origin: r.vec3(),
            position_valid: r.u32(),
            position: r.vec3(),
            eye_valid: r.u32(),
            gaze_point_3d: r.vec3(),
            gaze_point_2d: r.vec2(),
            eyeball_valid: r.u32(),
            eyeball_centre: r.vec3(),
            pupil_valid: r.u32(),
            pupil_diameter: r.f32(),
        }
    }

    pub fn unpack(data: &[u8]) -> Result<Self> {
        Ok(Self::read(&mut Unpacker::new(data, Self::SIZE)?))
    }
}

impl GazeDataRecord {
    pub fn unpack(data: &[u8]) -> Result<Self> {
        let mut r = Unpacker::new(data, Self::SIZE)?;
        Ok(Self { timestamp: r.i64(), left: EyeData::read(&mut r), right: EyeData::read(&mut r) })
    }
}

/// receive_presence_data 0x140266b60: device values 0, 1 and 2 pass, anything else becomes 0.
pub fn platmod_presence(device_value: i64) -> i64 {
    if (0..=2).contains(&device_value) { device_value } else { 0 }
}

/// Service callback 0x1400fa2b0: the property 0xB value the client maps to away or present.
pub fn prp_presence(platmod_value: i64) -> u32 {
    match platmod_value {
        1 => PRESENCE_AWAY,
        2 => PRESENCE_PRESENT,
        _ => PRESENCE_UNKNOWN,
    }
}

/// One user presence record: device timestamp and the device's presence value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presence {
    pub timestamp_us: i64,
    pub value: i32,
}

/// One 0x504 message, or `None` when the runtime drops it.
///
/// parse_presence 0x1402cba80 reads column 1 as int64 and column 2 as int32 (item tag 1,
/// which the ET5 sends) and skips other columns; a wrong type fails the parse. The
/// dispatcher 0x1402c2700 (index 5) leaves absent columns zero.
pub fn presence_message(payload: &[u8]) -> Result<Option<Presence>> {
    let mut presence = Presence { timestamp_us: 0, value: 0 };
    for column in stream_columns(payload)? {
        let expected = match column.column_id {
            1 => ParsedType::Int64,
            2 => ParsedType::Int32,
            _ => continue,
        };
        match parsed_value(&column.value)? {
            Some(Parsed::Int64(timestamp)) if expected == ParsedType::Int64 => presence.timestamp_us = timestamp,
            Some(Parsed::Int32(value)) if expected == ParsedType::Int32 => presence.value = value,
            _ => return Ok(None),
        }
    }
    Ok(Some(presence))
}

/// Builders for synthetic messages (tests and oracle edge cases).
pub mod build {
    use super::*;

    fn field(tag: u8, data: &[u8]) -> Field {
        Field::new(tag, data.to_vec())
    }

    pub fn uint32_column(value: u32) -> Vec<Field> {
        vec![field(wire::UINT32, &value.to_be_bytes())]
    }

    pub fn int32_column(value: i32) -> Vec<Field> {
        vec![field(wire::INT32, &value.to_be_bytes())]
    }

    pub fn int64_column(value: i64) -> Vec<Field> {
        vec![field(wire::INT64, &value.to_be_bytes())]
    }

    pub fn fixed16_column(value: f64) -> Result<Vec<Field>> {
        let scaled = round_f32(value) * 65536.0;
        match crate::numeric::truncate_in_range(scaled, -2_147_483_648.0, 2_147_483_648.0) {
            Some(raw) => Ok(vec![field(wire::FIXED16, &(raw as i32).to_be_bytes())]),
            None => fail("Value does not fit the fixed-point wire type"),
        }
    }

    fn fixed42(value: f64) -> Result<Field> {
        Ok(field(wire::FIXED42, &crate::fields::fixed42_raw(value)?.to_be_bytes()))
    }

    fn vector(shape: u32, values: &[f64]) -> Result<Vec<Field>> {
        let mut items = vec![field(wire::STRUCT, &((values.len() as u32) << 16 | shape).to_be_bytes())];
        for value in values {
            items.push(fixed42(*value)?);
        }
        Ok(items)
    }

    pub fn vec2_column(values: Vec2) -> Result<Vec<Field>> {
        vector(VEC2_STRUCT, &values)
    }

    pub fn vec3_column(values: Vec3) -> Result<Vec<Field>> {
        vector(VEC3_STRUCT, &values)
    }

    /// Column set payload in the given order.
    pub fn columns_payload(columns: &[(u32, Vec<Field>)]) -> Result<Vec<u8>> {
        let mut items = vec![field(wire::STRUCT, &((columns.len() as u32) << 16 | FRAME_STRUCT).to_be_bytes())];
        for (column_id, value) in columns {
            items.push(field(wire::STRUCT, &(2 << 16 | COLUMN_STRUCT).to_be_bytes()));
            items.push(field(wire::UINT32, &column_id.to_be_bytes()));
            items.extend(value.iter().cloned());
        }
        encode_payload(&items)
    }

    pub fn stream_message(stream: Stream, columns: &[(u32, Vec<Field>)]) -> Result<Message> {
        Ok(Message::new(STREAM_DATA_KIND, 0, 1, stream.wire_id(), 0, columns_payload(columns)?))
    }
}

#[cfg(test)]
mod tests;
