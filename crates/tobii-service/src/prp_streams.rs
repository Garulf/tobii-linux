//! PRP stream records of the runtime's gaze outputs and the game DLL's bodies for them.
//!
//! The service's stream callbacks (0x1400fc170, 0x1400fcdd0, 0x1400fc040, 0x1400faf30 with
//! 0x1400f9f00, 0x1400fae20, 0x1400fd0f0) copy each platmod record into the native stream
//! data the PRP serializer reads, keeping a validity only when it equals 1. The game DLL's
//! serializer (0x180052e60, packets 0x18004f730) then writes the stream bodies.
//! tools/gaze_oracle.py runs both; docs/gaze-host.md has the evidence.

use tobii_protocol::gaze::{EyeData, GazeDataRecord, OpennessRecord, Packer, PairRecord, PointRecord};

use crate::error::{Result, ServiceError};
use crate::prp::{GazePoint, encode_gaze_body, le_f32, push_u32, timestamp_words};

pub const OPERATION_STREAM: u32 = 1;
pub const OPERATION_COMPOUND_STREAM: u32 = 0xB;
pub const PRP_GAZE_POINT: u32 = 3;
pub const PRP_GAZE_ORIGIN: u32 = 4;
pub const PRP_GAZE_DATA: u32 = 6;
pub const PRP_RESPONSIVE_GAZE_POINT: u32 = 0x1D;
pub const PRP_EYE_OPENNESS: u32 = 0x21;
pub const PRP_COMPOUND_USER_POSITION_GUIDE: u32 = 2;

const POINT_NATIVE: usize = 20;
const PAIR_NATIVE: usize = 40;
const EYE_NATIVE: usize = 80;
const GAZE_DATA_NATIVE: usize = 8 + 2 * EYE_NATIVE;
pub const GAZE_DATA_BODY: u32 = 0xA4;
pub const GAZE_DATA_BODY_EXTENDED: u32 = 0xAC;

/// One platmod record by the name receive_gaze_data publishes it under.
#[derive(Debug, Clone, PartialEq)]
pub enum PlatmodRecord {
    GazePoint(PointRecord),
    ResponsiveGazePoint(PointRecord),
    GazeOrigin(PairRecord),
    GazeData(Box<GazeDataRecord>),
    EyeOpenness(OpennessRecord),
    UserPositionGuide(PairRecord),
}

/// Operation, stream id and the native stream data the PRP serializer reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrpRecord {
    pub kind: u32,
    pub stream: u32,
    pub native: Vec<u8>,
}

fn flag(value: u32) -> u32 {
    u32::from(value == 1)
}

fn pair_native(r: &PairRecord) -> Vec<u8> {
    Packer::default()
        .i64(r.timestamp)
        .u32(flag(r.left_valid))
        .floats(&r.left)
        .u32(flag(r.right_valid))
        .floats(&r.right)
        .finish()
}

fn eye_native(packer: Packer, e: &EyeData) -> Packer {
    packer
        .u32(flag(e.origin_valid))
        .floats(&e.origin)
        .u32(flag(e.position_valid))
        .floats(&e.position)
        .u32(flag(e.eye_valid))
        .floats(&e.gaze_point_3d)
        .floats(&e.gaze_point_2d)
        .u32(flag(e.eyeball_valid))
        .floats(&e.eyeball_centre)
        .u32(flag(e.pupil_valid))
        .f32(e.pupil_diameter)
}

/// The service callback's stream data for one platmod record.
pub fn prp_record(record: &PlatmodRecord) -> PrpRecord {
    let point = |r: &PointRecord| Packer::default().i64(r.timestamp).u32(flag(r.valid)).floats(&r.point).finish();
    let (kind, stream, native) = match record {
        PlatmodRecord::GazePoint(r) => (OPERATION_STREAM, PRP_GAZE_POINT, point(r)),
        PlatmodRecord::ResponsiveGazePoint(r) => (OPERATION_STREAM, PRP_RESPONSIVE_GAZE_POINT, point(r)),
        PlatmodRecord::GazeOrigin(r) => (OPERATION_STREAM, PRP_GAZE_ORIGIN, pair_native(r)),
        PlatmodRecord::GazeData(r) => {
            let packer = eye_native(Packer::default().i64(r.timestamp), &r.left);
            (OPERATION_STREAM, PRP_GAZE_DATA, eye_native(packer, &r.right).finish())
        }
        PlatmodRecord::EyeOpenness(r) => (
            OPERATION_STREAM,
            PRP_EYE_OPENNESS,
            Packer::default()
                .i64(r.timestamp)
                .u32(flag(r.left_valid))
                .f32(r.left)
                .u32(flag(r.right_valid))
                .f32(r.right)
                .finish(),
        ),
        PlatmodRecord::UserPositionGuide(r) => {
            (OPERATION_COMPOUND_STREAM, PRP_COMPOUND_USER_POSITION_GUIDE, pair_native(r))
        }
    };
    PrpRecord { kind, stream, native }
}

fn native_layout(native: &[u8], size: usize) -> Result<()> {
    if native.len() == size {
        return Ok(());
    }
    Err(ServiceError::Truncated(format!("unpack requires a buffer of {size} bytes")))
}

fn native_timestamp(native: &[u8]) -> i64 {
    i64::from_le_bytes(native[..8].try_into().expect("eight bytes"))
}

/// A validity byte of the native layout (`B3x`): the low byte of its word.
fn native_byte(native: &[u8], offset: usize) -> u32 {
    u32::from(native[offset])
}

fn body_start(size: u32, timestamp: i64) -> Vec<u8> {
    let mut body = Vec::with_capacity(size as usize);
    let (high, low) = timestamp_words(timestamp);
    push_u32(&mut body, size);
    push_u32(&mut body, high);
    push_u32(&mut body, low);
    body
}

/// Stream 4 (gaze origin) and compound stream 2 (user position guide) share this body.
pub fn pair_body(native: &[u8]) -> Result<Vec<u8>> {
    native_layout(native, PAIR_NATIVE)?;
    let mut body = body_start(44, native_timestamp(native));
    push_u32(&mut body, native_byte(native, 8));
    body.extend_from_slice(&native[12..24]);
    push_u32(&mut body, native_byte(native, 24));
    body.extend_from_slice(&native[28..40]);
    Ok(body)
}

/// The words of one eye and its position validity, which only the extended body carries.
fn eye_words(body: &mut Vec<u8>, eye: &[u8]) -> u32 {
    push_u32(body, native_byte(eye, 0));
    body.extend_from_slice(&eye[4..16]);
    body.extend_from_slice(&eye[20..32]);
    push_u32(body, native_byte(eye, 32));
    body.extend_from_slice(&eye[36..56]);
    push_u32(body, native_byte(eye, 56));
    body.extend_from_slice(&eye[60..72]);
    push_u32(body, native_byte(eye, 72));
    body.extend_from_slice(&eye[76..80]);
    native_byte(eye, 16)
}

/// Stream 6: 0xa4 bytes, or 0xac with the two position validities a newer peer reads.
pub fn gaze_data_body(native: &[u8], extended: bool) -> Result<Vec<u8>> {
    native_layout(native, GAZE_DATA_NATIVE)?;
    let size = if extended { GAZE_DATA_BODY_EXTENDED } else { GAZE_DATA_BODY };
    let mut body = body_start(size, native_timestamp(native));
    let left = eye_words(&mut body, &native[8..8 + EYE_NATIVE]);
    let right = eye_words(&mut body, &native[8 + EYE_NATIVE..]);
    if extended {
        push_u32(&mut body, left);
        push_u32(&mut body, right);
    }
    Ok(body)
}

/// Body the game DLL's serializer writes for this record, `None` where it has no case.
pub fn stream_body(record: &PrpRecord, extended: bool) -> Result<Option<Vec<u8>>> {
    let native = record.native.as_slice();
    if record.kind == OPERATION_COMPOUND_STREAM {
        return match record.stream {
            PRP_COMPOUND_USER_POSITION_GUIDE => pair_body(native).map(Some),
            _ => Ok(None),
        };
    }
    match record.stream {
        PRP_GAZE_POINT | PRP_RESPONSIVE_GAZE_POINT => {
            native_layout(native, POINT_NATIVE)?;
            let point = GazePoint {
                timestamp: native_timestamp(native),
                valid: native_byte(native, 8) != 0,
                x: le_f32(native, 12),
                y: le_f32(native, 16),
            };
            Ok(Some(encode_gaze_body(&point)))
        }
        PRP_GAZE_ORIGIN => pair_body(native).map(Some),
        PRP_GAZE_DATA => gaze_data_body(native, extended).map(Some),
        _ => Ok(None),
    }
}

/// Stream package with empty extensions, the layout the real service sends: operation 1
/// for streams, 11 for compound streams. `None` where the serializer has no case.
pub fn stream_packet(record: &PrpRecord, message_tag: u32, extended: bool) -> Result<Option<Vec<u8>>> {
    Ok(stream_body(record, extended)?
        .map(|body| crate::prp::stream_package(record.kind, message_tag, record.stream, &body)))
}

#[cfg(test)]
mod tests;
