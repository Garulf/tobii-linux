//! Named calibration profiles, kept and switched the way Tobii's Windows software does it.
//!
//! On Windows each user profile is a registry key
//! HKLM\SOFTWARE\WOW6432Node\Tobii\EyeXConfig\UserProfiles\<name> holding TrackedEyes and,
//! per tracker and monitor, the 1100 calibration blob verbatim
//! (Calibrations\<tracker uri>&<monitor id>\Data). CurrentUserProfile names the active one.
//! When it changes, the EyeX engine sends tobii_set_enabled_eye (3160) and
//! tobii_calibration_apply (1110) with that profile's blob
//! (EyeTrackingEngine.TrySetCalibrationAndProfileSettings). See the profiles section of
//! docs/gaze-calibration.md.
//!
//! Here a profile is a file NAME.setpm in calibration.setpm layout under
//! $XDG_DATA_HOME/tobii-linux/profiles, and the name of the active profile is kept in the
//! file `current` beside them.

use std::path::PathBuf;

use crate::calibration::{self, TRANSFER_TIMEOUT_S, load_backup, save_backup};
use crate::device::{self, Eye, LocalCalibration};
use crate::error::{GazeError, Result};
use crate::operation::{External, Io, OperationError, OperationResult};

pub const PROFILE_SUFFIX: &str = ".setpm";
pub const CURRENT_FILE: &str = "current";
pub const MAX_NAME_LENGTH: usize = 255;
pub const NAME_PUNCTUATION: [char; 4] = [' ', '(', ')', '-'];

/// The profiles directory for an environment's XDG_DATA_HOME and HOME.
pub fn default_directory(xdg_data_home: Option<&str>, home: Option<&str>) -> PathBuf {
    let data_home = match xdg_data_home.filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => expand_user(home.unwrap_or("~")).join(".local/share"),
    };
    data_home.join("tobii-linux").join("profiles")
}

/// `os.path.expanduser` for `~` and `~/...`, from the process's HOME.
fn expand_user(path: &str) -> PathBuf {
    let Some(rest) = path.strip_prefix('~').filter(|rest| rest.is_empty() || rest.starts_with('/')) else {
        return PathBuf::from(path);
    };
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(format!("{}{rest}", home.trim_end_matches('/'))),
        Err(_) => PathBuf::from(path),
    }
}

/// Python's `str.isspace`.
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// A string as Python's `repr` prints it.
pub(crate) fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut repr = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => repr.push_str("\\\\"),
            '\n' => repr.push_str("\\n"),
            '\r' => repr.push_str("\\r"),
            '\t' => repr.push_str("\\t"),
            c if c == quote => {
                repr.push('\\');
                repr.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => repr.push_str(&format!("\\x{:02x}", c as u32)),
            c => repr.push(c),
        }
    }
    repr.push(quote);
    repr
}

/// ConfigurationManager.ValidateUserProfileName: letters, digits, space, '(', ')', '-'; at most 255.
pub fn check_name(name: &str) -> Result<&str> {
    if name.chars().all(is_space) || name.chars().count() > MAX_NAME_LENGTH {
        return Err(GazeError::Profile("A profile name needs 1 to 255 characters".into()));
    }
    let mut invalid: Vec<char> =
        name.chars().filter(|c| !c.is_alphanumeric() && !NAME_PUNCTUATION.contains(c)).collect();
    invalid.sort_unstable();
    invalid.dedup();
    if !invalid.is_empty() {
        let joined: String = invalid.into_iter().collect();
        return Err(GazeError::Profile(format!("Invalid characters in profile name: {}", python_repr(&joined))));
    }
    Ok(name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub path: PathBuf,
    pub calibration: LocalCalibration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileStore {
    pub directory: PathBuf,
}

impl ProfileStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self { directory: directory.into() }
    }

    /// The store under the process environment's profiles directory.
    pub fn default_location() -> Self {
        let get = |name: &str| std::env::var(name).ok();
        Self::new(default_directory(get("XDG_DATA_HOME").as_deref(), get("HOME").as_deref()))
    }

    pub fn path(&self, name: &str) -> Result<PathBuf> {
        Ok(self.directory.join(format!("{}{PROFILE_SUFFIX}", check_name(name)?)))
    }

    pub fn names(&self) -> Result<Vec<String>> {
        if !self.directory.is_dir() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.directory)? {
            let Ok(file_name) = entry?.file_name().into_string() else {
                continue;
            };
            if file_name.starts_with('.') {
                continue;
            }
            if let Some(name) = file_name.strip_suffix(PROFILE_SUFFIX) {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn load(&self, name: &str) -> Result<Profile> {
        let path = self.path(name)?;
        if !path.exists() {
            let message = format!("No profile named {} in {}", python_repr(name), self.directory.display());
            return Err(GazeError::Profile(message));
        }
        let calibration = load_backup(&path)?;
        Ok(Profile { name: name.to_string(), path, calibration })
    }

    pub fn save(&self, name: &str, calibration: &LocalCalibration, overwrite: bool) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.directory)?;
        save_backup(&self.path(name)?, calibration, overwrite)
    }

    pub fn current(&self) -> Result<Option<String>> {
        let marker = self.directory.join(CURRENT_FILE);
        if !marker.exists() {
            return Ok(None);
        }
        let name = std::fs::read_to_string(marker)?.trim().to_string();
        Ok((!name.is_empty()).then_some(name))
    }

    pub fn set_current(&self, name: &str) -> Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        let name = check_name(name)?;
        std::fs::write(self.directory.join(CURRENT_FILE), format!("{name}\n"))?;
        Ok(())
    }
}

fn profile_error<E: External>(message: String) -> OperationError<E> {
    OperationError::Gaze(GazeError::Profile(message))
}

/// Reads the device calibration (1490, 1100, 1490) into a profile and verifies it; sends no write.
pub async fn save_from_device<E: External>(
    io: Io<E>,
    store: ProfileStore,
    name: String,
    overwrite: bool,
) -> OperationResult<Profile, E> {
    let calibration = calibration::backup_calibration(&io).await?;
    let path = store.save(&name, &calibration, overwrite)?;
    let check = calibration::verify_backup(&io, &load_backup(&path)?).await?;
    if !check.ok() {
        let (ids, blob) = (python_bool(check.ids_match()), python_bool(check.blob_matches()));
        return Err(profile_error(format!("Saved profile does not match the device (ids {ids}, blob {blob})")));
    }
    Ok(Profile { name, path, calibration })
}

fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchResult {
    pub profile: String,
    pub device_id_before: u32,
    pub device_id_after: u32,
    pub applied: bool,
}

/// Makes a profile active: 3160 enabled eye, then 1110 with its blob, then 1490 to confirm the id.
///
/// The engine sends both on every profile change; the apply is skipped here when the device
/// already reports the profile's id, as the runtime's own start check does. A `dry_run`
/// driver reports writes instead of sending them, so the id is not checked then and the
/// profile is not marked current.
pub async fn switch<E: External>(
    io: Io<E>,
    store: ProfileStore,
    name: String,
    eye: Eye,
    dry_run: bool,
) -> OperationResult<SwitchResult, E> {
    let profile = store.load(&name)?;
    let wanted = profile.calibration.calibration_id;
    let before = calibration::read_device_calibration_id(&io).await?;
    let applied = before != wanted;
    if applied {
        io.call(device::set_enabled_eye(0, eye), None).await.map_err(OperationError::External)?;
        let apply = device::calibration_apply(0, &profile.calibration.blob)?;
        io.call(apply, Some(TRANSFER_TIMEOUT_S)).await.map_err(OperationError::External)?;
    }
    let after = calibration::read_device_calibration_id(&io).await?;
    if !dry_run {
        if after != wanted {
            return Err(profile_error(format!(
                "Device reports calibration id {after:#x} after switching to {} ({wanted:#x})",
                python_repr(&name)
            )));
        }
        store.set_current(&name)?;
    }
    Ok(SwitchResult { profile: name, device_id_before: before, device_id_after: after, applied })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_directory_follows_xdg() {
        assert_eq!(default_directory(Some("/data"), None), PathBuf::from("/data/tobii-linux/profiles"));
        assert_eq!(
            default_directory(None, Some("/home/u")),
            PathBuf::from("/home/u/.local/share/tobii-linux/profiles")
        );
    }

    #[test]
    fn names_follow_the_windows_rules() {
        for name in ["Garulf", "Guest (glasses)", "player-2", "Åsa"] {
            assert_eq!(check_name(name).unwrap(), name);
        }
        let long = "a".repeat(256);
        for name in ["", "   ", "a/b", "../x", "x.y", long.as_str()] {
            assert!(matches!(check_name(name), Err(GazeError::Profile(_))), "{name}");
        }
        let Err(GazeError::Profile(message)) = check_name("../x") else { panic!() };
        assert_eq!(message, "Invalid characters in profile name: './'");
    }

    #[test]
    fn repr_quotes_like_python() {
        assert_eq!(python_repr("Nobody"), "'Nobody'");
        assert_eq!(python_repr("it's"), "\"it's\"");
        assert_eq!(python_repr("a\\b\n"), "'a\\\\b\\n'");
    }
}
