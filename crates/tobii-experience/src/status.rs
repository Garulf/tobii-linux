//! Tobii's mapping from the device status to the tray icon, tooltip and menu texts
//! (docs/experience-app.md sections 1.1, 1.2 and 1.4).

use tobii_control::Status;

pub const OPEN_TEXT: &str = "Open Tobii Experience";
pub const QUIT_TEXT: &str = "Quit";

/// Tobii's `EyeTrackingDeviceStatus` as far as the Linux service can report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    Initializing,
    Tracking,
    TrackingPaused,
    DeviceNotConnected,
    InvalidConfiguration,
    Configuring,
    Error,
}

/// Tobii's `ExperienceNotifyIconState`, in its enum order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    GetStarted,
    Tracking,
    Error,
    TrackingPaused,
    InvalidConfiguration,
    Initializing,
    Disconnected,
    Configuring,
}

/// Tobii's four tray icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    Progress,
    GetStarted,
    Disabled,
    Enabled,
}

/// The device status for the latest service status; `None` means the service is unreachable.
///
/// "Initializing" is not derived here: the app shows [`DeviceStatus::Initializing`] itself
/// until the first [`crate::LinkUpdate::Status`] after it starts the link, since the service
/// sends a status right after the connection opens.
pub fn device_status(status: Option<&Status>) -> DeviceStatus {
    let Some(status) = status else { return DeviceStatus::Error };
    if status.released {
        DeviceStatus::Configuring
    } else if !status.attached {
        DeviceStatus::DeviceNotConnected
    } else if !status.enabled {
        DeviceStatus::TrackingPaused
    } else if !status.display_area || status.profile.is_none() {
        DeviceStatus::InvalidConfiguration
    } else {
        DeviceStatus::Tracking
    }
}

/// The icon state for a device status; quitting shows Configuring.
pub fn icon_state(status: DeviceStatus, quitting: bool) -> IconState {
    if quitting {
        return IconState::Configuring;
    }
    match status {
        DeviceStatus::Initializing => IconState::Initializing,
        DeviceStatus::Tracking => IconState::Tracking,
        DeviceStatus::TrackingPaused => IconState::TrackingPaused,
        DeviceStatus::DeviceNotConnected => IconState::Disconnected,
        DeviceStatus::InvalidConfiguration => IconState::InvalidConfiguration,
        DeviceStatus::Configuring => IconState::Configuring,
        DeviceStatus::Error => IconState::Error,
    }
}

/// The icon and tooltip Tobii shows for an icon state.
pub fn icon_info(state: IconState) -> (IconName, &'static str) {
    match state {
        IconState::GetStarted => (IconName::GetStarted, "Tobii Experience - Get started"),
        IconState::Configuring => (IconName::Progress, "Tobii Experience - Configuring…"),
        IconState::Disconnected => (IconName::Disabled, "Tobii Experience - No sensor connected"),
        IconState::Error => (IconName::Disabled, "Tobii Experience - Please troubleshoot"),
        IconState::Initializing => (IconName::Progress, "Tobii Experience - Loading…"),
        IconState::InvalidConfiguration => (IconName::Disabled, "Tobii Experience - Action needed"),
        IconState::Tracking => (IconName::Enabled, "Tobii Experience - In use"),
        IconState::TrackingPaused => (IconName::Disabled, "Tobii Experience - Disabled"),
    }
}

/// The text of the enable or disable menu item for the current enabled state.
pub fn enable_disable_text(enabled: bool) -> &'static str {
    if enabled { "Disable Tobii Experience" } else { "Enable Tobii Experience" }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn every_device_status_maps_like_tobii() {
        use DeviceStatus as D;
        use IconName::{Disabled, Enabled, Progress};
        let table = [
            (D::Initializing, IconState::Initializing, Progress, "Tobii Experience - Loading\u{2026}"),
            (D::Tracking, IconState::Tracking, Enabled, "Tobii Experience - In use"),
            (D::TrackingPaused, IconState::TrackingPaused, Disabled, "Tobii Experience - Disabled"),
            (D::DeviceNotConnected, IconState::Disconnected, Disabled, "Tobii Experience - No sensor connected"),
            (D::InvalidConfiguration, IconState::InvalidConfiguration, Disabled, "Tobii Experience - Action needed"),
            (D::Configuring, IconState::Configuring, Progress, "Tobii Experience - Configuring\u{2026}"),
            (D::Error, IconState::Error, Disabled, "Tobii Experience - Please troubleshoot"),
        ];
        for (device, state, icon, tooltip) in table {
            assert_eq!(icon_state(device, false), state, "{device:?}");
            assert_eq!(icon_info(state), (icon, tooltip), "{state:?}");
            let quitting = icon_state(device, true);
            assert_eq!(quitting, IconState::Configuring, "{device:?} while quitting");
            assert_eq!(icon_info(quitting), (Progress, "Tobii Experience - Configuring\u{2026}"));
        }
        assert_eq!(icon_info(IconState::GetStarted), (IconName::GetStarted, "Tobii Experience - Get started"));
    }

    #[test]
    fn menu_texts_follow_enabled() {
        assert_eq!(enable_disable_text(true), "Disable Tobii Experience");
        assert_eq!(enable_disable_text(false), "Enable Tobii Experience");
        assert_eq!(OPEN_TEXT, "Open Tobii Experience");
        assert_eq!(QUIT_TEXT, "Quit");
    }

    #[test]
    fn no_service_is_error() {
        assert_eq!(device_status(None), DeviceStatus::Error);
    }

    #[test]
    fn service_status_maps_in_tobiis_order() {
        let tracking = status();
        assert_eq!(device_status(Some(&tracking)), DeviceStatus::Tracking);

        let released = Status { released: true, attached: false, ..status() };
        assert_eq!(device_status(Some(&released)), DeviceStatus::Configuring);

        let detached = Status { attached: false, ..status() };
        assert_eq!(device_status(Some(&detached)), DeviceStatus::DeviceNotConnected);

        let paused = Status { enabled: false, display_area: false, ..status() };
        assert_eq!(device_status(Some(&paused)), DeviceStatus::TrackingPaused);

        let no_area = Status { display_area: false, ..status() };
        assert_eq!(device_status(Some(&no_area)), DeviceStatus::InvalidConfiguration);

        let no_profile = Status { profile: None, ..status() };
        assert_eq!(device_status(Some(&no_profile)), DeviceStatus::InvalidConfiguration);
    }
}
