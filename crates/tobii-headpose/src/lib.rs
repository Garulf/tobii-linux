//! Head-pose math of Tobii's platform runtime, rebuilt from its float32 machine code.
//!
//! Bottom up: float32 inversion (`linalg`), the face-model projection and Jacobians
//! (`projection`), mesh normalization (`mesh`), the installed fitting model (`model`), the
//! fitting updates (`fit`), the bounded retry policy (`policy`) and the temporal caller
//! (`temporal`); then the LBF eye refinement (`lbf`, `eye_refine`), landmark smoothing
//! (`smoothing`) and the public head-pose output (`output`, `geometry`, `pipeline`). On top,
//! the face tracker (`tracking`) runs the `tobii-inference` face finder and D1 network and
//! drives [`temporal::TemporalPoseFitter`], [`smoothing::LandmarkFilter`] per frame, and
//! [`processor::HeadPoseProcessor`] turns one image frame into one [`HeadPoseSample`];
//! `d1_pose` fits the landmarks of a supplied face box. Assets are read with `tobii-models`.
//! Plain types in, plain types out.

#![allow(clippy::excessive_precision, reason = "constants are written as the exact float32 values the runtime stores")]
#![allow(
    clippy::neg_cmp_op_on_partial_ord,
    reason = "negated comparisons keep the original's NaN behaviour (unordered compares)"
)]

pub mod array;
pub mod d1_pose;
pub mod error;
pub mod eye_refine;
pub mod fit;
pub mod geometry;
pub mod lbf;
pub mod linalg;
pub mod mesh;
pub mod model;
pub mod output;
pub mod pipeline;
pub mod policy;
pub mod processor;
pub mod projection;
pub mod smoothing;
pub mod temporal;
pub mod tracking;

pub use array::{Basis, Matrix, Vec3};
pub use error::{Error, Result};
pub use model::{FdpId, PoseModel, load_pose_model};
pub use pipeline::{DisplayArea, HeadPoseOutput, HeadPoseSample};
pub use processor::HeadPoseProcessor;
pub use smoothing::{LandmarkFilter, LandmarkRecord};
pub use temporal::{TemporalFit, TemporalPoseFitter};
pub use tracking::{FaceTracker, TrackedFrame};

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::{Path, PathBuf};
    use std::sync::LazyLock;

    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    static MODELS: LazyLock<PathBuf> = LazyLock::new(|| Path::new(ROOT).join("models/tobii-et5"));

    /// The local models/tobii-et5 layout, or `None` (the test then skips) when it is absent.
    /// The models are not part of the repository; see models/README.md.
    pub fn models() -> Option<&'static Path> {
        if MODELS.is_dir() {
            Some(MODELS.as_path())
        } else {
            eprintln!("skipped: models/tobii-et5 is absent");
            None
        }
    }

    /// The OpenVINO runtime from `TOBII_OPENVINO_LIBRARY` or the wheel under `.local/inference`,
    /// or `None` (the test then skips) when neither is available.
    pub fn runtime() -> Option<tobii_inference::Runtime> {
        let wheel = Path::new(ROOT).join(".local/inference/openvino/libs");
        match tobii_inference::Runtime::shared(&[wheel]) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                eprintln!("skipped: OpenVINO is unavailable ({error})");
                None
            }
        }
    }
}
