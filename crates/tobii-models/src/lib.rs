//! Tobii's head-tracking models and face assets.
//!
//! The models are not part of this project: the user supplies a models/tobii-et5 layout.
//! This crate finds it (`paths`), knows the SHA-256 of its files (`digests`), reads the
//! assets the head-pose pipeline loads (`face`, `lbf`) and expands the LBF coefficients
//! for inspection (`decoded`).

pub mod decoded;
pub mod digests;
pub mod error;
pub mod face;
pub mod json;
pub mod lbf;
pub mod paths;
pub mod tree;

mod bytes;

pub use bytes::sha256_hex;
pub use error::{Error, Result};
pub use face::{FaceMesh, Fdp, FdpPoint, Shapes};
pub use lbf::{LbfModel, LvModel, Matrix};
pub use tree::FileTree;

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::{Path, PathBuf};
    use std::sync::LazyLock;

    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    pub static COMMITTED: LazyLock<PathBuf> = LazyLock::new(|| Path::new(ROOT).join("models/tobii-et5"));
    pub static SUPPORT: LazyLock<PathBuf> = LazyLock::new(|| COMMITTED.join("support"));

    pub fn support_file(relative: &str) -> PathBuf {
        SUPPORT.join(relative)
    }
}
