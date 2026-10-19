//! Tobii Experience for Linux: Tobii's device status to tray icon mapping, the menu texts,
//! the link to `tobii-linux serve`, the command line, settings, quit and guest. The GTK 4
//! app shell (`app`, feature `gtk`) builds on these toolkit-free parts.

#[cfg(feature = "gtk")]
pub mod app;
pub mod cli;
pub mod guest;
pub mod link;
pub mod quit;
pub mod settings;
pub mod status;
pub mod tray;

pub use link::{Link, LinkUpdate};
pub use settings::Settings;
pub use status::{
    DeviceStatus, IconName, IconState, OPEN_TEXT, QUIT_TEXT, device_status, enable_disable_text, icon_info, icon_state,
};
pub use tray::{Tray, TrayAction};
