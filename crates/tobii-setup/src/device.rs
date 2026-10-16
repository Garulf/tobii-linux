//! The tracker session the setup runs on: a USB tracker, or the simulated one, optionally in
//! dry-run mode.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tobii_device::usb_session::CONTROL_TIMEOUT_MS;
use tobii_device::{Channel, ChannelError, DeviceError, TrackerSession};
use tobii_gaze::calibration::READ_COMMANDS;
use tobii_protocol::Message;
use tobii_protocol::commands::{RESPONSE_KIND, command};
use tobii_protocol::ttp::Decoder;
use tobii_usb::{UsbChannel, open_transport, probe};

pub type Session = TrackerSession<Box<dyn Channel>>;

/// Whether a dry run sends `command`: the reads of a calibration dry run, the mounting
/// geometry and the gaze subscription.
pub fn dry_run_allows(command: u32) -> bool {
    READ_COMMANDS.contains(&command)
        || [command::GET_GEOMETRY_MOUNTING, command::SUBSCRIBE_STREAM, command::UNSUBSCRIBE_STREAM].contains(&command)
}

/// Reports a request a dry run held back: its command and payload in hex.
pub type WouldSend = Box<dyn FnMut(&Message) + Send>;

/// A channel that, once armed, sends only [`dry_run_allows`] requests and answers every
/// other one itself with an empty success reply, as the calibration's DryRunSession does.
pub struct DryRun<C> {
    inner: C,
    armed: Arc<AtomicBool>,
    requests: Decoder,
    answers: Vec<u8>,
    report: WouldSend,
}

impl<C> std::fmt::Debug for DryRun<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DryRun").field("armed", &self.armed.load(Ordering::Relaxed)).finish_non_exhaustive()
    }
}

impl<C: Channel> DryRun<C> {
    /// The channel and the switch that arms it (after the session's startup requests).
    pub fn new(inner: C, report: WouldSend) -> (Self, Arc<AtomicBool>) {
        let armed = Arc::new(AtomicBool::new(false));
        let channel =
            Self { inner, armed: Arc::clone(&armed), requests: Decoder::default(), answers: Vec::new(), report };
        (channel, armed)
    }
}

impl<C: Channel> Channel for DryRun<C> {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        if !self.armed.load(Ordering::Relaxed) {
            return self.inner.write(data);
        }
        let bad = |error| ChannelError::new(false, error);
        for request in self.requests.feed(data).map_err(bad)? {
            if dry_run_allows(request.command) {
                self.inner.write(&request.encode().map_err(bad)?)?;
                continue;
            }
            (self.report)(&request);
            let reply = Message::new(RESPONSE_KIND, request.request_id, 0, request.command, 0, Vec::new());
            self.answers.extend(reply.encode().map_err(bad)?);
        }
        Ok(())
    }

    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        if !self.answers.is_empty() {
            return Ok(std::mem::take(&mut self.answers));
        }
        self.inner.read(timeout_ms)
    }
}

/// Opens the `index`th tracker as `tobii_device::usb_session` does, then reads the camera
/// calibration in the head-pose realm as the calibration tools do. With `dry_run` every
/// later write goes to `report` instead of the device.
pub fn open_usb(index: usize, channel: u32, dry_run: Option<WouldSend>) -> Result<Session, DeviceError> {
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
    let (boxed, armed): (Box<dyn Channel>, _) = match dry_run {
        Some(report) => {
            let (wrapped, armed) = DryRun::new(usb_channel, report);
            (Box::new(wrapped), Some(armed))
        }
        None => (Box::new(usb_channel), None),
    };
    let mut session = TrackerSession::new(boxed);
    session.initialize()?;
    session.camera_calibration(0)?;
    if let Some(armed) = armed {
        armed.store(true, Ordering::Relaxed);
    }
    Ok(session)
}

/// A session on any channel, dry-run from the start when `dry_run` is given.
pub fn session_on<C: Channel + 'static>(channel: C, dry_run: Option<WouldSend>) -> Session {
    match dry_run {
        Some(report) => {
            let (wrapped, armed) = DryRun::new(channel, report);
            armed.store(true, Ordering::Relaxed);
            TrackerSession::new(Box::new(wrapped))
        }
        None => TrackerSession::new(Box::new(channel)),
    }
}

/// A request as the dry-run report shows it: command and payload in hex.
pub fn describe(message: &Message) -> serde_json::Value {
    let payload: String = message.payload.iter().map(|b| format!("{b:02x}")).collect();
    serde_json::json!({ "command": message.command, "payload": payload })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulated::{SimulatedTracker, Simulation};
    use std::sync::Mutex;
    use tobii_gaze::device::{self as gd, gaze_command as gc};

    #[test]
    fn a_dry_run_answers_writes_itself_and_forwards_reads() {
        let tracker = SimulatedTracker::new(Simulation::default());
        let state = Arc::clone(&tracker.state);
        let held = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&held);
        let mut session = session_on(tracker, Some(Box::new(move |m: &Message| sink.lock().unwrap().push(m.command))));
        session.try_call(|id| tobii_protocol::commands::header_only_request(id, command::GET_DISPLAY_AREA)).unwrap();
        session.call(|id| gd::header_only(id, gc::CALIBRATION_START).unwrap()).unwrap();
        assert_eq!(state.lock().unwrap().commands(), [command::GET_DISPLAY_AREA]);
        assert_eq!(*held.lock().unwrap(), [gc::CALIBRATION_START]);
    }
}
