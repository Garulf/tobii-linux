//! A started session on a connected tracker.

use std::sync::Arc;

use tobii_usb::{ProbedDevice, UsbChannel, open_transport, probe};

use crate::error::{DeviceError, Result};
use crate::session::TrackerSession;

pub const CONTROL_TIMEOUT_MS: u32 = 1000;

/// Opens the `index`th Eye Tracker 5, its vendor channel and a started session on it.
/// Dropping the session closes the channel and releases the device.
pub fn usb_session(index: usize, channel: u32) -> Result<(ProbedDevice, TrackerSession<UsbChannel>)> {
    let devices = probe()?;
    let Some(device) = devices.get(index).cloned() else {
        return Err(DeviceError::NoTracker(if devices.is_empty() {
            "No Eye Tracker 5 is visible over USB".into()
        } else {
            format!("Tracker index {index} requested, {} visible", devices.len())
        }));
    };
    let transport = Arc::new(open_transport(device.bus, device.address)?);
    let mut usb_channel = UsbChannel::new(transport, channel, CONTROL_TIMEOUT_MS)?;
    usb_channel.open()?;
    let mut session = TrackerSession::new(usb_channel);
    session.initialize()?;
    Ok((device, session))
}
