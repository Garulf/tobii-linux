//! Tobii's neural networks on OpenVINO and the numeric code around them.
//!
//! `legacy_ir` translates the researched IR4/IR6 graphs into a current OpenVINO IR,
//! `openvino` loads the OpenVINO C library at run time and runs the compiled networks,
//! `d1` holds the landmark network's crop, input tensor and output decoding, `d1_quality`
//! its confidence aggregation and gate, and `face_detect` the face finder's letterbox,
//! anchors, decoding and suppression. `numeric` reproduces the NumPy and Python float
//! semantics the reconstruction was checked with.

pub mod d1;
pub mod d1_quality;
pub mod error;
pub mod face_detect;
pub mod legacy_ir;
pub mod numeric;
pub mod openvino;

pub use d1::{Affine, D1LandmarkModel, D1Landmarks, D1Output, Point};
pub use error::{Error, Result};
pub use face_detect::{Candidate, DetectOptions, Detection, FaceFinder, Letterbox};
pub use legacy_ir::IrModel;
pub use openvino::{Network, Runtime};

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
    pub fn runtime() -> Option<crate::Runtime> {
        let wheel = Path::new(ROOT).join(".local/inference/openvino/libs");
        match crate::Runtime::shared(&[wheel]) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                eprintln!("skipped: OpenVINO is unavailable ({error})");
                None
            }
        }
    }
}
