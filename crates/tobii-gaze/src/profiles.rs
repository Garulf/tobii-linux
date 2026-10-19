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
//! file `current` beside them. NAME.json, when present, holds the profile's tracked eyes and
//! which file serves which monitor; other monitors' calibrations are NAME@<monitor key>.setpm.
//! A profile without NAME.json tracks both eyes and its NAME.setpm serves every monitor.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::calibration::{self, TRANSFER_TIMEOUT_S, load_backup, save_backup};
use crate::device::{self, Eye, LocalCalibration};
use crate::error::{GazeError, Result};
use crate::operation::{External, Io, OperationError, OperationResult};

pub const PROFILE_SUFFIX: &str = ".setpm";
pub const SETTINGS_SUFFIX: &str = ".json";
pub const MONITOR_SEPARATOR: char = '@';
pub const CURRENT_FILE: &str = "current";
pub const MAX_NAME_LENGTH: usize = 255;
pub const NAME_PUNCTUATION: [char; 4] = [' ', '(', ')', '-'];
/// The profile Tobii's guest calibration (`Tobii.Configuration.exe -G`) writes; it is never
/// listed and cannot be deleted.
pub const GUEST: &str = "Guest";
/// The record of the last display setup, beside the profiles directory.
pub const DISPLAY_RECORD: &str = "display-setup.json";

/// $XDG_DATA_HOME/tobii-linux for an environment's XDG_DATA_HOME and HOME.
pub fn default_data_directory(xdg_data_home: Option<&str>, home: Option<&str>) -> PathBuf {
    let data_home = match xdg_data_home.filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => expand_user(home.unwrap_or("~")).join(".local/share"),
    };
    data_home.join("tobii-linux")
}

/// The profiles directory for an environment's XDG_DATA_HOME and HOME.
pub fn default_directory(xdg_data_home: Option<&str>, home: Option<&str>) -> PathBuf {
    default_data_directory(xdg_data_home, home).join("profiles")
}

/// Where `tobii-linux-setup` records the monitor of the last display setup.
pub fn default_display_record(xdg_data_home: Option<&str>, home: Option<&str>) -> PathBuf {
    default_data_directory(xdg_data_home, home).join(DISPLAY_RECORD)
}

/// The monitor key of the last display setup: the EDID id when it was read, else the
/// connector. `None` when the record or both fields are missing.
pub fn recorded_monitor_key(path: &Path) -> Option<String> {
    let record: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let monitor = record.get("monitor")?;
    let text = |field: &str| monitor.get(field)?.as_str().filter(|text| !text.is_empty()).map(str::to_string);
    text("edid_id").or_else(|| text("key"))
}

/// Tobii's `TrackedEyes` value as the eye the device tracks: 1 left, 2 right, else both.
pub fn tracked_eye(tracked_eyes: u32) -> Eye {
    match tracked_eyes {
        1 => Eye::Left,
        2 => Eye::Right,
        _ => Eye::Both,
    }
}

/// A monitor key as it appears in a file name: anything but letters, digits, `-` and `_`
/// becomes `_`.
fn file_key(monitor_key: &str) -> String {
    monitor_key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// What NAME.json holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Settings {
    tracked_eyes: u32,
    calibrations: BTreeMap<String, String>,
}

impl Settings {
    fn from_json(value: &Value) -> Self {
        let tracked_eyes = value.get("tracked_eyes").and_then(Value::as_u64).filter(|&v| v <= 2).unwrap_or(0) as u32;
        let calibrations = value
            .get("calibrations")
            .and_then(Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|(key, file)| Some((key.clone(), file.as_str()?.to_string())))
                    .filter(|(_, file)| is_plain_file_name(file))
                    .collect()
            })
            .unwrap_or_default();
        Self { tracked_eyes, calibrations }
    }

    fn to_json(&self) -> Value {
        json!({"tracked_eyes": self.tracked_eyes, "calibrations": self.calibrations})
    }
}

fn is_plain_file_name(file: &str) -> bool {
    file.ends_with(PROFILE_SUFFIX) && !file.starts_with('.') && !file.contains(['/', '\\'])
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

    fn settings_path(&self, name: &str) -> Result<PathBuf> {
        Ok(self.directory.join(format!("{}{SETTINGS_SUFFIX}", check_name(name)?)))
    }

    fn settings(&self, name: &str) -> Result<Settings> {
        let path = self.settings_path(name)?;
        if !path.exists() {
            return Ok(Settings::default());
        }
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text).map(|value| Settings::from_json(&value)).unwrap_or_default())
    }

    fn store_settings(&self, name: &str, settings: &Settings) -> Result<()> {
        let path = self.settings_path(name)?;
        let temporary = path.with_extension("json.partial");
        std::fs::write(&temporary, format!("{}\n", settings.to_json()))?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    fn require(&self, name: &str) -> Result<PathBuf> {
        let path = self.path(name)?;
        if !path.exists() {
            let message = format!("No profile named {} in {}", python_repr(name), self.directory.display());
            return Err(GazeError::Profile(message));
        }
        Ok(path)
    }

    /// Tobii's `TrackedEyes` of a profile: 0 both, 1 left, 2 right; 0 without NAME.json.
    pub fn tracked_eyes(&self, name: &str) -> Result<u32> {
        Ok(self.settings(name)?.tracked_eyes)
    }

    pub fn set_tracked_eyes(&self, name: &str, value: u32) -> Result<()> {
        if value > 2 {
            return Err(GazeError::Profile(format!("Tracked eyes must be 0, 1 or 2, not {value}")));
        }
        self.require(name)?;
        let mut settings = self.settings(name)?;
        settings.tracked_eyes = value;
        self.store_settings(name, &settings)
    }

    /// The profile's calibration for a monitor, else its NAME.setpm.
    pub fn calibration_for(&self, name: &str, monitor_key: Option<&str>) -> Result<LocalCalibration> {
        let settings = self.settings(name)?;
        match monitor_key.and_then(|key| settings.calibrations.get(key)) {
            Some(file) => Ok(load_backup(&self.directory.join(file))?),
            None => Ok(self.load(name)?.calibration),
        }
    }

    /// Whether the profile holds a calibration made for the monitor: its entry in NAME.json,
    /// or NAME.setpm when there is no monitor key. The legacy fallback does not count.
    pub fn has_calibration_for(&self, name: &str, monitor_key: Option<&str>) -> Result<bool> {
        match monitor_key {
            Some(key) => {
                let settings = self.settings(name)?;
                Ok(settings.calibrations.get(key).is_some_and(|file| self.directory.join(file).is_file()))
            }
            None => Ok(self.path(name)?.is_file()),
        }
    }

    /// Stores a calibration for a monitor. A new profile's first calibration is NAME.setpm;
    /// a monitor the profile has none for gets NAME@<monitor key>.setpm.
    pub fn save_for(
        &self,
        name: &str,
        monitor_key: Option<&str>,
        calibration: &LocalCalibration,
        overwrite: bool,
    ) -> Result<PathBuf> {
        let legacy = self.path(name)?;
        let Some(key) = monitor_key else { return self.save(name, calibration, overwrite) };
        let mut settings = self.settings(name)?;
        let file = match settings.calibrations.get(key) {
            Some(file) => file.clone(),
            None if !legacy.exists() => format!("{name}{PROFILE_SUFFIX}"),
            None => format!("{name}{MONITOR_SEPARATOR}{}{PROFILE_SUFFIX}", file_key(key)),
        };
        std::fs::create_dir_all(&self.directory)?;
        let path = save_backup(&self.directory.join(&file), calibration, overwrite)?;
        settings.calibrations.insert(key.to_string(), file);
        self.store_settings(name, &settings)?;
        Ok(path)
    }

    /// Removes a profile with every calibration and its settings; refuses the current
    /// profile and [`GUEST`].
    pub fn delete(&self, name: &str) -> Result<()> {
        let legacy = self.path(name)?;
        if name == GUEST {
            return Err(GazeError::Profile(format!("{GUEST} cannot be deleted")));
        }
        if self.current()?.as_deref() == Some(name) {
            return Err(GazeError::Profile(format!("{} is the current profile", python_repr(name))));
        }
        let settings = self.settings_path(name)?;
        if !legacy.exists() && !settings.exists() {
            self.require(name)?;
        }
        let prefix = format!("{name}{MONITOR_SEPARATOR}");
        for entry in std::fs::read_dir(&self.directory)? {
            let path = entry?.path();
            let Some(file_name) = path.file_name().and_then(|file| file.to_str()) else { continue };
            if file_name.starts_with(&prefix) && file_name.ends_with(PROFILE_SUFFIX) {
                std::fs::remove_file(&path)?;
            }
        }
        for path in [settings, legacy] {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
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
            if let Some(name) = file_name.strip_suffix(PROFILE_SUFFIX).filter(|name| check_name(name).is_ok()) {
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

/// How [`switch_with`] puts a profile on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchOptions {
    pub eye: Eye,
    /// The monitor whose calibration is applied; `None` applies NAME.setpm.
    pub monitor_key: Option<String>,
    /// Sends 3160 and 1110 even when the device already reports the profile's id.
    pub force: bool,
    /// Sends 3160 even when the device already reports the profile's id, so the tracked
    /// eyes follow the profile after another program changed the device or the file.
    pub always_set_eye: bool,
    pub dry_run: bool,
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
    let options = SwitchOptions { eye, monitor_key: None, force: false, always_set_eye: false, dry_run };
    switch_with(io, store, name, options).await
}

/// [`switch`] with the calibration of a monitor, optionally forcing the apply.
pub async fn switch_with<E: External>(
    io: Io<E>,
    store: ProfileStore,
    name: String,
    options: SwitchOptions,
) -> OperationResult<SwitchResult, E> {
    let calibration = store.calibration_for(&name, options.monitor_key.as_deref())?;
    let wanted = calibration.calibration_id;
    let before = calibration::read_device_calibration_id(&io).await?;
    let applied = options.force || before != wanted;
    if applied || options.always_set_eye {
        io.call(device::set_enabled_eye(0, options.eye), None).await.map_err(OperationError::External)?;
    }
    if applied {
        let apply = device::calibration_apply(0, &calibration.blob)?;
        io.call(apply, Some(TRANSFER_TIMEOUT_S)).await.map_err(OperationError::External)?;
    }
    let after = calibration::read_device_calibration_id(&io).await?;
    if !options.dry_run {
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

    fn temporary_store(name: &str) -> ProfileStore {
        let directory = std::env::temp_dir().join(format!("tobii-gaze-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        ProfileStore::new(directory)
    }

    fn calibration(id: u32) -> LocalCalibration {
        let mut blob = vec![0u8; 32];
        blob[20..24].copy_from_slice(&id.to_le_bytes());
        LocalCalibration { calibration_id: id, blob }
    }

    #[test]
    fn legacy_profile_reads_as_both_eyes() {
        let store = temporary_store("legacy");
        store.save("Garulf", &calibration(1), false).unwrap();
        assert_eq!(store.tracked_eyes("Garulf").unwrap(), 0);
        assert_eq!(tracked_eye(store.tracked_eyes("Garulf").unwrap()), Eye::Both);
        assert_eq!(store.calibration_for("Garulf", None).unwrap(), calibration(1));
        let _ = std::fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn tracked_eyes_round_trip() {
        let store = temporary_store("eyes");
        store.save("Garulf", &calibration(1), false).unwrap();
        store.set_tracked_eyes("Garulf", 2).unwrap();
        assert_eq!(store.tracked_eyes("Garulf").unwrap(), 2);
        assert_eq!(tracked_eye(2), Eye::Right);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(store.directory.join("Garulf.json")).unwrap()).unwrap();
        assert_eq!(saved, serde_json::json!({"tracked_eyes": 2, "calibrations": {}}));
        assert!(matches!(store.set_tracked_eyes("Garulf", 3), Err(GazeError::Profile(_))));
        assert!(matches!(store.set_tracked_eyes("Nobody", 1), Err(GazeError::Profile(_))));
        assert_eq!(store.names().unwrap(), ["Garulf"]);
        let _ = std::fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn per_monitor_calibration_falls_back_to_legacy() {
        let store = temporary_store("monitors");
        store.save("Garulf", &calibration(1), false).unwrap();
        store.set_tracked_eyes("Garulf", 1).unwrap();
        let path = store.save_for("Garulf", Some("10AC4C41"), &calibration(2), false).unwrap();
        assert_eq!(path, store.directory.join("Garulf@10AC4C41.setpm"));
        assert_eq!(store.calibration_for("Garulf", Some("10AC4C41")).unwrap(), calibration(2));
        assert_eq!(store.calibration_for("Garulf", Some("DP-1")).unwrap(), calibration(1));
        assert_eq!(store.calibration_for("Garulf", None).unwrap(), calibration(1));
        assert_eq!(store.tracked_eyes("Garulf").unwrap(), 1);
        assert!(store.save_for("Garulf", Some("10AC4C41"), &calibration(3), false).is_err());
        store.save_for("Garulf", Some("10AC4C41"), &calibration(3), true).unwrap();
        assert_eq!(store.calibration_for("Garulf", Some("10AC4C41")).unwrap(), calibration(3));
        assert_eq!(store.names().unwrap(), ["Garulf"]);

        let first = store.save_for("New", Some("DP-1"), &calibration(4), false).unwrap();
        assert_eq!(first, store.directory.join("New.setpm"));
        assert_eq!(store.calibration_for("New", Some("DP-1")).unwrap(), calibration(4));
        assert_eq!(store.names().unwrap(), ["Garulf", "New"]);
        let _ = std::fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn a_calibration_for_a_monitor_is_its_own_file() {
        let store = temporary_store("has-calibration");
        assert!(!store.has_calibration_for("Garulf", None).unwrap());
        store.save("Garulf", &calibration(1), false).unwrap();
        assert!(store.has_calibration_for("Garulf", None).unwrap());
        assert!(!store.has_calibration_for("Garulf", Some("10AC4C41")).unwrap(), "the legacy file is no monitor's");
        store.save_for("Garulf", Some("10AC4C41"), &calibration(2), false).unwrap();
        assert!(store.has_calibration_for("Garulf", Some("10AC4C41")).unwrap());
        store.save_for("New", Some("DP-1"), &calibration(3), false).unwrap();
        assert!(
            store.has_calibration_for("New", Some("DP-1")).unwrap(),
            "a first calibration registered as NAME.setpm"
        );
        assert!(!store.has_calibration_for("New", Some("DP-2")).unwrap());
        let _ = std::fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn delete_refuses_current_and_guest() {
        let store = temporary_store("delete");
        for name in ["Garulf", GUEST, "Other", "Oth"] {
            store.save(name, &calibration(1), false).unwrap();
        }
        store.save_for("Other", Some("DP-1"), &calibration(2), false).unwrap();
        store.save_for("Oth", Some("DP-1"), &calibration(3), false).unwrap();
        store.set_tracked_eyes("Other", 2).unwrap();
        store.set_current("Garulf").unwrap();
        assert!(matches!(store.delete("Garulf"), Err(GazeError::Profile(_))));
        assert!(matches!(store.delete(GUEST), Err(GazeError::Profile(_))));
        assert!(matches!(store.delete("Nobody"), Err(GazeError::Profile(_))));

        store.delete("Other").unwrap();
        assert_eq!(store.names().unwrap(), ["Garulf", GUEST, "Oth"]);
        let mut left: Vec<String> = std::fs::read_dir(&store.directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["Garulf.setpm", "Guest.setpm", "Oth.json", "Oth.setpm", "Oth@DP-1.setpm", "current"]);
        let _ = std::fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn monitor_key_prefers_the_edid_id() {
        let directory = std::env::temp_dir().join(format!("tobii-gaze-display-record-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let record = directory.join(DISPLAY_RECORD);
        assert_eq!(recorded_monitor_key(&record), None);
        for (text, key) in [
            (r#"{"monitor":{"key":"DP-1","edid_id":"10AC4C41"}}"#, Some("10AC4C41")),
            (r#"{"monitor":{"key":"DP-1","edid_id":null}}"#, Some("DP-1")),
            (r#"{"monitor":null,"area":{}}"#, None),
            ("not json", None),
        ] {
            std::fs::write(&record, text).unwrap();
            assert_eq!(recorded_monitor_key(&record).as_deref(), key, "{text}");
        }
        assert_eq!(default_display_record(Some("/data"), None), PathBuf::from("/data/tobii-linux/display-setup.json"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn repr_quotes_like_python() {
        assert_eq!(python_repr("Nobody"), "'Nobody'");
        assert_eq!(python_repr("it's"), "\"it's\"");
        assert_eq!(python_repr("a\\b\n"), "'a\\\\b\\n'");
    }
}
