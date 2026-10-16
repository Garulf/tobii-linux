//! One image frame to one head-pose sample: face tracking, fitting and the output stages.
//!
//! The models load once. Each tracker session starts with that session's camera extrinsic
//! and fresh tracking state and output filters, as the runtime builds a new algorithm
//! instance when a device is added.

use std::path::Path;

use tobii_inference::Runtime;

use crate::array::Vec3;
use crate::error::{Result, invalid};
use crate::pipeline::{DisplayArea, HeadPoseOutput, HeadPoseSample};
use crate::tracking::{FaceTracker, TrackedFrame};

pub const NOT_STARTED_MESSAGE: &str = "The head-pose processor has no camera calibration; start it first";

/// [`FaceTracker`] and [`HeadPoseOutput`] as the head-pose stream runs them.
#[derive(Debug)]
pub struct HeadPoseProcessor {
    pub tracker: FaceTracker,
    pub display_area: DisplayArea,
    pub output: Option<HeadPoseOutput>,
}

impl HeadPoseProcessor {
    pub fn new(runtime: &Runtime, model_directory: &Path, display_area: DisplayArea) -> Result<Self> {
        Ok(Self { tracker: FaceTracker::new(runtime, model_directory, None, None)?, display_area, output: None })
    }

    /// A new tracker session with the camera extrinsic from the tracker's hardware info.
    pub fn start(&mut self, camera_translation: Vec3, camera_angles: Vec3) {
        self.tracker.reset();
        self.output = Some(HeadPoseOutput::new(camera_translation, camera_angles, &self.display_area));
    }

    /// The sample of one `width`×`height` grayscale frame with its device timestamp.
    pub fn process(&mut self, pixels: &[u8], width: usize, height: usize, timestamp_us: i64) -> Result<HeadPoseSample> {
        Ok(self.track(pixels, width, height, timestamp_us)?.1)
    }

    /// [`Self::process`] that also returns the tracked frame.
    pub fn track(
        &mut self,
        pixels: &[u8],
        width: usize,
        height: usize,
        timestamp_us: i64,
    ) -> Result<(TrackedFrame, HeadPoseSample)> {
        let Some(output) = self.output.as_mut() else { return invalid(NOT_STARTED_MESSAGE) };
        let tracked = self.tracker.process(pixels, width, height, timestamp_us.div_euclid(1000))?;
        let sample = output.process(tracked.fit.as_ref().map(|fit| &fit.rigid_state), timestamp_us);
        Ok((tracked, sample))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{MODELS, runtime};

    const ALBEDO: DisplayArea = DisplayArea {
        top_left: [-296.62, 326.0, 111.82],
        top_right: [300.38, 326.0, 111.82],
        bottom_left: [-296.62, 10.27, -3.1],
    };

    #[test]
    fn blank_frames_give_invalid_samples_after_start() {
        let Some(runtime) = runtime() else { return };
        let mut processor = HeadPoseProcessor::new(&runtime, &MODELS, ALBEDO).unwrap();
        assert!(processor.process(&vec![0; 320 * 240], 320, 240, 5_000).is_err());
        processor.start([0.0, 0.0, 4.14], [0.0; 3]);
        let sample = processor.process(&vec![0; 320 * 240], 320, 240, 5_000).unwrap();
        assert_eq!((sample.timestamp_us, sample.valid), (5_000, false));
    }
}
