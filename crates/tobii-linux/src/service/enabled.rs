//! Whether eye tracking is enabled, kept across service runs as Tobii keeps
//! `EyeTrackingEnabled` per user (docs/experience-app.md, section 2).

use std::path::{Path, PathBuf};

use tobii_device::DeviceError;

/// Reply statuses the stream engine turns into `TOBII_ERROR_NOT_SUPPORTED` (`0x1402c1110`,
/// docs/gaze-calibration.md): unknown or unsupported operation, unknown id, extension required.
pub const NOT_SUPPORTED_STATUSES: [u32; 4] = [9, 10, 13, 15];

/// Whether a failed pause or resume is one Tobii's toggle counts as done: not supported.
pub fn not_supported(error: &DeviceError) -> bool {
    matches!(error, DeviceError::Failed { status, .. } if NOT_SUPPORTED_STATUSES.contains(status))
}

/// File name of the state under `tobii-linux/` in the state directory.
pub const STATE_FILE: &str = "eye-tracking-enabled";

/// The file holding the on/off state: `1` enabled, `0` disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnabledState {
    path: PathBuf,
}

impl EnabledState {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `<XDG_STATE_HOME or ~/.local/state>/tobii-linux/eye-tracking-enabled`.
    pub fn default_path(xdg_state_home: Option<&str>, home: Option<&str>) -> PathBuf {
        let state_home = match xdg_state_home.filter(|value| !value.is_empty()) {
            Some(value) => PathBuf::from(value),
            None => home_directory(home).join(".local/state"),
        };
        state_home.join("tobii-linux").join(STATE_FILE)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stored state; enabled when the file is missing, unreadable or not `0`.
    pub fn load(&self) -> bool {
        std::fs::read_to_string(&self.path).map_or(true, |text| text.trim() != "0")
    }

    /// Replaces the file atomically, creating its directory.
    pub fn store(&self, enabled: bool) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut partial = self.path.clone().into_os_string();
        partial.push(".partial");
        std::fs::write(&partial, if enabled { "1\n" } else { "0\n" })?;
        std::fs::rename(&partial, &self.path)
    }
}

fn home_directory(home: Option<&str>) -> PathBuf {
    home.map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("~"))
}
