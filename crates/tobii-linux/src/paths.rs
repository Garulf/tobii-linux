//! Where the models and the OpenVINO library are found.

use std::path::PathBuf;

const MODELS_VARIABLE: &str = "TOBII_LINUX_MODELS";

/// The repository the binary was built from; its models are used when they are there, as
/// the Python package uses the checkout it runs from.
fn checkout() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `TOBII_LINUX_MODELS`, else the checkout's models/tobii-et5, else
/// `~/.local/share/tobii-linux/models` (`$XDG_DATA_HOME/tobii-linux/models`).
pub fn default_models() -> PathBuf {
    if let Some(configured) = std::env::var_os(MODELS_VARIABLE).filter(|value| !value.is_empty()) {
        return PathBuf::from(configured);
    }
    let repository = checkout().join("models/tobii-et5");
    if repository.is_dir() {
        return repository;
    }
    tobii_models::paths::default_models_dir()
}

/// Directories that may hold `libopenvino_c`: an `openvino` package on `PYTHONPATH` (the
/// `openvino/libs` directory of the wheel, which the Python package would load) and the
/// checkout's `.local/inference`.
pub fn openvino_directories() -> Vec<PathBuf> {
    let python_path = std::env::var_os("PYTHONPATH").unwrap_or_default();
    std::env::split_paths(&python_path)
        .chain([checkout().join(".local/inference")])
        .map(|root| root.join("openvino/libs"))
        .filter(|directory| directory.is_dir())
        .collect()
}

/// The shared OpenVINO runtime.
pub fn openvino() -> tobii_inference::Result<tobii_inference::Runtime> {
    tobii_inference::Runtime::shared(&openvino_directories())
}
