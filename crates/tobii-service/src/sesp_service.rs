//! Head pose for the game DLL's tet-tcp devices as the SESP service sends it.
//!
//! The legacy module turns every SESP head position p into A p + t, with A a rotation
//! about x by the display tilt and t minus the display centre, both from the device display
//! area (0x18004ac80, applied by 0x180048d30). Stream Engine head positions are already
//! display centred, so by default positions are sent as A^T (p - t), which the module turns
//! back into the position the PRP route delivers. Without a transform they are sent
//! unchanged, as ses_windows.dll does. The asyncio server that pairs SESP channels stays in
//! `tobii_linux.sesp_service`; docs/sesp.md has the evidence.

use tobii_protocol::numeric::round_f32;

use crate::sesp::{HeadPose, INVALID, VALID, encode_head_pose};

/// Unsent bytes above which head-pose messages are dropped for a slow client.
pub const MAX_PENDING_BYTES: usize = 64 * 1024;
pub const CAPABILITIES: [u32; 2] = [crate::sesp::CAPABILITY_HEAD_POSE, crate::sesp::CAPABILITY_STREAM_STATUS];

/// What the legacy module does to SESP head positions for one display area.
///
/// `cos`, `sin` and `translation` are the float32 values the module derives; applying them
/// is modelled in double precision, which agrees with the module within 1e-3 mm
/// (tools/sesp_oracle.py).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyPositionTransform {
    pub cos: f64,
    pub sin: f64,
    pub translation: [f64; 3],
}

impl LegacyPositionTransform {
    /// From the top-left, top-right and bottom-left display corners in mm.
    pub fn from_display_area(top_left: [f64; 3], top_right: [f64; 3], bottom_left: [f64; 3]) -> Self {
        let tilt = round_f32(-round_f32(top_left[2] - bottom_left[2]).atan2(round_f32(top_left[1] - bottom_left[1])));
        let translation = std::array::from_fn(|i| round_f32(round_f32(bottom_left[i] + top_right[i]) * -0.5));
        Self { cos: round_f32(tilt.cos()), sin: round_f32(tilt.sin()), translation }
    }

    pub fn apply(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let [tx, ty, tz] = self.translation;
        [x + tx, self.cos * y - self.sin * z + ty, self.sin * y + self.cos * z + tz]
    }

    pub fn invert(&self, position: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = std::array::from_fn(|i| position[i] - self.translation[i]);
        [x, self.cos * y + self.sin * z, -self.sin * y + self.cos * z]
    }
}

/// One head-pose sample on the service clock, as the head-pose pipeline produces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadPoseSample {
    pub valid: bool,
    pub position_mm: [f64; 3],
    pub rotation_rad: [f64; 3],
}

/// SESP type 8 for one sample, at `timestamp_us` on the game's counter.
pub fn head_pose_message(
    sequence: u32,
    sample: &HeadPoseSample,
    timestamp_us: i64,
    transform: Option<&LegacyPositionTransform>,
) -> Vec<u8> {
    let validity = if sample.valid { VALID } else { INVALID };
    let position = transform.map_or(sample.position_mm, |transform| transform.invert(sample.position_mm));
    encode_head_pose(
        sequence,
        &HeadPose {
            timestamp_us,
            position,
            rotation: sample.rotation_rad,
            position_validity: validity,
            rotation_validity: [validity; 3],
        },
    )
}

#[cfg(test)]
mod tests;
