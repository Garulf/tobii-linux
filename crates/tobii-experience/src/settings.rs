//! The app's persisted settings, Tobii's `PersistentSettings` as far as the app uses them.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "experience.json";

/// The settings file's content; fields missing from the file take their defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Tobii's `SkipConfirmQuitDialog` (default false: Quit asks first).
    pub skip_quit_confirmation: bool,
    /// Tobii's `GazeTrackerHasBeenConnected`: a tracker was attached once, so the page keeps
    /// its gaze sections while it is unplugged or the service is down.
    pub gaze_tracker_has_been_connected: bool,
    /// Whether the page shows Tobii's banner that asks to set up the display or calibrate
    /// (default true). Not a Tobii setting.
    pub show_banner: bool,
    /// Whether the app starts the setup on its own the first time it sees nothing set up,
    /// as Tobii's licence page does (default true). Not a Tobii setting.
    pub auto_setup: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            skip_quit_confirmation: false,
            gaze_tracker_has_been_connected: false,
            show_banner: true,
            auto_setup: true,
        }
    }
}

impl Settings {
    /// `<XDG_CONFIG_HOME or ~/.config>/tobii-linux/experience.json`, from the given values of
    /// `XDG_CONFIG_HOME` and `HOME`. A relative `XDG_CONFIG_HOME` is ignored, as the XDG
    /// specification asks; `None` without a usable home.
    pub fn path_from(xdg_config_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
        let config_home = match xdg_config_home.filter(|path| path.is_absolute()) {
            Some(path) => path.to_path_buf(),
            None => home.filter(|path| !path.as_os_str().is_empty())?.join(".config"),
        };
        Some(config_home.join("tobii-linux").join(FILE_NAME))
    }

    /// [`Settings::path_from`] with this process's environment.
    pub fn default_path() -> Option<PathBuf> {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Settings::path_from(xdg.as_deref(), home.as_deref())
    }

    /// The stored settings; the defaults when the file does not exist.
    pub fn load(path: &Path) -> io::Result<Settings> {
        match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(error) => Err(error),
        }
    }

    /// Replaces the file atomically (a synced sibling renamed over it), creating its directory.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut partial = path.as_os_str().to_owned();
        partial.push(".partial");
        let partial = PathBuf::from(partial);
        let mut file = std::fs::File::create(&partial)?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&partial, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("tobii-experience-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn settings_round_trip_and_defaults() {
        assert!(!Settings::default().skip_quit_confirmation, "Tobii shows the quit dialog by default");
        assert!(!Settings::default().gaze_tracker_has_been_connected);
        assert!(Settings::default().show_banner && Settings::default().auto_setup);

        let directory = scratch("settings");
        let path = directory.join("tobii-linux").join("experience.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());

        let settings = Settings {
            skip_quit_confirmation: true,
            gaze_tracker_has_been_connected: true,
            show_banner: false,
            auto_setup: false,
        };
        settings.store(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let expected = serde_json::json!({
            "skip_quit_confirmation": true,
            "gaze_tracker_has_been_connected": true,
            "show_banner": false,
            "auto_setup": false,
        });
        assert_eq!(json, expected);
        let leftovers: Vec<_> =
            std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers, ["experience.json"], "the partial file is renamed over the old one");

        Settings::default().store(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());

        std::fs::write(&path, "{}").unwrap();
        assert_eq!(Settings::load(&path).unwrap(), Settings::default(), "missing fields take their defaults");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);

        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn path_follows_xdg_config_home() {
        let expected = |base: &str| Some(PathBuf::from(base).join("tobii-linux/experience.json"));
        let home = Some(Path::new("/home/garulf"));
        assert_eq!(Settings::path_from(Some(Path::new("/xdg")), home), expected("/xdg"));
        assert_eq!(Settings::path_from(None, home), expected("/home/garulf/.config"));
        assert_eq!(Settings::path_from(Some(Path::new("")), home), expected("/home/garulf/.config"));
        assert_eq!(Settings::path_from(Some(Path::new("relative")), home), expected("/home/garulf/.config"));
        assert_eq!(Settings::path_from(None, None), None);
        assert_eq!(Settings::path_from(None, Some(Path::new(""))), None);
    }
}
