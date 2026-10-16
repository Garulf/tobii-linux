//! D1 landmarks of a supplied face box and the no-hint single-parent fitting branch.
//!
//! The caller supplies grayscale pixels, a face box, the quality mode and the prior crop
//! roll. Coordinates are the internal fitting coordinates, not the final Tobii API; face
//! detection, temporal mode selection and runtime hardware calibration are not part of it.

use std::path::Path;

use tobii_inference::d1::D1Landmarks;
use tobii_inference::d1_quality::{d1_quality_score, passes_d1_quality};
use tobii_inference::{D1LandmarkModel, Runtime};
use tobii_models::Shapes;

use crate::error::{Result, invalid};
use crate::model::{PoseModel, load_pose_model};
use crate::policy::{StagedFit, fit_staged_landmarks};
use crate::temporal::Rectangle;
use crate::tracking::INIT_SHAPE;

pub const MODE_MESSAGE: &str = "Expected an observed caller quality mode (0, 1 or 2)";

/// One frame's landmarks, their quality score and gate, and the fit of accepted landmarks.
#[derive(Debug, Clone, PartialEq)]
pub struct FramePose {
    pub landmarks: D1Landmarks,
    pub quality_score: f32,
    pub quality_accepted: bool,
    pub fit: Option<StagedFit>,
}

/// The D1 network and the installed fitting model.
#[derive(Debug)]
pub struct D1Pose {
    landmarks: D1LandmarkModel,
    pub model: PoseModel,
    quality_ids: Vec<(i64, i64)>,
}

impl D1Pose {
    pub fn new(runtime: &Runtime, model_directory: &Path) -> Result<Self> {
        let reference = Shapes::read(&model_directory.join(INIT_SHAPE), 2)?;
        let landmarks =
            D1LandmarkModel::new(runtime, &model_directory.join("d1"), reference.values.as_chunks::<2>().0)?;
        let model = load_pose_model(&model_directory.join("support"))?;
        let quality_ids = model.landmark_ids.iter().map(|&(g, i)| (i64::from(g), i64::from(i))).collect();
        Ok(Self { landmarks, model, quality_ids })
    }

    /// The landmarks of `face_box` in a `width`×`height` frame, gated with `caller_mode`
    /// (0, 1 or 2) and fitted when they pass.
    pub fn predict_frame(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
        face_box: Rectangle,
        caller_mode: i64,
        roll_degrees: f64,
    ) -> Result<FramePose> {
        if !(0..=2).contains(&caller_mode) {
            return invalid(MODE_MESSAGE);
        }
        let landmarks = self.landmarks.predict_frame(pixels, width, height, face_box, roll_degrees, true)?;
        let quality_score = d1_quality_score(&landmarks.quality, &self.quality_ids)?;
        let quality_accepted = passes_d1_quality(quality_score, caller_mode);
        let fit = if quality_accepted {
            Some(fit_staged_landmarks(&self.model, &landmarks.normalized_points, width as i64, height as i64)?)
        } else {
            None
        };
        Ok(FramePose { landmarks, quality_score, quality_accepted, fit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{MODELS, runtime};

    #[test]
    fn rejected_landmarks_are_not_fitted() {
        let Some(runtime) = runtime() else { return };
        let pose = D1Pose::new(&runtime, &MODELS).unwrap();
        let result = pose.predict_frame(&vec![0; 160 * 120], 160, 120, [30, 20, 65, 80], 0, 0.0).unwrap();
        assert_eq!(result.landmarks.normalized_points.len(), 57);
        assert!(!result.quality_accepted && result.fit.is_none());
        assert!(pose.predict_frame(&vec![0; 160 * 120], 160, 120, [30, 20, 65, 80], 3, 0.0).is_err());
    }
}
