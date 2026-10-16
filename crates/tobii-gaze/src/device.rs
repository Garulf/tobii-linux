//! Device-side gaze configuration: calibration commands, persistent files and start checks.
//!
//! Recovered from the platform runtime's tracker vtable 0x1410af508 and the castor platform
//! module that drives it; docs/gaze-device.md has the command table and evidence. Every
//! encoder here is compared with the original builder in emulation by
//! tools/gaze_device_oracle.py.
//!
//! The runtime keeps the user calibration on the device. At start it reads the device
//! calibration id (status key 7) and compares it with the copy in calibration.setpm; it
//! applies the local copy (1110) only when they differ.

use tobii_protocol::commands::{blob_field, command, header_only_request, request, string_field};
use tobii_protocol::fields::{
    StructHeader, Value, array_fields, fixed16_field, fixed42_field, int32_field, struct_field, triple_fixed42_fields,
    uint32_field,
};
use tobii_protocol::{Field, Message, ProtocolError};

use crate::error::{GazeError, Result};
use crate::math::float32;
use crate::operation::{External, Io, OperationResult};

/// Eye selector shared by enabled-eye, per-eye calibration and 1011 (0x1402b5290).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Eye {
    Left = 1,
    Right = 2,
    Both = 3,
}

impl Eye {
    pub fn from_u32(value: u32) -> Option<Eye> {
        match value {
            1 => Some(Eye::Left),
            2 => Some(Eye::Right),
            3 => Some(Eye::Both),
            _ => None,
        }
    }
}

pub mod gaze_command {
    pub const CALIBRATION_START: u32 = 1010;
    pub const CALIBRATION_START_EYE: u32 = 1011;
    pub const CALIBRATION_STOP: u32 = 1020;
    pub const CALIBRATION_COLLECT_2D: u32 = 1030;
    pub const CALIBRATION_COLLECT_3D: u32 = 1031;
    pub const CALIBRATION_COLLECT_PER_EYE_2D: u32 = 1032;
    pub const CALIBRATION_CLEAR: u32 = 1060;
    pub const CALIBRATION_COMPUTE_AND_APPLY: u32 = 1070;
    pub const CALIBRATION_COMPUTE_AND_APPLY_PER_EYE: u32 = 1071;
    pub const CALIBRATION_DISCARD_2D: u32 = 1080;
    pub const CALIBRATION_DISCARD_3D: u32 = 1081;
    pub const CALIBRATION_RETRIEVE: u32 = 1100;
    pub const CALIBRATION_APPLY: u32 = 1110;
    pub const GET_CALIBRATION_STIMULUS_POINTS: u32 = 1120;
    pub const GET_CONFIGURATION_KEY: u32 = 1310;
    pub const SET_DISPLAY_AREA: u32 = 1440;
    pub const ENABLE_EXTENSION: u32 = 1800;
    pub const PERSISTENT_FILE_ENUMERATE: u32 = 3060;
    pub const PERSISTENT_FILE_WRITE: u32 = 3070;
    pub const PERSISTENT_FILE_READ: u32 = 3080;
    pub const PERSISTENT_FILE_ERASE: u32 = 3090;
    pub const SET_ENABLED_EYE: u32 = 3160;
}

use gaze_command::*;

/// Gaze commands as (name, value), in declaration order.
pub const GAZE_COMMANDS: [(&str, u32); 22] = [
    ("CALIBRATION_START", CALIBRATION_START),
    ("CALIBRATION_START_EYE", CALIBRATION_START_EYE),
    ("CALIBRATION_STOP", CALIBRATION_STOP),
    ("CALIBRATION_COLLECT_2D", CALIBRATION_COLLECT_2D),
    ("CALIBRATION_COLLECT_3D", CALIBRATION_COLLECT_3D),
    ("CALIBRATION_COLLECT_PER_EYE_2D", CALIBRATION_COLLECT_PER_EYE_2D),
    ("CALIBRATION_CLEAR", CALIBRATION_CLEAR),
    ("CALIBRATION_COMPUTE_AND_APPLY", CALIBRATION_COMPUTE_AND_APPLY),
    ("CALIBRATION_COMPUTE_AND_APPLY_PER_EYE", CALIBRATION_COMPUTE_AND_APPLY_PER_EYE),
    ("CALIBRATION_DISCARD_2D", CALIBRATION_DISCARD_2D),
    ("CALIBRATION_DISCARD_3D", CALIBRATION_DISCARD_3D),
    ("CALIBRATION_RETRIEVE", CALIBRATION_RETRIEVE),
    ("CALIBRATION_APPLY", CALIBRATION_APPLY),
    ("GET_CALIBRATION_STIMULUS_POINTS", GET_CALIBRATION_STIMULUS_POINTS),
    ("GET_CONFIGURATION_KEY", GET_CONFIGURATION_KEY),
    ("SET_DISPLAY_AREA", SET_DISPLAY_AREA),
    ("ENABLE_EXTENSION", ENABLE_EXTENSION),
    ("PERSISTENT_FILE_ENUMERATE", PERSISTENT_FILE_ENUMERATE),
    ("PERSISTENT_FILE_WRITE", PERSISTENT_FILE_WRITE),
    ("PERSISTENT_FILE_READ", PERSISTENT_FILE_READ),
    ("PERSISTENT_FILE_ERASE", PERSISTENT_FILE_ERASE),
    ("SET_ENABLED_EYE", SET_ENABLED_EYE),
];

pub const HEADER_ONLY: [u32; 8] = [
    CALIBRATION_START,
    CALIBRATION_STOP,
    CALIBRATION_CLEAR,
    CALIBRATION_COMPUTE_AND_APPLY,
    CALIBRATION_COMPUTE_AND_APPLY_PER_EYE,
    CALIBRATION_RETRIEVE,
    GET_CALIBRATION_STIMULUS_POINTS,
    PERSISTENT_FILE_ENUMERATE,
];

pub const READ_ONLY: [u32; 5] = [
    CALIBRATION_RETRIEVE,
    GET_CALIBRATION_STIMULUS_POINTS,
    GET_CONFIGURATION_KEY,
    PERSISTENT_FILE_ENUMERATE,
    PERSISTENT_FILE_READ,
];

pub const DISPLAY_AREA_TRAILER: u32 = 0x3039;
pub const STATUS_STRUCT: u32 = 10000;
pub const STATUS_CALIBRATION_ID: i64 = 7;
pub const SETTING_FILE_VERSION: u32 = 4;
pub const SETTING_FILE_HEADER_SIZE: usize = 12;
pub const STIMULUS_POINT_STRUCT: u32 = 13200;
pub const STIMULUS_POSITION_STRUCT: u32 = 8002;
pub const STIMULUS_POINT_ITEMS: u32 = 7;
pub const CALIBRATION_RECORD_SIZE: usize = 40;

fn le_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().expect("four bytes"))
}

fn le_i32(data: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(data[at..at + 4].try_into().expect("four bytes"))
}

fn le_f32(data: &[u8], at: usize) -> f64 {
    f64::from(f32::from_le_bytes(data[at..at + 4].try_into().expect("four bytes")))
}

/// Packs a double as float32 like `struct.pack('<f')`, refusing values beyond its range.
pub fn pack_f32(value: f64) -> Result<[u8; 4]> {
    let narrowed = value as f32;
    if narrowed.is_infinite() && value.is_finite() {
        return Err(GazeError::Overflow);
    }
    Ok(narrowed.to_le_bytes())
}

pub fn header_only(request_id: u32, command: u32) -> Result<Message, ProtocolError> {
    if !HEADER_ONLY.contains(&command) {
        return Err(ProtocolError::new("Command has fields; use its encoder"));
    }
    request(request_id, command, &[])
}

/// Command 1011 from vtable slot 0x1c0; no log name, so the name is inferred.
pub fn calibration_start_eye(request_id: u32, eye: Eye) -> Message {
    request(request_id, CALIBRATION_START_EYE, &[uint32_field(eye as u32)]).expect("small request")
}

fn point_request(request_id: u32, command: u32, coordinates: &[f64], eye: Eye) -> Result<Message, ProtocolError> {
    let mut fields = coordinates.iter().map(|&c| fixed42_field(float32(c))).collect::<Result<Vec<Field>, _>>()?;
    fields.push(uint32_field(eye as u32));
    request(request_id, command, &fields)
}

/// 1030; the tracker (0x1402b42c0) always passes eye 3.
pub fn calibration_collect_2d(request_id: u32, x: f64, y: f64, eye: Eye) -> Result<Message, ProtocolError> {
    point_request(request_id, CALIBRATION_COLLECT_2D, &[x, y], eye)
}

pub fn calibration_collect_3d(request_id: u32, point: [f64; 3], eye: Eye) -> Result<Message, ProtocolError> {
    point_request(request_id, CALIBRATION_COLLECT_3D, &point, eye)
}

pub fn calibration_collect_per_eye_2d(request_id: u32, x: f64, y: f64, eye: Eye) -> Result<Message, ProtocolError> {
    point_request(request_id, CALIBRATION_COLLECT_PER_EYE_2D, &[x, y], eye)
}

pub fn calibration_discard_2d(request_id: u32, x: f64, y: f64, eye: Eye) -> Result<Message, ProtocolError> {
    point_request(request_id, CALIBRATION_DISCARD_2D, &[x, y], eye)
}

pub fn calibration_discard_3d(request_id: u32, point: [f64; 3], eye: Eye) -> Result<Message, ProtocolError> {
    point_request(request_id, CALIBRATION_DISCARD_3D, &point, eye)
}

/// 1110 with the opaque blob that 1100 returned (0x1402c5050).
pub fn calibration_apply(request_id: u32, calibration: &[u8]) -> Result<Message, ProtocolError> {
    request(request_id, CALIBRATION_APPLY, &[blob_field(calibration)])
}

/// 1440 (0x1402c6f40): three float32 points as fixed42 triples, then an empty array of type 0x3039.
pub fn set_display_area(
    request_id: u32,
    top_left: [f64; 3],
    top_right: [f64; 3],
    bottom_left: [f64; 3],
) -> Result<Message, ProtocolError> {
    let mut fields = Vec::new();
    for point in [top_left, top_right, bottom_left] {
        fields.extend(triple_fixed42_fields(point.map(float32))?);
    }
    fields.extend(array_fields(0, DISPLAY_AREA_TRAILER)?);
    request(request_id, SET_DISPLAY_AREA, &fields)
}

pub fn set_enabled_eye(request_id: u32, eye: Eye) -> Message {
    request(request_id, SET_ENABLED_EYE, &[uint32_field(eye as u32)]).expect("small request")
}

/// 1800 from vtable slot 0x2e0; no log name, the name is inferred from 1810/1820.
pub fn enable_extension(request_id: u32, extension: u32) -> Message {
    request(request_id, ENABLE_EXTENSION, &[uint32_field(extension)]).expect("small request")
}

pub fn get_configuration_key(request_id: u32, name: &str) -> Result<Message, ProtocolError> {
    request(request_id, GET_CONFIGURATION_KEY, &[string_field(name)?])
}

pub fn persistent_file_read(request_id: u32, name: &str) -> Result<Message, ProtocolError> {
    request(request_id, PERSISTENT_FILE_READ, &[string_field(name)?])
}

pub fn persistent_file_write(request_id: u32, name: &str, data: &[u8]) -> Result<Message, ProtocolError> {
    request(request_id, PERSISTENT_FILE_WRITE, &[string_field(name)?, blob_field(data)])
}

pub fn persistent_file_erase(request_id: u32, name: &str) -> Result<Message, ProtocolError> {
    request(request_id, PERSISTENT_FILE_ERASE, &[string_field(name)?])
}

/// Key/value pairs of a 1490 (or 1330) reply, in reply order; a repeated key replaces the
/// earlier value.
pub fn status_entries(values: &[Value]) -> Result<Vec<(i64, Vec<u8>)>, ProtocolError> {
    let header = Value::Struct(StructHeader { item_count: 2, struct_id: STATUS_STRUCT });
    let mut entries: Vec<(i64, Vec<u8>)> = Vec::new();
    let mut index = 0;
    while index < values.len() {
        if values[index] != header {
            return Err(ProtocolError::new("Expected a status entry header"));
        }
        if index + 2 >= values.len() {
            return Err(ProtocolError::new("Truncated status entry"));
        }
        let (Some(key), Value::Bytes(text)) = (values[index + 1].as_int(), &values[index + 2]) else {
            return Err(ProtocolError::new("Unexpected status entry"));
        };
        match entries.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = text.clone(),
            None => entries.push((key, text.clone())),
        }
        index += 3;
    }
    Ok(entries)
}

/// Calibration id from status key 7, parsed with strtoul as 0x1402b9360 does; `None` when absent.
pub fn device_calibration_id(values: &[Value]) -> Result<Option<u32>> {
    let entries = status_entries(values)?;
    let Some((_, text)) = entries.iter().find(|(key, _)| *key == STATUS_CALIBRATION_ID) else {
        return Ok(None);
    };
    if text.is_empty() {
        return Ok(None);
    }
    let text = std::str::from_utf8(text).map_err(|e| GazeError::Value(e.to_string()))?;
    let digits: String = text.trim().chars().filter(|&c| c != '_').collect();
    let number: i128 =
        digits.parse().map_err(|_| GazeError::Value(format!("invalid literal for int() with base 10: '{text}'")))?;
    Ok(Some((number & 0xFFFF_FFFF) as u32))
}

/// calibration.setpm as written by calibration_receive_and_commit (0x140267cf0).
///
/// The payload is the calibration id followed by the 1100 reply blob verbatim; 1110 sends
/// that blob back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCalibration {
    pub calibration_id: u32,
    pub blob: Vec<u8>,
}

/// Strips the 12-byte setting-file header: version 4, kind 1, payload size.
pub fn setting_file_payload(data: &[u8]) -> Result<&[u8], ProtocolError> {
    if data.len() < SETTING_FILE_HEADER_SIZE {
        return Err(ProtocolError::new("Setting file is shorter than its header"));
    }
    let (version, size) = (le_u32(data, 0), le_u32(data, 8));
    if version != SETTING_FILE_VERSION || size as usize != data.len() - SETTING_FILE_HEADER_SIZE {
        return Err(ProtocolError::new("Unexpected setting file version or size"));
    }
    Ok(&data[SETTING_FILE_HEADER_SIZE..])
}

pub fn setting_file(payload: &[u8], kind: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity(SETTING_FILE_HEADER_SIZE + payload.len());
    for word in [SETTING_FILE_VERSION, kind, payload.len() as u32] {
        data.extend_from_slice(&word.to_le_bytes());
    }
    data.extend_from_slice(payload);
    data
}

/// calibration.setpm bytes for a calibration, the inverse of [`parse_calibration_setting`].
pub fn calibration_setting(calibration: &LocalCalibration) -> Result<Vec<u8>, ProtocolError> {
    if calibration.blob.is_empty() {
        return Err(ProtocolError::new("Calibration holds no blob"));
    }
    let payload = [&calibration.calibration_id.to_le_bytes()[..], &calibration.blob].concat();
    Ok(setting_file(&payload, 1))
}

pub fn parse_calibration_setting(data: &[u8]) -> Result<LocalCalibration, ProtocolError> {
    let payload = setting_file_payload(data)?;
    if payload.len() <= 4 {
        return Err(ProtocolError::new("Calibration setting holds no blob"));
    }
    Ok(LocalCalibration { calibration_id: le_u32(payload, 0), blob: payload[4..].to_vec() })
}

/// screenplane.setpm: bottom-left, top-left, top-right corners in millimetres.
pub fn screen_plane_setting(data: &[u8]) -> Result<[[f64; 3]; 3], ProtocolError> {
    let payload = setting_file_payload(data)?;
    if payload.len() != 36 {
        return Err(ProtocolError::new("Screen plane setting must hold three points"));
    }
    Ok(std::array::from_fn(|point| std::array::from_fn(|axis| le_f32(payload, (point * 3 + axis) * 4))))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationState {
    InSync = 0,
    Applied = 1,
    ApplyNeeded = 2,
    DeviceOnly = 3,
}

fn status_request() -> Message {
    header_only_request(0, command::GET_DEVICE_STATUS).expect("1490 is header only")
}

/// start_synchronize_calibrations (0x140263010) without its destructive branch.
///
/// The runtime applies the local calibration when the ids differ, and clears the device
/// (1010 then 1020) when there is no local calibration; here the clear is never sent and
/// `DeviceOnly` is returned instead. Applying needs `allow_apply`.
pub async fn synchronize_calibration<E: External>(
    io: Io<E>,
    local: Option<LocalCalibration>,
    allow_apply: bool,
) -> OperationResult<CalibrationState, E> {
    let status = io.call(status_request(), None).await.map_err(crate::operation::OperationError::External)?;
    let device_id = device_calibration_id(&status)?.unwrap_or(0);
    let Some(local) = local.filter(|local| local.calibration_id != 0) else {
        return Ok(if device_id != 0 { CalibrationState::DeviceOnly } else { CalibrationState::InSync });
    };
    if device_id == local.calibration_id {
        return Ok(CalibrationState::InSync);
    }
    if !allow_apply {
        return Ok(CalibrationState::ApplyNeeded);
    }
    io.call(calibration_apply(0, &local.blob)?, None).await.map_err(crate::operation::OperationError::External)?;
    Ok(CalibrationState::Applied)
}

/// The single blob of a 1100 reply.
pub fn reply_blob(values: &[Value]) -> Option<Vec<u8>> {
    let mut blobs = values.iter().filter_map(|value| match value {
        Value::Bytes(bytes) => Some(bytes),
        _ => None,
    });
    match (blobs.next(), blobs.next()) {
        (Some(blob), None) => Some(blob.clone()),
        _ => None,
    }
}

/// Read-only 1100: the device's current calibration blob.
pub async fn retrieve_calibration<E: External>(io: Io<E>) -> OperationResult<Vec<u8>, E> {
    let request = header_only(0, CALIBRATION_RETRIEVE)?;
    let reply = io.call(request, None).await.map_err(crate::operation::OperationError::External)?;
    Ok(reply_blob(&reply).ok_or_else(|| ProtocolError::new("Calibration reply must hold one blob"))?)
}

/// tobii_calibration_stimulus_point_status_t and tobii_calibration_point_status_t.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointStatus {
    FailedOrInvalid = 0,
    ValidButNotUsed = 1,
    ValidAndUsed = 2,
}

impl PointStatus {
    /// 0x1402b7830 keeps 0, 1 and 2 and turns any other device status into 0.
    pub fn from_tracker(raw: i64) -> PointStatus {
        match raw {
            1 => PointStatus::ValidButNotUsed,
            2 => PointStatus::ValidAndUsed,
            _ => PointStatus::FailedOrInvalid,
        }
    }

    /// Stream Engine's blob status: -1 failed, 0 valid but unused, 1 valid and used.
    pub fn from_blob(raw: i32) -> PointStatus {
        match raw {
            0 => PointStatus::ValidButNotUsed,
            1 => PointStatus::ValidAndUsed,
            _ => PointStatus::FailedOrInvalid,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PointStatus::FailedOrInvalid => "FAILED_OR_INVALID",
            PointStatus::ValidButNotUsed => "VALID_BUT_NOT_USED",
            PointStatus::ValidAndUsed => "VALID_AND_USED",
        }
    }
}

/// Per-eye quality of a stimulus point; `status` is the raw value written to the wire,
/// a [`PointStatus`] once decoded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeQuality {
    pub status: i64,
    pub bias: f64,
    pub precision: f64,
}

/// One record of the 1120 reply (struct 13200), copied in wire order by 0x1402b7830.
///
/// Field names follow Stream Engine's tobii_calibration_stimulus_point_data_t. Bias is the
/// distance from the target to the mean calibrated gaze point and precision its spread,
/// both in normalized display units.
#[derive(Debug, Clone, PartialEq)]
pub struct StimulusPoint {
    pub point: [f64; 3],
    pub left: EyeQuality,
    pub right: EyeQuality,
}

fn number(value: &Value) -> Option<f64> {
    match *value {
        Value::Float(x) => Some(x),
        Value::Int(n) => Some(n as f64),
        Value::UInt(n) => Some(n as f64),
        _ => None,
    }
}

/// Decodes the reply values of 1120 (get_calibration_stimulus_pts).
pub fn stimulus_points(values: &[Value]) -> Result<Vec<StimulusPoint>, ProtocolError> {
    let Some(Value::Struct(array)) = values.first().filter(|_| values.len() >= 2) else {
        return Err(ProtocolError::new("Stimulus points reply must start with an array header"));
    };
    if array.struct_id != 0x100 {
        return Err(ProtocolError::new("Stimulus points reply must start with an array header"));
    }
    let count = i64::from(array.item_count) - 1;
    if values[1].as_int() != Some(STIMULUS_POINT_STRUCT.into()) || values.len() as i64 != 2 + count * 11 {
        return Err(ProtocolError::new("Unexpected stimulus points array"));
    }
    let point_header =
        Value::Struct(StructHeader { item_count: STIMULUS_POINT_ITEMS, struct_id: STIMULUS_POINT_STRUCT });
    let position_header = Value::Struct(StructHeader { item_count: 3, struct_id: STIMULUS_POSITION_STRUCT });
    let unexpected = || ProtocolError::new("Unexpected stimulus point record");
    values[2..]
        .chunks(11)
        .map(|record| {
            if record[0] != point_header || record[1] != position_header {
                return Err(unexpected());
            }
            let numbers: Vec<f64> = record[2..5].iter().map(number).collect::<Option<_>>().ok_or_else(unexpected)?;
            let eye = |at: usize| -> Result<EyeQuality, ProtocolError> {
                let status = record[at].as_int().ok_or_else(unexpected)?;
                let bias = number(&record[at + 1]).ok_or_else(unexpected)?;
                let precision = number(&record[at + 2]).ok_or_else(unexpected)?;
                Ok(EyeQuality { status: PointStatus::from_tracker(status) as i64, bias, precision })
            };
            Ok(StimulusPoint { point: [numbers[0], numbers[1], numbers[2]], left: eye(5)?, right: eye(8)? })
        })
        .collect()
}

/// Reply fields of 1120 for the given records, in the layout [`stimulus_points`] reads.
pub fn stimulus_points_payload(points: &[StimulusPoint]) -> Result<Vec<Field>, ProtocolError> {
    let mut fields = array_fields(points.len() as u32, STIMULUS_POINT_STRUCT)?.to_vec();
    for point in points {
        fields.push(struct_field(STIMULUS_POINT_ITEMS, STIMULUS_POINT_STRUCT)?);
        fields.push(struct_field(3, STIMULUS_POSITION_STRUCT)?);
        for &c in &point.point {
            fields.push(fixed16_field(c)?);
        }
        for eye in [point.left, point.right] {
            let status =
                i32::try_from(eye.status).map_err(|_| ProtocolError::new("Expected a signed 32-bit integer"))?;
            fields.extend([int32_field(status), fixed16_field(eye.bias)?, fixed16_field(eye.precision)?]);
        }
    }
    Ok(fields)
}

/// tobii_calibration_point_data_t as tobii_calibration_parse (0x18007e9b0) reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationPointData {
    pub point: [f64; 2],
    pub left_status: PointStatus,
    pub left_mapping: [f64; 2],
    pub right_status: PointStatus,
    pub right_mapping: [f64; 2],
}

/// The per-sample point table of a 1100 blob, read as Stream Engine's tobii_calibration_parse does.
///
/// Word 0 of the blob is the offset of a trailer whose second word is the record count;
/// each 40-byte record holds the target, then per eye the mapped gaze point and a status
/// (-1 failed, 0 valid but unused, 1 valid and used).
pub fn calibration_points(blob: &[u8]) -> Result<Vec<CalibrationPointData>, ProtocolError> {
    if blob.len() < 8 {
        return Err(ProtocolError::new("Calibration blob is too short"));
    }
    let trailer = i64::from(le_i32(blob, 0));
    if !(0..=blob.len() as i64 - 8).contains(&trailer) {
        return Err(ProtocolError::new("Calibration trailer offset is outside the blob"));
    }
    let trailer = trailer as usize;
    let count = i64::from(le_i32(blob, trailer + 4));
    let first = trailer + 8;
    if count < 0 || first as i64 + count * CALIBRATION_RECORD_SIZE as i64 > blob.len() as i64 {
        return Err(ProtocolError::new("Calibration point table does not fit the blob"));
    }
    Ok((0..count as usize)
        .map(|index| {
            let at = first + index * CALIBRATION_RECORD_SIZE;
            let f = |offset: usize| le_f32(blob, at + offset);
            CalibrationPointData {
                point: [f(0), f(4)],
                left_mapping: [f(8), f(12)],
                left_status: PointStatus::from_blob(le_i32(blob, at + 16)),
                right_mapping: [f(24), f(28)],
                right_status: PointStatus::from_blob(le_i32(blob, at + 32)),
            }
        })
        .collect())
}

/// One calibration record in blob layout, as Stream Engine writes it.
pub fn calibration_record(point: [f64; 2], left: ([f64; 2], i32), right: ([f64; 2], i32)) -> Result<Vec<u8>> {
    let mut record = Vec::with_capacity(CALIBRATION_RECORD_SIZE);
    for value in [point[0], point[1], left.0[0], left.0[1]] {
        record.extend(pack_f32(value)?);
    }
    record.extend(left.1.to_le_bytes());
    record.extend([0; 4]);
    for value in right.0 {
        record.extend(pack_f32(value)?);
    }
    record.extend(right.1.to_le_bytes());
    record.extend([0; 4]);
    Ok(record)
}

/// Word 5 of a 1100 blob; on albedo it equals status key 7 and the calibration.setpm id.
pub fn blob_calibration_id(blob: &[u8]) -> Result<u32, ProtocolError> {
    if blob.len() < 24 {
        return Err(ProtocolError::new("Calibration blob is too short"));
    }
    Ok(le_u32(blob, 20))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tobii_protocol::fields::{value, values};

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Bytes produced by the original builders in emulation (tools/gaze_device_oracle.py),
    /// from tests/test_gaze_device.py.
    #[test]
    fn original_builder_bytes() {
        let cases = [
            (header_only(1, CALIBRATION_RETRIEVE).unwrap(), "0000005100000001000000000000044c0000000000000000"),
            (
                calibration_collect_2d(7, 0.5, 0.5, Eye::Both).unwrap(),
                "00000051000000070000000000000406000000000000002500000400000008000002000000000004\
                 000000080000020000000000020000000400000003",
            ),
            (
                calibration_collect_3d(8, [0.0, 0.0, 600.0], Eye::Left).unwrap(),
                "0000005100000008000000000000040700000000000000320000040000000800000000000000000400\
                 000008000000000000000004000000080009600000000000020000000400000001",
            ),
            (
                calibration_start_eye(9, Eye::Right),
                "000000510000000900000000000003f3000000000000000b0000020000000400000002",
            ),
            (
                persistent_file_read(10, "a").unwrap(),
                "000000510000000a0000000000000c08000000000000000c000014000000050000000161",
            ),
            (
                calibration_apply(12, b"calibration").unwrap(),
                "000000510000000c000000000000045600000000000000160000150000000f0000000b63616c6962726174696f6e",
            ),
            (
                set_display_area(13, [0.0, 0.0, 0.0], [1.0, 2.0, 3.0], [-4.5, 5.25, -6.125]).unwrap(),
                "000000510000000d00000000000005a000000000000000a40000050000000400031f41040000000800\
                 000000000000000400000008000000000000000004000000080000000000000000050000000400031f\
                 410400000008000004000000000004000000080000080000000000040000000800000c000000000005\
                 0000000400031f410400000008ffffee0000000000040000000800001500000000000400000008ffff\
                 e78000000000050000000400010100020000000400003039",
            ),
        ];
        for (message, expected) in cases {
            assert_eq!(hex(&message.encode().unwrap()), expected.replace(char::is_whitespace, ""));
        }
    }

    #[test]
    fn header_only_rejects_commands_with_fields() {
        assert!(header_only(1, CALIBRATION_APPLY).is_err());
    }

    #[test]
    fn points_are_rounded_to_float32_before_fixed_point() {
        let exact = calibration_collect_2d(1, 0.1, 0.9, Eye::Both).unwrap();
        let rounded = calibration_collect_2d(1, float32(0.1), 0.9, Eye::Both).unwrap();
        assert_eq!(exact, rounded);
    }

    fn status_values(entries: &[(i64, &[u8])]) -> Vec<Value> {
        entries
            .iter()
            .flat_map(|(key, text)| {
                [
                    Value::Struct(StructHeader { item_count: 2, struct_id: STATUS_STRUCT }),
                    Value::Int(*key),
                    Value::Bytes(text.to_vec()),
                ]
            })
            .collect()
    }

    #[test]
    fn calibration_id_from_status_key_7() {
        let values = status_values(&[(0, b"false"), (4, b"HELLO_RGB"), (7, b"3157793335"), (8, b"false")]);
        assert_eq!(device_calibration_id(&values).unwrap(), Some(0xBC38_1A37));
        assert_eq!(device_calibration_id(&status_values(&[(0, b"false")])).unwrap(), None);
        let malformed = [Value::Struct(StructHeader { item_count: 2, struct_id: STATUS_STRUCT }), Value::Int(7)];
        assert!(status_entries(&malformed).is_err());
    }

    fn setting(payload: &[u8]) -> Vec<u8> {
        setting_file(payload, 1)
    }

    #[test]
    fn calibration_setting_round_trip_and_errors() {
        let file = setting(&[&0x1234u32.to_le_bytes()[..], b"blob bytes"].concat());
        let local = parse_calibration_setting(&file).unwrap();
        assert_eq!(local, LocalCalibration { calibration_id: 0x1234, blob: b"blob bytes".to_vec() });
        assert_eq!(calibration_setting(&local).unwrap(), file);
        let mut longer = setting(&[&1u32.to_le_bytes()[..], b"abc"].concat());
        longer.push(0);
        assert!(parse_calibration_setting(&longer).is_err());
        assert!(parse_calibration_setting(&setting(&1u32.to_le_bytes())).is_err());
    }

    #[test]
    fn screen_plane() {
        let corners = [[-296.5, 10.25, -3.0], [-296.5, 326.0, 111.75], [300.5, 326.0, 111.75]];
        let payload: Vec<u8> = corners.iter().flatten().flat_map(|&c| (c as f32).to_le_bytes()).collect();
        assert_eq!(screen_plane_setting(&setting(&payload)).unwrap(), corners);
    }

    #[test]
    fn calibration_points_statuses_and_bounds() {
        let mut blob = [4i32.to_le_bytes(), 0i32.to_le_bytes(), 3i32.to_le_bytes()].concat();
        for status in [-1, 0, 7] {
            blob.extend(calibration_record([0.5, 0.5], ([0.4, 0.6], status), ([0.6, 0.4], 1)).unwrap());
        }
        let points = calibration_points(&blob).unwrap();
        let statuses: Vec<PointStatus> = points.iter().map(|p| p.left_status).collect();
        assert_eq!(
            statuses,
            [PointStatus::FailedOrInvalid, PointStatus::ValidButNotUsed, PointStatus::FailedOrInvalid]
        );
        assert_eq!(points[0].left_mapping, [float32(0.4), float32(0.6)]);
        let short = [4i32.to_le_bytes(), 0i32.to_le_bytes(), 2i32.to_le_bytes()].concat();
        assert!(calibration_points(&[short, vec![0; 40]].concat()).is_err());
    }

    #[test]
    fn stimulus_points_round_trip_and_status_mapping() {
        let points = [StimulusPoint {
            point: [0.5, 0.25, 0.0],
            left: EyeQuality { status: 2, bias: 0.015625, precision: 0.03125 },
            right: EyeQuality { status: 5, bias: 0.5, precision: 0.25 },
        }];
        let fields = stimulus_points_payload(&points).unwrap();
        let decoded: Vec<Value> = fields.iter().map(|f| value(f).unwrap()).collect();
        let parsed = stimulus_points(&decoded).unwrap();
        assert_eq!(parsed[0].left, points[0].left);
        assert_eq!(parsed[0].right.status, 0);
        assert_eq!(parsed[0].point, [0.5, 0.25, 0.0]);
        let encoded = tobii_protocol::ttp::encode_payload(&fields).unwrap();
        assert_eq!(stimulus_points(&values(&encoded).unwrap()).unwrap(), parsed);
    }

    #[test]
    fn blob_id_is_word_five() {
        let blob =
            [[0u32; 5].iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>(), 0x1234u32.to_le_bytes().to_vec()]
                .concat();
        assert_eq!(blob_calibration_id(&blob).unwrap(), 0x1234);
        assert!(blob_calibration_id(&blob[..23]).is_err());
    }
}
