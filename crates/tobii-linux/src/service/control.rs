//! The private control channel of the experience app and `tobii-linux-setup` on the relay
//! port (docs/control-channel.md): status, on/off, gaze for the bubble and lending the
//! tracker. It is tobii-linux's own protocol, not Tobii's; the messages are tobii_control's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use tobii_control::protocol::{
    Event, FRAME_CONTROL_EVENT, FRAME_CONTROL_GAZE, FRAME_CONTROL_REPLY, FRAME_CONTROL_REQUEST, GazeSample,
    PROTOCOL_VERSION, Reply, Request, Status, TrackedEyes, decode_hello, decode_request, payload, request_id,
};
use tobii_gaze::operation::Operation;
use tobii_gaze::profiles::{self, GUEST, SwitchOptions, recorded_monitor_key, tracked_eye};
use tobii_protocol::gaze::GazeFrame;

use super::link::{Ended, Frames, Link, Outcome, refused};
use super::{MAX_PENDING_STREAM_BYTES, PrpService};

/// One control connection: what it subscribed and the status it last saw.
#[derive(Debug)]
pub struct ControlConnection {
    key: u64,
    client: String,
    link: Link,
    gaze: Cell<bool>,
    last_status: RefCell<Option<Status>>,
}

impl ControlConnection {
    fn send(&self, kind: u32, value: &impl serde::Serialize) {
        match payload(value) {
            Ok(payload) => self.link.frame(kind, &payload),
            Err(error) => log::warn!("dropping a control message: {error}"),
        }
    }

    /// Sends `status` as an event unless it is the one this client saw last.
    fn show(&self, status: &Status) {
        if self.last_status.borrow().as_ref() == Some(status) {
            return;
        }
        *self.last_status.borrow_mut() = Some(status.clone());
        self.send(FRAME_CONTROL_EVENT, &Event::Status(status.clone()));
    }
}

/// Serves one control connection from its HELLO until it closes.
pub async fn serve(service: &PrpService, link: Link, frames: &mut Frames, hello: &[u8]) -> Outcome {
    let hello = decode_hello(hello).map_err(|error| Ended::Refused(error.to_string()))?;
    if hello.version != PROTOCOL_VERSION {
        return refused(format!("control protocol version {}; this service speaks {PROTOCOL_VERSION}", hello.version));
    }
    let key = service.0.control_keys.get();
    service.0.control_keys.set(key + 1);
    let connection = Rc::new(ControlConnection {
        key,
        client: hello.client,
        link,
        gaze: Cell::new(false),
        last_status: RefCell::new(None),
    });
    service.0.controls.borrow_mut().push(Rc::clone(&connection));
    log::info!("control client {} connected", connection.client);
    connection.show(&service.control_status());
    let outcome = requests(service, &connection, frames).await;
    service.0.controls.borrow_mut().retain(|other| !Rc::ptr_eq(other, &connection));
    if service.0.release_holder.get() == Some(key) {
        log::info!("control client {} closed while holding the tracker; reacquiring", connection.client);
        service.reacquire_tracker();
    }
    log::info!("control client {} disconnected", connection.client);
    outcome
}

async fn requests(service: &PrpService, connection: &ControlConnection, frames: &mut Frames) -> Outcome {
    loop {
        let (kind, payload) = frames.next().await?;
        if kind != FRAME_CONTROL_REQUEST {
            return refused(format!("unexpected control frame {kind}"));
        }
        let (id, result) = match decode_request(&payload) {
            Ok((id, request)) => (id, answer(service, connection, request).await),
            Err(error) => match request_id(&payload) {
                Some(id) => (id, Err(error.to_string())),
                None => return refused(error.to_string()),
            },
        };
        connection.send(FRAME_CONTROL_REPLY, &Reply { id, result });
    }
}

async fn answer(service: &PrpService, connection: &ControlConnection, request: Request) -> Result<Status, String> {
    match request {
        Request::Status => {}
        Request::SetEnabled(enabled) => service.set_enabled(enabled).await.map_err(|error| error.to_string())?,
        Request::SwitchProfile { name } => service.switch_profile(name).await?,
        Request::SetTrackedEyes(eyes) => service.set_tracked_eyes(eyes).await?,
        Request::Release => service.release_tracker(connection).await?,
        Request::Reacquire => match service.0.release_holder.get() {
            Some(holder) if holder != connection.key => return Err(held_elsewhere()),
            Some(_) => service.reacquire_tracker(),
            None => {}
        },
        Request::SubscribeGaze(on) => connection.gaze.set(on),
    }
    Ok(service.control_status())
}

fn held_elsewhere() -> String {
    "Another client holds the tracker".into()
}

impl PrpService {
    /// What control clients are told about the service.
    pub fn control_status(&self) -> Status {
        let released = self.released();
        let profile = self.0.profiles.current().unwrap_or_else(|error| {
            log::debug!("no current profile: {error}");
            None
        });
        Status {
            attached: self.tracker_attached() && !released,
            enabled: self.enabled(),
            released,
            display_area: self.display_area().is_some(),
            calibration_matches_profile: profile.is_some() && self.0.profile_matches.get() && !released,
            profile,
        }
    }

    /// Why no profile can be put on the tracker now.
    fn profile_target(&self) -> Result<(), String> {
        if self.released() {
            return Err("The tracker is lent to a control client".into());
        }
        if !self.tracker_attached() || self.0.device.borrow().is_none() {
            return Err("No tracker is attached".into());
        }
        Ok(())
    }

    /// Puts the current profile on a tracker that just attached, in the background.
    pub(super) fn apply_current_profile(&self) {
        if self.profile_target().is_err() {
            return;
        }
        let name = match self.0.profiles.current() {
            Ok(Some(name)) => name,
            Ok(None) => return,
            Err(error) => {
                log::warn!("cannot read the current profile: {error}");
                return;
            }
        };
        let service = self.clone();
        tokio::task::spawn_local(async move {
            if let Err(error) = service.apply_profile(name.clone(), false).await {
                log::warn!("could not put profile {name} on the tracker: {error}");
            }
        });
    }

    /// Sends 3160 with the profile's tracked eyes and 1110 with its calibration for the
    /// recorded monitor, then makes it current; `force` sends both even when the device
    /// already reports the calibration's id.
    async fn apply_profile(&self, name: String, force: bool) -> Result<(), String> {
        let _applying = self.0.profile_lock.lock().await;
        self.profile_target()?;
        self.0.profile_matches.set(false);
        let store = self.0.profiles.clone();
        let eye = tracked_eye(store.tracked_eyes(&name).map_err(|error| error.to_string())?);
        let monitor_key = recorded_monitor_key(&self.0.display_record);
        let options = SwitchOptions { eye, monitor_key, force, dry_run: false };
        let switching = Operation::new(move |io| profiles::switch_with(io, store, name, options));
        let outcome = self.run_operation(switching).await;
        if let Ok(result) = &outcome {
            self.0.calibration_id.set(result.device_id_after);
            self.0.profile_matches.set(self.profile_target().is_ok());
            let action = if result.applied { "applied" } else { "already on the tracker" };
            log::info!("profile {} {action} (calibration id {:#x})", result.profile, result.device_id_after);
        }
        self.broadcast_status();
        outcome.map(|_| ()).map_err(|error| error.to_string())
    }

    /// Makes a listed profile current, as a click in Tobii's profile list does; the current
    /// profile and [`GUEST`] are not in that list.
    async fn switch_profile(&self, name: String) -> Result<(), String> {
        self.profile_target()?;
        if name == GUEST {
            return Err(format!("{GUEST} is not in the profile list"));
        }
        if !self.0.profiles.names().map_err(|error| error.to_string())?.contains(&name) {
            return Err(format!("No profile named {name}"));
        }
        if self.0.profiles.current().map_err(|error| error.to_string())?.as_deref() == Some(name.as_str()) {
            return Ok(());
        }
        self.apply_profile(name, false).await
    }

    /// Stores the current profile's tracked eyes and applies the profile again when they
    /// changed, as Tobii's tracked-eyes radios do.
    async fn set_tracked_eyes(&self, eyes: TrackedEyes) -> Result<(), String> {
        self.profile_target()?;
        let store = &self.0.profiles;
        let name = store.current().map_err(|error| error.to_string())?.ok_or("No profile is current")?;
        let value = eyes.registry_value();
        if store.tracked_eyes(&name).map_err(|error| error.to_string())? == value {
            return Ok(());
        }
        store.set_tracked_eyes(&name, value).map_err(|error| error.to_string())?;
        self.apply_profile(name, true).await
    }

    /// Whether a control client holds the tracker.
    pub fn released(&self) -> bool {
        self.0.release_gate.is_held()
    }

    pub fn control_connections(&self) -> usize {
        self.0.controls.borrow().len()
    }

    /// Sends the current status to every control client that has not seen it.
    pub(super) fn broadcast_status(&self) {
        let controls = self.0.controls.borrow().clone();
        if controls.is_empty() {
            return;
        }
        let status = self.control_status();
        for connection in controls {
            connection.show(&status);
        }
    }

    pub(super) fn publish_control_gaze(&self, frame: &GazeFrame) {
        let controls = self.0.controls.borrow().clone();
        let point = frame.gaze_point;
        let sample = GazeSample {
            timestamp_us: frame.timestamp_us(),
            x: point.point[0],
            y: point.point[1],
            valid: point.valid != 0,
        };
        for connection in controls.iter().filter(|connection| connection.gaze.get()) {
            if connection.link.pending() <= MAX_PENDING_STREAM_BYTES {
                connection.link.frame(FRAME_CONTROL_GAZE, &sample.encode());
            }
        }
    }

    pub(super) fn close_controls(&self) {
        let controls = self.0.controls.borrow().clone();
        for connection in controls {
            connection.link.close();
        }
    }

    /// Holds the release gate for `connection` and returns once the tracker session has
    /// closed USB.
    async fn release_tracker(&self, connection: &ControlConnection) -> Result<(), String> {
        if self.0.release_holder.get().is_some_and(|holder| holder != connection.key) {
            return Err(held_elsewhere());
        }
        self.0.release_holder.set(Some(connection.key));
        let gate = Arc::clone(&self.0.release_gate);
        gate.hold();
        log::info!("releasing the tracker to control client {}", connection.client);
        self.broadcast_status();
        gate.closed().await;
        log::info!("tracker released to control client {}", connection.client);
        Ok(())
    }

    fn reacquire_tracker(&self) {
        self.0.release_holder.set(None);
        self.0.release_gate.open();
        log::info!("reacquiring the tracker");
        self.broadcast_status();
    }
}
