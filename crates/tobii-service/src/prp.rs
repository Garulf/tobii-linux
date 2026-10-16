//! PRP envelopes, the gaze and head-pose stream bodies and non-compound stream packets of
//! the bundled game DLL's serializer.
//!
//! These functions do not implement the service handshake, subscription commands or
//! transport (see [`crate::prp_session`]). Recovered from DLL SHA256
//! 35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020; docs/game-interface.md
//! has locations and limits, tools/prp_encoder_oracle.py runs the original serializer.
//!
//! Sample coordinates are float32 on the wire; they are kept as `f64` holding the value the
//! caller gave and narrowed once when packed, as `struct.pack("<f")` does.

use crate::error::{Result, prp};

pub const MAGIC: u32 = 0x5052_5054;
pub const HEAD_POSE: u32 = 2;
pub const GAZE_POINT: u32 = 3;
pub const RESPONSIVE_GAZE_POINT: u32 = 29;
/// An implementation resource limit, not a recovered vendor protocol maximum.
pub const DEFAULT_MAX_PAYLOAD: u64 = 16 * 1024 * 1024;
pub const HEADER_SIZE: usize = 12;
pub const GAZE_BODY_SIZE: usize = 24;
pub const HEAD_BODY_SIZE: usize = 52;

pub(crate) fn le_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().expect("four bytes"))
}

pub(crate) fn le_f32(data: &[u8], offset: usize) -> f64 {
    f64::from(f32::from_le_bytes(data[offset..offset + 4].try_into().expect("four bytes")))
}

pub(crate) fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_f32(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&(value as f32).to_le_bytes());
}

/// Wrap an already serialized, opaque PRP operation payload.
pub fn encode_envelope(payload: &[u8], max_payload: u64) -> Result<Vec<u8>> {
    let size = payload.len() as u64;
    if size > max_payload.min(0xFFFF_FFFF) {
        return prp("PRP payload exceeds the configured limit");
    }
    let size = size as u32;
    let mut packet = Vec::with_capacity(HEADER_SIZE + payload.len());
    push_u32(&mut packet, MAGIC);
    push_u32(&mut packet, size);
    push_u32(&mut packet, size ^ MAGIC);
    packet.extend_from_slice(payload);
    Ok(packet)
}

/// Validate one complete envelope; return its opaque payload.
pub fn decode_envelope(packet: &[u8], max_payload: u64) -> Result<&[u8]> {
    if packet.len() < HEADER_SIZE {
        return prp("truncated PRP envelope");
    }
    let (magic, size, check) = (le_u32(packet, 0), le_u32(packet, 4), le_u32(packet, 8));
    if magic != MAGIC || check != size ^ MAGIC {
        return prp("invalid PRP header");
    }
    if u64::from(size) > max_payload {
        return prp("PRP payload exceeds the configured limit");
    }
    if packet.len() != HEADER_SIZE + size as usize {
        return prp("PRP envelope length mismatch");
    }
    Ok(&packet[HEADER_SIZE..])
}

/// High and low words of a signed 64-bit timestamp, as the serializer writes them.
pub fn timestamp_words(timestamp: i64) -> (u32, u32) {
    let value = timestamp as u64;
    ((value >> 32) as u32, value as u32)
}

pub fn timestamp(high: u32, low: u32) -> i64 {
    ((u64::from(high) << 32) | u64::from(low)) as i64
}

fn validity(value: u32) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => prp("noncanonical PRP validity field"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GazePoint {
    pub timestamp: i64,
    pub valid: bool,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadPose {
    pub timestamp: i64,
    pub position_valid: bool,
    pub position: [f64; 3],
    pub rotation_valid: [bool; 3],
    pub rotation: [f64; 3],
}

fn body_start(size: usize, timestamp: i64) -> Vec<u8> {
    let mut body = Vec::with_capacity(size);
    let (high, low) = timestamp_words(timestamp);
    push_u32(&mut body, size as u32);
    push_u32(&mut body, high);
    push_u32(&mut body, low);
    body
}

/// The 24-byte body used by stream ids 3 and 29.
pub fn encode_gaze_body(sample: &GazePoint) -> Vec<u8> {
    let mut body = body_start(GAZE_BODY_SIZE, sample.timestamp);
    push_u32(&mut body, u32::from(sample.valid));
    push_f32(&mut body, sample.x);
    push_f32(&mut body, sample.y);
    body
}

fn sized_body(body: &[u8], size: usize) -> bool {
    body.len() == size && le_u32(body, 0) as usize == size
}

pub fn decode_gaze_body(body: &[u8]) -> Result<GazePoint> {
    if !sized_body(body, GAZE_BODY_SIZE) {
        return prp("invalid gaze body length");
    }
    Ok(GazePoint {
        timestamp: timestamp(le_u32(body, 4), le_u32(body, 8)),
        valid: validity(le_u32(body, 12))?,
        x: le_f32(body, 16),
        y: le_f32(body, 20),
    })
}

/// The 52-byte body used by stream id 2.
pub fn encode_head_body(sample: &HeadPose) -> Vec<u8> {
    let mut body = body_start(HEAD_BODY_SIZE, sample.timestamp);
    push_u32(&mut body, u32::from(sample.position_valid));
    sample.position.iter().for_each(|value| push_f32(&mut body, *value));
    sample.rotation_valid.iter().for_each(|valid| push_u32(&mut body, u32::from(*valid)));
    sample.rotation.iter().for_each(|value| push_f32(&mut body, *value));
    body
}

pub fn decode_head_body(body: &[u8]) -> Result<HeadPose> {
    if !sized_body(body, HEAD_BODY_SIZE) {
        return prp("invalid head body length");
    }
    let floats = |start: usize| [le_f32(body, start), le_f32(body, start + 4), le_f32(body, start + 8)];
    Ok(HeadPose {
        timestamp: timestamp(le_u32(body, 4), le_u32(body, 8)),
        position_valid: validity(le_u32(body, 12))?,
        position: floats(16),
        rotation_valid: [validity(le_u32(body, 28))?, validity(le_u32(body, 32))?, validity(le_u32(body, 36))?],
        rotation: floats(40),
    })
}

/// A sample of a non-compound stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamSample {
    Gaze(GazePoint),
    Head(HeadPose),
}

/// Operation 1 or 11 (`kind`) carrying one stream body, with empty extensions: the layout
/// the real service sends.
pub fn stream_package(kind: u32, message_tag: u32, stream: u32, body: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(28 + body.len() + 8);
    for word in [12, message_tag, kind, 4, 8, stream, 4] {
        push_u32(&mut payload, word);
    }
    payload.extend_from_slice(body);
    push_u32(&mut payload, 4);
    push_u32(&mut payload, 0);
    encode_envelope(&payload, DEFAULT_MAX_PAYLOAD).expect("stream packages fit the default limit")
}

pub const OPERATION_STREAM: u32 = 1;

/// The observed non-compound stream operation with empty extensions.
///
/// `message_tag` is the opaque u32 preceding operation type 1. Its service-level meaning is
/// not established; a caller must supply it rather than assume zero. This is a serializer,
/// not an operational service implementation.
pub fn encode_stream_packet(sample: &StreamSample, message_tag: u32, responsive: bool) -> Result<Vec<u8>> {
    let (stream, body) = match sample {
        StreamSample::Gaze(gaze) => {
            (if responsive { RESPONSIVE_GAZE_POINT } else { GAZE_POINT }, encode_gaze_body(gaze))
        }
        StreamSample::Head(head) if !responsive => (HEAD_POSE, encode_head_body(head)),
        StreamSample::Head(_) => return unsupported_sample(),
    };
    Ok(stream_package(OPERATION_STREAM, message_tag, stream, &body))
}

pub fn unsupported_sample<T>() -> Result<T> {
    prp("unsupported stream sample")
}

#[cfg(test)]
mod tests;
