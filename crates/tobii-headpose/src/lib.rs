//! Head-pose math of Tobii's platform runtime, rebuilt from its float32 machine code.
//!
//! Bottom up: float32 inversion (`linalg`), the face-model projection and Jacobians
//! (`projection`), mesh normalization (`mesh`), the installed fitting model (`model`), the
//! fitting updates (`fit`), the bounded retry policy (`policy`) and the temporal caller
//! (`temporal`); then the LBF eye refinement (`lbf`, `eye_refine`), landmark smoothing
//! (`smoothing`) and the public head-pose output (`output`, `geometry`, `pipeline`).
//! Assets are read with `tobii-models`. Plain types in, plain types out: the face tracker
//! drives [`temporal::TemporalPoseFitter`], [`smoothing::LandmarkFilter`] and
//! [`pipeline::HeadPoseOutput`] per frame.

#![allow(clippy::excessive_precision, reason = "constants are written as the exact float32 values the runtime stores")]
#![allow(
    clippy::neg_cmp_op_on_partial_ord,
    reason = "negated comparisons keep the original's NaN behaviour (unordered compares)"
)]

pub mod array;
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
pub mod projection;
pub mod smoothing;
pub mod temporal;

pub use array::{Basis, Matrix, Vec3};
pub use error::{Error, Result};
pub use model::{FdpId, PoseModel, load_pose_model};
pub use pipeline::{DisplayArea, HeadPoseOutput, HeadPoseSample};
pub use smoothing::{LandmarkFilter, LandmarkRecord};
pub use temporal::{TemporalFit, TemporalPoseFitter};
