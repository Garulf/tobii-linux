//! Tobii's Eye tracking page, the Store app's `SettingsPage` (docs/experience-app.md
//! section 4): its texts, the page state derived from the service status, the
//! `RefreshAvailability` rules and the setup flows its buttons start. The GTK window
//! (`widgets`) is built on these toolkit-free parts.

use std::time::Duration;

use tobii_control::Status;
use tobii_gaze::profiles::GUEST;

use crate::status::DeviceStatus;

pub mod profiles;
pub mod style;
#[cfg(feature = "gtk")]
mod widgets;

pub use profiles::{PageAction, ProfileItem, ProfileList};
#[cfg(feature = "gtk")]
pub use widgets::page;
#[cfg(feature = "gtk")]
pub(crate) use widgets::{PageEvent, PageModel, on_page_event, on_service_update, on_shutdown, open_window};

pub const SWITCH_TITLE: &str = "Tobii Experience";
pub const ON_TEXT: &str = "ON";
pub const OFF_TEXT: &str = "OFF";
pub const IMPROVE_TITLE: &str = "Improve my calibration";
pub const IMPROVE_BODY: &str = "If the light conditions change or if you experience less tracker precision, you might \
                                benefit from improving your calibration.";
pub const IMPROVE_BUTTON: &str = "Improve calibration";
pub const PREVIEW_TITLE: &str = "Preview my gaze";
pub const PREVIEW_BODY: &str = "Shows you a visual trail of your gaze";
pub const EYES_TITLE: &str = "Select eyes to detect";
pub const EYES_BODY: &str = "If you typically squint or have poor sight\u{a0}in one eye, you can make the eye tracker \
                             detect one eye only.";
pub const BOTH_EYES: &str = "Both eyes";
pub const LEFT_EYE: &str = "Left eye only";
pub const RIGHT_EYE: &str = "Right eye only";
pub const SCREEN_TITLE: &str = "Change screen";
pub const SCREEN_BODY: &str =
    "If you move the sensor to a different monitor, you\u{2019}ll need to set up the new display.";
pub const SCREEN_BUTTON: &str = "Set up display";
pub const PROFILE_TITLE: &str = "Change user profile";
pub const PROFILE_BODY: &str = "Each user needs a unique profile. You may also need separate profiles to track your eyes \
                                with or without glasses/contacts. Press Ctrl+Shft+F10 for a temporary guest profile.";
pub const NEW_PROFILE_BUTTON: &str = "Create new profile";
/// Hard-coded in Tobii's list item template, not from its resources.
pub const UNDO_TEXT: &str = "Undo";

/// How long Tobii keeps the page disabled after turning eye tracking off
/// (`SetEyeTrackingEnabled`); turning it on releases the page at once.
pub const DISABLE_DELAY: Duration = Duration::from_millis(500);

/// Tobii's `TrackingDeviceState`, in its enum order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    Tracking,
    TrackingDisabled,
    Disconnected,
    Initializing,
    InvalidMonitorSetup,
    NoCalibrationProfile,
    InvalidCalibration,
    TrackedMonitorNotAvailableForBuiltInTracker,
    Configuring,
    Error,
    ExclusiveMode,
    FirmwareUpdateInProgress,
    InvalidMonitorRotation,
}

impl PageState {
    pub const ALL: [PageState; 13] = [
        PageState::Tracking,
        PageState::TrackingDisabled,
        PageState::Disconnected,
        PageState::Initializing,
        PageState::InvalidMonitorSetup,
        PageState::NoCalibrationProfile,
        PageState::InvalidCalibration,
        PageState::TrackedMonitorNotAvailableForBuiltInTracker,
        PageState::Configuring,
        PageState::Error,
        PageState::ExclusiveMode,
        PageState::FirmwareUpdateInProgress,
        PageState::InvalidMonitorRotation,
    ];
}

/// Tobii's `CapabilityStatus` of the gaze stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GazeCapability {
    NotAvailable,
    HasBeenAvailable,
    AvailableButNotReadyToUse,
    ReadyToUse,
}

impl GazeCapability {
    /// Tobii's `HasDeviceWithGazeCapabilityEverBeenConnected`.
    pub fn has_gaze(self) -> bool {
        self != GazeCapability::NotAvailable
    }

    fn available(self) -> bool {
        matches!(self, GazeCapability::AvailableButNotReadyToUse | GazeCapability::ReadyToUse)
    }
}

/// Tobii's `SystemEnabled`: the persisted on/off value, read as true when it is missing, so
/// while the app has no status yet or the service is unreachable.
pub fn system_enabled(status: Option<&Status>) -> bool {
    status.is_none_or(|status| status.enabled)
}

/// Tobii's `UpdateState` step 6 over the Linux device status: the device status with the
/// tracking-enabled flag (`Disabled` column), the configuration status for
/// `InvalidConfiguration` (no display area first, then no profile). The Linux service has
/// no producer for `InvalidCalibration`, the firmware, rotation and exclusive-mode states.
pub fn page_state(device: DeviceStatus, status: Option<&Status>) -> PageState {
    let enabled = status.is_some_and(|status| status.enabled);
    match device {
        DeviceStatus::Initializing => PageState::Initializing,
        DeviceStatus::Error => PageState::Error,
        DeviceStatus::TrackingPaused => PageState::TrackingDisabled,
        _ if !enabled => PageState::TrackingDisabled,
        DeviceStatus::Tracking => PageState::Tracking,
        DeviceStatus::DeviceNotConnected => PageState::Disconnected,
        DeviceStatus::Configuring => PageState::Configuring,
        DeviceStatus::InvalidConfiguration if !status.is_some_and(|status| status.display_area) => {
            PageState::InvalidMonitorSetup
        }
        DeviceStatus::InvalidConfiguration => PageState::NoCalibrationProfile,
    }
}

/// The gaze capability as far as the Linux service shows it: ready while tracking, available
/// while a tracker session runs, and "has been available" once a tracker was ever attached
/// (`has_been_connected`, Tobii's `GazeTrackerHasBeenConnected`).
pub fn gaze_capability(state: PageState, status: Option<&Status>, has_been_connected: bool) -> GazeCapability {
    match status {
        Some(status) if status.attached && state == PageState::Tracking => GazeCapability::ReadyToUse,
        Some(status) if status.attached => GazeCapability::AvailableButNotReadyToUse,
        _ if has_been_connected => GazeCapability::HasBeenAvailable,
        _ => GazeCapability::NotAvailable,
    }
}

/// Which sections are shown (`SettingsPageViewModel` visibility properties).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visibility {
    /// `IsProfilesSectionVisible`: sections 3 to 7, and the Linux profile header.
    pub profiles_section: bool,
    pub calibration: bool,
    pub gaze_trace: bool,
    pub tracked_eyes: bool,
    /// `ShowDisplaySetup`: the Eye Tracker 5 is a peripheral.
    pub display_setup: bool,
    pub user_profile: bool,
}

pub fn visibility(capability: GazeCapability) -> Visibility {
    let has_gaze = capability.has_gaze();
    Visibility {
        profiles_section: has_gaze,
        calibration: has_gaze,
        gaze_trace: has_gaze,
        tracked_eyes: has_gaze,
        display_setup: true,
        user_profile: has_gaze,
    }
}

/// Which controls are enabled (`RefreshAvailability` and the preview toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Availability {
    pub improve_calibration: bool,
    /// `CanChangeCalibrationProfile`: the profile list (switch, delete, undo).
    pub change_profile: bool,
    pub create_new_profile: bool,
    pub setup_display: bool,
    /// `CanSelectTrackedEyes`: the fold-out button; the radios follow only the busy marker.
    pub select_eyes: bool,
    pub enable_disable: bool,
    pub preview: bool,
}

/// Tobii's `RefreshAvailability` for state `state`, the gaze capability, the current
/// profile and the persisted tracking-enabled flag (`SystemEnabled`).
pub fn availability(
    state: PageState,
    capability: GazeCapability,
    profile: Option<&str>,
    system_enabled: bool,
) -> Availability {
    use PageState as S;
    let gaze = capability.available();
    let can_calibrate = gaze && matches!(state, S::InvalidCalibration | S::Tracking);
    let can_create = gaze && matches!(state, S::InvalidCalibration | S::NoCalibrationProfile | S::Tracking);
    let blocks_switch = matches!(
        state,
        S::Error
            | S::Configuring
            | S::ExclusiveMode
            | S::Initializing
            | S::FirmwareUpdateInProgress
            | S::InvalidMonitorRotation
    );
    Availability {
        improve_calibration: can_calibrate && profile != Some(GUEST),
        change_profile: can_calibrate,
        create_new_profile: can_create,
        setup_display: gaze && (state == S::InvalidMonitorSetup || can_create),
        select_eyes: gaze && profile.is_some_and(|name| !name.is_empty()),
        enable_disable: !system_enabled || !blocks_switch,
        preview: state == S::Tracking,
    }
}

/// The configuration flows the page starts in `tobii-linux-setup`, by Tobii's arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupFlow {
    Calibrate,
    CreateNewProfile,
    SetupDisplay,
    ResolveConfiguration,
}

impl SetupFlow {
    /// `Tobii.Configuration.exe`'s flag for the flow.
    pub fn argument(self) -> &'static str {
        match self {
            SetupFlow::Calibrate => "-C",
            SetupFlow::CreateNewProfile => "-N",
            SetupFlow::SetupDisplay => "-S",
            SetupFlow::ResolveConfiguration => "-R",
        }
    }

    /// Set up display: `ResolveConfiguration` when the state is `InvalidMonitorSetup`.
    pub fn display(state: PageState) -> SetupFlow {
        if state == PageState::InvalidMonitorSetup { SetupFlow::ResolveConfiguration } else { SetupFlow::SetupDisplay }
    }

    /// The page action the flow's button stands for.
    pub fn action(self) -> PageAction {
        match self {
            SetupFlow::Calibrate => PageAction::ImproveCalibration,
            SetupFlow::CreateNewProfile => PageAction::CreateNewProfile,
            SetupFlow::SetupDisplay | SetupFlow::ResolveConfiguration => PageAction::SetupDisplay,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::guest::{SETUP_BINARY, SetupProcess};

    fn status() -> Status {
        Status {
            attached: true,
            enabled: true,
            released: false,
            display_area: true,
            profile: Some("Garulf".into()),
            calibration_matches_profile: true,
        }
    }

    fn state_of(status: Option<Status>) -> PageState {
        page_state(crate::status::device_status(status.as_ref()), status.as_ref())
    }

    #[test]
    fn page_state_follows_the_service_status() {
        assert_eq!(page_state(DeviceStatus::Initializing, None), PageState::Initializing);
        assert_eq!(state_of(None), PageState::Error, "an unreachable service");
        assert_eq!(state_of(Some(status())), PageState::Tracking);
        assert_eq!(state_of(Some(Status { enabled: false, ..status() })), PageState::TrackingDisabled);
        assert_eq!(state_of(Some(Status { attached: false, ..status() })), PageState::Disconnected);
        let detached_off = Status { attached: false, enabled: false, ..status() };
        assert_eq!(state_of(Some(detached_off)), PageState::TrackingDisabled);
        let released = Status { attached: false, released: true, ..status() };
        assert_eq!(state_of(Some(released.clone())), PageState::Configuring);
        assert_eq!(state_of(Some(Status { enabled: false, ..released })), PageState::TrackingDisabled);
        let no_area = Status { display_area: false, ..status() };
        assert_eq!(state_of(Some(no_area)), PageState::InvalidMonitorSetup);
        let neither = Status { display_area: false, profile: None, ..status() };
        assert_eq!(state_of(Some(neither)), PageState::InvalidMonitorSetup);
        assert_eq!(state_of(Some(Status { profile: None, ..status() })), PageState::NoCalibrationProfile);
    }

    #[test]
    fn unknown_enabled_state_reads_as_enabled() {
        assert!(system_enabled(None));
        assert!(system_enabled(Some(&status())));
        assert!(!system_enabled(Some(&Status { enabled: false, ..status() })));
        for state in [PageState::Initializing, PageState::Error] {
            let flags = availability(state, GazeCapability::NotAvailable, None, system_enabled(None));
            assert!(!flags.enable_disable, "{state:?}: the switch is insensitive as Tobii's");
        }
    }

    #[test]
    fn gaze_capability_remembers_a_connected_tracker() {
        let tracking = status();
        assert_eq!(gaze_capability(PageState::Tracking, Some(&tracking), false), GazeCapability::ReadyToUse);
        let paused = Status { enabled: false, ..status() };
        let available = GazeCapability::AvailableButNotReadyToUse;
        assert_eq!(gaze_capability(PageState::TrackingDisabled, Some(&paused), false), available);
        let detached = Status { attached: false, ..status() };
        let state = PageState::Disconnected;
        assert_eq!(gaze_capability(state, Some(&detached), true), GazeCapability::HasBeenAvailable);
        assert_eq!(gaze_capability(state, Some(&detached), false), GazeCapability::NotAvailable);
        assert_eq!(gaze_capability(PageState::Error, None, true), GazeCapability::HasBeenAvailable);

        assert!(visibility(GazeCapability::HasBeenAvailable).profiles_section);
        let none = visibility(GazeCapability::NotAvailable);
        assert_eq!(
            none,
            Visibility {
                profiles_section: false,
                calibration: false,
                gaze_trace: false,
                tracked_eyes: false,
                display_setup: true,
                user_profile: false
            }
        );
    }

    /// The table in docs/experience-app.md section 4.3, columns Improve calibration, Profile
    /// list, Create new profile, Set up display, Select eyes and Switch (with SystemEnabled
    /// true, then false).
    #[test]
    fn availability_follows_refresh_rules() {
        use PageState as S;
        let table = [
            (S::Tracking, [true, true, true, true, true, true]),
            (S::InvalidCalibration, [true, true, true, true, true, true]),
            (S::NoCalibrationProfile, [false, false, true, true, true, true]),
            (S::InvalidMonitorSetup, [false, false, false, true, true, true]),
            (S::TrackedMonitorNotAvailableForBuiltInTracker, [false, false, false, false, true, true]),
            (S::TrackingDisabled, [false, false, false, false, true, true]),
            (S::Disconnected, [false, false, false, false, true, true]),
            (S::Initializing, [false, false, false, false, true, false]),
            (S::Configuring, [false, false, false, false, true, false]),
            (S::Error, [false, false, false, false, true, false]),
            (S::ExclusiveMode, [false, false, false, false, true, false]),
            (S::FirmwareUpdateInProgress, [false, false, false, false, true, false]),
            (S::InvalidMonitorRotation, [false, false, false, false, true, false]),
        ];
        for state in PageState::ALL {
            assert_eq!(table.iter().filter(|(row, _)| *row == state).count(), 1, "{state:?} has one row");
        }
        let ready = GazeCapability::ReadyToUse;
        for (state, [improve, list, create, display, eyes, switch]) in table {
            let flags = availability(state, ready, Some("Garulf"), true);
            let expected = Availability {
                improve_calibration: improve,
                change_profile: list,
                create_new_profile: create,
                setup_display: display,
                select_eyes: eyes,
                enable_disable: switch,
                preview: state == S::Tracking,
            };
            assert_eq!(flags, expected, "{state:?}");
            assert!(availability(state, ready, Some("Garulf"), false).enable_disable, "{state:?}: no+ is yes when off");

            let guest = availability(state, ready, Some("Guest"), true);
            assert!(!guest.improve_calibration, "{state:?}: Guest cannot improve its calibration");
            assert_eq!(guest.change_profile, list, "{state:?}");
            assert!(!availability(state, ready, None, true).select_eyes, "{state:?}: no current profile");
            assert!(!availability(state, ready, Some(""), true).select_eyes, "{state:?}: empty profile name");
            assert_eq!(availability(state, GazeCapability::AvailableButNotReadyToUse, Some("Garulf"), true), flags);

            for unavailable in [GazeCapability::NotAvailable, GazeCapability::HasBeenAvailable] {
                let off = availability(state, unavailable, Some("Garulf"), true);
                let expected = Availability {
                    improve_calibration: false,
                    change_profile: false,
                    create_new_profile: false,
                    setup_display: false,
                    select_eyes: false,
                    enable_disable: switch,
                    preview: state == S::Tracking,
                };
                assert_eq!(off, expected, "{state:?} with {unavailable:?}");
            }
        }
    }

    #[test]
    fn buttons_start_setup_with_tobiis_arguments() {
        let launcher = SetupProcess::new(PathBuf::from(SETUP_BINARY), crate::cli::DEFAULT_ADDR);
        let command_line = |flow: SetupFlow| {
            let command = launcher.command(&[flow.argument()]);
            let mut line = vec![command.get_program().to_string_lossy().into_owned()];
            line.extend(command.get_args().map(|argument| argument.to_string_lossy().into_owned()));
            line
        };
        assert_eq!(command_line(SetupFlow::Calibrate), ["tobii-linux-setup", "-C", "--service", "127.0.0.1:7760"]);
        assert_eq!(
            command_line(SetupFlow::CreateNewProfile),
            ["tobii-linux-setup", "-N", "--service", "127.0.0.1:7760"]
        );
        for state in PageState::ALL {
            let expected = if state == PageState::InvalidMonitorSetup { "-R" } else { "-S" };
            assert_eq!(
                command_line(SetupFlow::display(state)),
                ["tobii-linux-setup", expected, "--service", "127.0.0.1:7760"],
                "{state:?}"
            );
        }
        assert_eq!(SetupFlow::Calibrate.action(), PageAction::ImproveCalibration);
        assert_eq!(SetupFlow::CreateNewProfile.action(), PageAction::CreateNewProfile);
        assert_eq!(SetupFlow::ResolveConfiguration.action(), PageAction::SetupDisplay);
    }
}
