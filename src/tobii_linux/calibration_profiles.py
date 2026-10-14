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
"""
from dataclasses import dataclass
import os
from pathlib import Path

from . import calibration as cal
from . import gaze_device as gd
from .gaze_device import Eye, LocalCalibration

PROFILE_SUFFIX = '.setpm'
CURRENT_FILE = 'current'
MAX_NAME_LENGTH = 255
NAME_PUNCTUATION = frozenset(' ()-')


class ProfileError(ValueError):
    pass


def default_directory(environ=os.environ) -> Path:
    data_home = environ.get('XDG_DATA_HOME') or str(Path(environ.get('HOME', '~')).expanduser()/'.local/share')
    return Path(data_home)/'tobii-linux'/'profiles'


def check_name(name: str) -> str:
    """ConfigurationManager.ValidateUserProfileName: letters, digits, space, '(', ')', '-'; at most 255."""
    if not name or not name.strip() or len(name) > MAX_NAME_LENGTH:
        raise ProfileError('A profile name needs 1 to 255 characters')
    invalid = sorted({c for c in name if not c.isalnum() and c not in NAME_PUNCTUATION})
    if invalid:
        raise ProfileError(f'Invalid characters in profile name: {"".join(invalid)!r}')
    return name


@dataclass(frozen=True)
class Profile:
    name: str
    path: Path
    calibration: LocalCalibration


class ProfileStore:
    def __init__(self, directory=None):
        self.directory = Path(directory) if directory else default_directory()

    def path(self, name) -> Path:
        return self.directory/f'{check_name(name)}{PROFILE_SUFFIX}'

    def names(self):
        if not self.directory.is_dir():
            return []
        return sorted(p.name[:-len(PROFILE_SUFFIX)] for p in self.directory.glob(f'*{PROFILE_SUFFIX}'))

    def load(self, name) -> Profile:
        path = self.path(name)
        if not path.exists():
            raise ProfileError(f'No profile named {name!r} in {self.directory}')
        return Profile(name, path, cal.load_backup(path))

    def save(self, name, calibration: LocalCalibration, overwrite=False) -> Path:
        self.directory.mkdir(parents=True, exist_ok=True)
        return cal.save_backup(self.path(name), calibration, overwrite=overwrite)

    def current(self):
        marker = self.directory/CURRENT_FILE
        name = marker.read_text().strip() if marker.exists() else ''
        return name or None

    def set_current(self, name):
        self.directory.mkdir(parents=True, exist_ok=True)
        (self.directory/CURRENT_FILE).write_text(check_name(name) + '\n')


def save_from_device(session, store: ProfileStore, name, overwrite=False) -> Profile:
    """Read the device calibration (1490, 1100, 1490) into a profile and verify it; sends no write."""
    calibration = cal.backup_calibration(session)
    path = store.save(name, calibration, overwrite=overwrite)
    check = cal.verify_backup(session, cal.load_backup(path))
    if not check.ok:
        raise ProfileError(f'Saved profile does not match the device (ids {check.ids_match}, '
                           f'blob {check.blob_matches})')
    return Profile(name, path, calibration)


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
    profile = store.load(name)
    before = cal.read_device_calibration_id(session)
    applied = before != profile.calibration.calibration_id
    if applied:
        session.call(lambda rid: gd.set_enabled_eye(rid, eye))
        session.call(lambda rid: gd.calibration_apply(rid, profile.calibration.blob),
                     timeout_s=cal.TRANSFER_TIMEOUT_S)
    after = cal.read_device_calibration_id(session)
    if not isinstance(session, cal.DryRunSession):
        if after != profile.calibration.calibration_id:
            raise ProfileError(f'Device reports calibration id {after:#x} after switching to {name!r} '
                               f'({profile.calibration.calibration_id:#x})')
        store.set_current(name)
    return SwitchResult(name, before, after, applied)
