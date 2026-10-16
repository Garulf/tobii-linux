"""Named calibration profiles, kept and switched the way Tobii's Windows software does it.

On Windows each user profile is a registry key
HKLM\\SOFTWARE\\WOW6432Node\\Tobii\\EyeXConfig\\UserProfiles\\<name> holding
TrackedEyes and, per tracker and monitor, the 1100 calibration blob verbatim
(Calibrations\\<tracker uri>&<monitor id>\\Data). CurrentUserProfile names the
active one. When it changes, the EyeX engine sends tobii_set_enabled_eye
(3160) and tobii_calibration_apply (1110) with that profile's blob
(EyeTrackingEngine.TrySetCalibrationAndProfileSettings). See the profiles
section of docs/gaze-calibration.md.

Here a profile is a file NAME.setpm in calibration.setpm layout under
$XDG_DATA_HOME/tobii-linux/profiles, and the name of the active profile is
kept in the file `current` beside them.

The store and the procedures are crates/tobii-gaze/src/profiles.rs.
"""
from dataclasses import dataclass
import os
from pathlib import Path

from . import calibration as cal
from ._native import calibration_profiles as _cp
from .gaze_device import Eye, LocalCalibration, drive

PROFILE_SUFFIX = _cp.PROFILE_SUFFIX
CURRENT_FILE = _cp.CURRENT_FILE
MAX_NAME_LENGTH = _cp.MAX_NAME_LENGTH
NAME_PUNCTUATION = frozenset(' ()-')

ProfileError = _cp.ProfileError


def default_directory(environ=os.environ) -> Path:
    return Path(_cp.default_directory(environ.get('XDG_DATA_HOME'), environ.get('HOME')))


def check_name(name: str) -> str:
    """ConfigurationManager.ValidateUserProfileName: letters, digits, space, '(', ')', '-'; at most 255."""
    return _cp.check_name(name)


@dataclass(frozen=True)
class Profile:
    name: str
    path: Path
    calibration: LocalCalibration


def _profile(name, path, calibration):
    return Profile(name, Path(path), LocalCalibration(*calibration))


class ProfileStore:
    def __init__(self, directory=None):
        self.directory = Path(directory) if directory else default_directory()

    def path(self, name) -> Path:
        return Path(_cp.profile_path(self.directory, name))

    def names(self):
        return _cp.profile_names(self.directory)

    def load(self, name) -> Profile:
        return _profile(*_cp.load_profile(self.directory, name))

    def save(self, name, calibration: LocalCalibration, overwrite=False) -> Path:
        return Path(_cp.save_profile(self.directory, name, calibration, overwrite))

    def current(self):
        return _cp.current_profile(self.directory)

    def set_current(self, name):
        _cp.set_current_profile(self.directory, name)


def save_from_device(session, store: ProfileStore, name, overwrite=False) -> Profile:
    """Read the device calibration (1490, 1100, 1490) into a profile and verify it; sends no write."""
    return _profile(*drive(_cp.save_from_device(store.directory, name, overwrite), session))


@dataclass(frozen=True)
class SwitchResult:
    profile: str
    device_id_before: int
    device_id_after: int
    applied: bool


def switch(session, store: ProfileStore, name, eye=Eye.BOTH) -> SwitchResult:
    """Make a profile active: 3160 enabled eye, then 1110 with its blob, then 1490 to confirm the id.

The engine sends both on every profile change; the apply is skipped here when
the device already reports the profile's id, as the runtime's own start check
does. Pass a DryRunSession to report the writes instead of sending them.
"""
    dry_run = isinstance(session, cal.DryRunSession)
    return SwitchResult(*drive(_cp.switch(store.directory, name, int(Eye(eye)), dry_run), session))
