//! Tobii's head-tracking models and face assets.
//!
//! The models/tobii-et5 layout (`digests`) and typed readers for the assets the head-pose
//! pipeline loads (`face`, `lbf`).

pub mod digests;
pub mod error;
pub mod face;
pub mod json;
pub mod lbf;
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
    pub static RUNTIME: LazyLock<PathBuf> = LazyLock::new(|| {
        Path::new(ROOT).join("evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe")
    });

    pub fn support_file(relative: &str) -> PathBuf {
        SUPPORT.join(relative)
    }

    pub fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    /// A fresh directory under the build's target directory (the system /tmp may be small),
    /// removed when dropped.
    #[derive(Debug)]
    pub struct TestDir(PathBuf);

    impl TestDir {
        pub fn new(name: &str) -> Self {
            let target = option_env!("CARGO_TARGET_DIR").map_or_else(|| Path::new(ROOT).join("target"), PathBuf::from);
            let path = target.join("tobii-models-tests").join(format!("{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
