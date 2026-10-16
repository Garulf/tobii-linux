//! Fitted face pose to the public 48-byte head-pose sample (producer 0x140293ca0 onwards).

use crate::array::Vec3;
use crate::geometry::{Matrix4, camera_extrinsic, display_matrix, geometry_config, geometry_stage};
use crate::output::{HeadPoseFilter, PI, adapter_head_pose, matrix_euler};

/// `tobii_head_pose_t` as filled by 0x140287120.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadPoseSample {
    pub timestamp_us: i64,
    pub valid: bool,
    pub position_mm: Vec3,
    pub rotation_rad: Vec3,
}

impl HeadPoseSample {
    pub const SIZE: usize = 48;

    /// `<qi3fiii3f>`: timestamp, position validity, position, the three rotation
    /// validities and the rotation; an invalid sample carries zeros.
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let flag = i32::from(self.valid);
        let (position, rotation) =
            if self.valid { (self.position_mm, self.rotation_rad) } else { ([0.0; 3], [0.0; 3]) };
        let mut bytes = Vec::with_capacity(Self::SIZE);
        bytes.extend(self.timestamp_us.to_le_bytes());
        bytes.extend(flag.to_le_bytes());
        position.iter().for_each(|v| bytes.extend(v.to_le_bytes()));
        [flag; 3].iter().for_each(|f| bytes.extend(f.to_le_bytes()));
        rotation.iter().for_each(|v| bytes.extend(v.to_le_bytes()));
        bytes.try_into().unwrap()
    }
}

/// Stream Engine display area in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayArea {
    pub top_left: Vec3,
    pub top_right: Vec3,
    pub bottom_left: Vec3,
}

/// Adapter slot 5, matrix-to-angles, geometry stage and One Euro filter stage.
///
/// The camera extrinsic comes from the tracker's hardware info. Without a display area the
/// original produces no head pose.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadPoseOutput {
    pub camera: Matrix4,
    pub display: Matrix4,
    pub filter: HeadPoseFilter,
}

impl HeadPoseOutput {
    pub fn new(camera_translation: Vec3, camera_angles: Vec3, area: &DisplayArea) -> Self {
        Self {
            camera: camera_extrinsic(camera_translation, camera_angles),
            display: display_matrix(area.top_left, area.bottom_left, area.top_right),
            filter: HeadPoseFilter::default(),
        }
    }

    /// Display-axes rotation and position before the One Euro filters.
    pub fn unfiltered(&self, fitted_rigid_state: &[f32; 6]) -> (Vec3, Vec3) {
        let [rx, ry, rz, tx, ty, tz] = *fitted_rigid_state;
        let (matrix, position) = adapter_head_pose([rx, ry - PI, rz], [tx, ty, tz]);
        geometry_stage(matrix_euler(&matrix), position, &self.camera, &self.display, &geometry_config())
    }

    /// `fitted_rigid_state` is `(rx, ry, rz, tx, ty, tz)` from the face-model fit, or `None`.
    pub fn process(&mut self, fitted_rigid_state: Option<&[f32; 6]>, timestamp_us: i64) -> HeadPoseSample {
        let Some(state) = fitted_rigid_state else {
            return HeadPoseSample { timestamp_us, valid: false, position_mm: [0.0; 3], rotation_rad: [0.0; 3] };
        };
        let (rotation, position) = self.unfiltered(state);
        let (rotation_rad, position_mm) = self.filter.filter(rotation, position, timestamp_us);
        HeadPoseSample { timestamp_us, valid: true, position_mm, rotation_rad }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALBEDO: DisplayArea = DisplayArea {
        top_left: [-296.62, 326.0, 111.82],
        top_right: [300.38, 326.0, 111.82],
        bottom_left: [-296.62, 10.27, -3.10],
    };

    #[test]
    fn missing_fit_gives_invalid_zero_sample() {
        let sample = HeadPoseOutput::new([0.0; 3], [0.0; 3], &ALBEDO).process(None, 42);
        let bytes = sample.encode();
        assert_eq!(i64::from_le_bytes(bytes[..8].try_into().unwrap()), 42);
        assert_eq!(bytes[8..], [0; 40]);
    }

    #[test]
    fn frontal_head_reports_display_tilt_and_distance() {
        let mut output = HeadPoseOutput::new([0.0; 3], [0.0; 3], &ALBEDO);
        let sample = output.process(Some(&[0.0, PI, 0.0, 0.0, 0.0, 0.6]), 1000);
        let bytes = sample.encode();
        assert_eq!(i32::from_le_bytes(bytes[8..12].try_into().unwrap()), 1);
        assert_eq!(bytes[24..36], [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
        assert!((sample.rotation_rad[0].abs() - 20f32.to_radians()).abs() < 0.01);
        assert!(sample.position_mm[2] > 500.0);
    }
}
