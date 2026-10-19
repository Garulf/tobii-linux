//! Tobii Experience for Linux. This part is toolkit free: Tobii's device status to tray icon
//! mapping, the menu texts, and the link to `tobii-linux serve`.

pub mod link;
pub mod status;

pub use link::{Link, LinkUpdate};
pub use status::{
    DeviceStatus, IconName, IconState, OPEN_TEXT, QUIT_TEXT, device_status, enable_disable_text, icon_info, icon_state,
};
