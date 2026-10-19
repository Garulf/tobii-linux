//! The control channel of the experience app and the setup handover, driven with the
//! blocking tobii_control client against a service fed by scripted trackers.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fake_tracker::{FakeTracker, GAZE_POINT};
use common::link::{BACKOFF_FIRST_S, Link, Plugged, WAIT, plug, until};
use common::*;
use tobii_control::protocol::{Event, Request, Status, TrackedEyes};
use tobii_control::{ControlClient, ControlError, GazeSample, Incoming};
use tobii_gaze::device::{LocalCalibration, gaze_command};
use tobii_gaze::profiles::{GUEST, ProfileStore};
use tobii_linux::service::clock::{Clocks, wine_clock};
use tobii_linux::service::{Options, PrpService};
use tobii_protocol::commands::command;
use tobii_protocol::fields::{Value, values};
use tobii_setup::cli::{self, Command};
use tobii_setup::device::{Session, session_on};
use tobii_setup::flow::FlowError;
use tobii_setup::handover::Handover;
use tobii_setup::simulated::{SimulatedTracker, Simulation};

async fn service(options: Options) -> (PrpService, u16, tokio::task::JoinHandle<()>) {
    common::service(options, wine_clock("auto", Clocks::Host).unwrap()).await
}

/// The blocking client, run off the service's loop.
struct Client(Option<ControlClient>);

impl Client {
    async fn connect(port: u16, client: &'static str) -> Self {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let connected = tokio::task::spawn_blocking(move || ControlClient::connect(addr, client, WAIT));
        Self(Some(connected.await.unwrap().unwrap()))
    }

    async fn with<T: Send + 'static>(&mut self, call: impl FnOnce(&mut ControlClient) -> T + Send + 'static) -> T {
        let mut client = self.0.take().unwrap();
        let (client, outcome) = tokio::task::spawn_blocking(move || {
            let outcome = call(&mut client);
            (client, outcome)
        })
        .await
        .unwrap();
        self.0 = Some(client);
        outcome
    }

    async fn request(&mut self, request: Request) -> Result<Status, ControlError> {
        self.with(move |client| client.request(request, WAIT)).await
    }

    async fn next(&mut self, timeout: Duration) -> Option<Incoming> {
        self.with(move |client| client.next(timeout)).await.unwrap()
    }

    /// Skips events and gaze until a status event that `wanted` accepts.
    async fn status(&mut self, what: &str, wanted: impl Fn(&Status) -> bool) -> Status {
        let deadline = Instant::now() + WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "no status event: {what}");
            if let Some(Incoming::Event(Event::Status(status))) = self.next(remaining).await
                && wanted(&status)
            {
                return status;
            }
        }
    }

    async fn gaze(&mut self) -> GazeSample {
        let deadline = Instant::now() + WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "no gaze sample");
            if let Some(Incoming::Gaze(sample)) = self.next(remaining).await {
                return sample;
            }
        }
    }
}

fn pause_values(device: &FakeTracker) -> Vec<i64> {
    let values =
        |message: tobii_protocol::Message| values(&message.payload).unwrap().first().and_then(Value::as_int).unwrap();
    device.log().into_iter().filter(|m| m.command == command::PAUSE_DEVICE).map(values).collect()
}

fn profiles_directory(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("tobii-linux-control-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    directory
}

/// A calibration whose blob carries its id in word 5, as the device's do.
fn calibration(id: u32) -> LocalCalibration {
    let mut blob = vec![0x5a; 40];
    blob[20..24].copy_from_slice(&id.to_le_bytes());
    LocalCalibration { calibration_id: id, blob }
}

/// The 3160 eyes and 1110 calibration ids the device was sent, in order.
fn profile_writes(device: &FakeTracker) -> Vec<(u32, i64)> {
    let first = |message: &tobii_protocol::Message| values(&message.payload).unwrap().into_iter().next().unwrap();
    device
        .log()
        .iter()
        .filter_map(|message| match message.command {
            gaze_command::SET_ENABLED_EYE => Some((message.command, first(message).as_int().unwrap())),
            gaze_command::CALIBRATION_APPLY => match first(message) {
                Value::Bytes(blob) => {
                    Some((message.command, i64::from(u32::from_le_bytes(blob[20..24].try_into().unwrap()))))
                }
                other => panic!("1110 with {other:?}"),
            },
            _ => None,
        })
        .collect()
}

/// A profile store and display record in a fresh directory, with `profiles` saved.
fn profile_options(name: &str, profiles: &[(&str, u32)], current: &str) -> (Options, ProfileStore, std::path::PathBuf) {
    let directory = profiles_directory(name);
    let store = ProfileStore::new(directory.join("profiles"));
    for &(profile, id) in profiles {
        store.save(profile, &calibration(id), false).unwrap();
    }
    store.set_current(current).unwrap();
    let options = Options {
        profiles: Some(store.directory.clone()),
        display_record: Some(directory.join("display-setup.json")),
        ..options("ET5-TEST")
    };
    (options, store, directory)
}

const EYE: u32 = gaze_command::SET_ENABLED_EYE;
const APPLY: u32 = gaze_command::CALIBRATION_APPLY;
const LEFT: i64 = 1;
const RIGHT: i64 = 2;
const BOTH: i64 = 3;

#[test]
fn status_reflects_attach_and_enabled() {
    run(async {
        let profiles = profiles_directory("status");
        ProfileStore::new(&profiles).set_current("Default").unwrap();
        let options = Options { profiles: Some(profiles.clone()), ..options("ET5-TEST") };
        let (service, port, accept) = service(options).await;
        let mut client = Client::connect(port, "experience").await;
        let initial = client.status("initial", |_| true).await;
        let detached = Status { enabled: true, profile: Some("Default".into()), ..Status::default() };
        assert_eq!(initial, detached);

        let slot = Plugged::new(Mutex::new(Some(FakeTracker::streaming())));
        let mut link = Link::start(&service, false, &slot);
        let attached = client.status("attached", |status| status.attached).await;
        assert_eq!(attached, Status { attached: true, display_area: true, ..detached.clone() });

        let disabled = client.request(Request::SetEnabled(false)).await.unwrap();
        assert_eq!(disabled, Status { enabled: false, ..attached.clone() });
        assert_eq!(client.status("disabled", |status| !status.enabled).await, disabled);
        assert!(!service.enabled());
        assert_eq!(client.request(Request::Status).await.unwrap(), disabled);
        for refused in [Request::SwitchProfile { name: GUEST.into() }, Request::SetTrackedEyes(TrackedEyes::Left)] {
            let refusal = client.request(refused).await;
            assert!(matches!(&refusal, Err(ControlError::Refused(_))), "{refusal:?}");
        }

        assert!(link.stop().await);
        let stopped = client.status("detached", |status| !status.attached).await;
        assert_eq!(stopped, Status { attached: false, ..disabled });
        service.close();
        accept.abort();
        let _ = std::fs::remove_dir_all(profiles);
    });
}

#[test]
fn set_enabled_over_control_pauses() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::pausable();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        let mut client = Client::connect(port, "experience").await;
        client.status("attached", |status| status.attached).await;

        assert!(!client.request(Request::SetEnabled(false)).await.unwrap().enabled);
        until("paused", || pause_values(&device) == [1]).await;
        assert!(client.request(Request::SetEnabled(true)).await.unwrap().enabled);
        until("resumed", || pause_values(&device) == [1, 0]).await;
        assert_eq!(link.sessions_and_failures(), (1, 0));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn release_closes_usb_and_blocks_reopen() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        let mut app = Client::connect(port, "experience").await;
        let mut setup = Client::connect(port, "setup").await;
        setup.status("attached", |status| status.attached).await;
        let idle_handles = 2;
        assert!(device.handles() > idle_handles);

        let released = setup.request(Request::Release).await.unwrap();
        assert_eq!(device.handles(), idle_handles, "the session still holds the tracker");
        assert!(!service.release_gate().usb_open());
        assert!((released.released, released.attached) == (true, false), "{released:?}");
        app.status("released", |status| status.released && !status.attached).await;
        assert!(matches!(app.request(Request::Release).await, Err(ControlError::Refused(_))));
        assert!(matches!(app.request(Request::Reacquire).await, Err(ControlError::Refused(_))));

        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(link.sessions_and_failures(), (1, 0));
        assert_eq!(device.handles(), idle_handles);
        assert!(!service.tracker_attached());
        assert!(setup.request(Request::Status).await.unwrap().released);

        let reacquired = setup.request(Request::Reacquire).await.unwrap();
        assert!(!reacquired.released);
        app.status("attached again", |status| status.attached && !status.released).await;
        assert_eq!(link.sessions_and_failures(), (2, 0));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

/// Runs setup's session opener for `tobii-linux-setup --service 127.0.0.1:PORT` on a blocking
/// thread, with a fake USB opener that records the tracker handles left besides its own.
async fn open_setup_session(
    port: u16,
    device: &FakeTracker,
) -> (Result<(Session, Option<Handover>), FlowError>, Option<usize>) {
    let device = device.clone();
    tokio::task::spawn_blocking(move || {
        let Ok(Command::Run(args)) = cli::parse(["--service".to_string(), format!("127.0.0.1:{port}")]) else {
            panic!("the arguments parse")
        };
        let handles_at_open = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&handles_at_open);
        let opened = cli::session_opener(&args, Box::new(|| None), move |_, _, _| {
            *seen.lock().unwrap() = Some(device.handles() - 1);
            Ok(session_on(SimulatedTracker::new(Simulation::default()), None))
        })();
        let handles = *handles_at_open.lock().unwrap();
        (opened, handles)
    })
    .await
    .unwrap()
}

#[test]
fn setup_releases_and_service_reacquires() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        let mut app = Client::connect(port, "experience").await;
        app.status("attached", |status| status.attached).await;
        let idle_handles = 2;

        let (opened, handles_at_open) = open_setup_session(port, &device).await;
        let (session, handover) = opened.unwrap();
        assert!(handover.is_some(), "setup holds the release while its session is open");
        assert_eq!(handles_at_open, Some(idle_handles), "USB was opened before the service let go");
        assert!(!service.release_gate().usb_open());
        let status = app.request(Request::Status).await.unwrap();
        assert!(status.released && !status.attached, "{status:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(link.sessions_and_failures(), (1, 0));

        drop(session);
        drop(handover);
        app.status("attached again", |status| status.attached && !status.released).await;
        assert_eq!(link.sessions_and_failures(), (2, 0));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn setup_does_not_take_a_tracker_held_by_another_client() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        let mut other = Client::connect(port, "setup").await;
        other.status("attached", |status| status.attached).await;
        other.request(Request::Release).await.unwrap();

        let (opened, handles_at_open) = open_setup_session(port, &device).await;
        let error = opened.expect_err("a held release refuses the setup");
        assert!(error.to_string().contains("refused"), "{error}");
        assert_eq!(handles_at_open, None, "no USB grab");
        assert!(service.released());
        drop(other);
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn dropped_release_connection_reacquires() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let slot = Plugged::new(Mutex::new(Some(FakeTracker::streaming())));
        let mut link = Link::start(&service, false, &slot);
        let mut app = Client::connect(port, "experience").await;
        let mut setup = Client::connect(port, "setup").await;
        app.status("attached", |status| status.attached).await;
        setup.request(Request::Release).await.unwrap();
        app.status("released", |status| status.released).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        drop(setup);
        let dropped = Instant::now();
        app.status("attached again", |status| status.attached && !status.released).await;
        let limit = Duration::from_secs_f64(BACKOFF_FIRST_S + 1.0);
        assert!(dropped.elapsed() < limit, "reacquired after {:?}", dropped.elapsed());
        assert!(!service.released());
        assert_eq!(link.sessions_and_failures(), (2, 0));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn gaze_subscription_streams_samples() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        assert!(service.gaze_hub().is_none());
        let slot = Plugged::new(Mutex::new(Some(FakeTracker::streaming().gazing())));
        let mut link = Link::start(&service, false, &slot);
        let mut client = Client::connect(port, "experience").await;
        client.status("attached", |status| status.attached).await;

        client.request(Request::SubscribeGaze(true)).await.unwrap();
        let sample = client.gaze().await;
        assert_eq!(([sample.x, sample.y], sample.valid), (GAZE_POINT, true));
        let later = client.gaze().await;
        assert!(later.timestamp_us > sample.timestamp_us);

        client.request(Request::SubscribeGaze(false)).await.unwrap();
        let deadline = Instant::now() + WAIT;
        while client.next(Duration::from_millis(300)).await.is_some() {
            assert!(Instant::now() < deadline, "gaze kept coming after unsubscribing");
        }
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn control_survives_tracker_loss() {
    run(async {
        let (service, port, accept) = service(options("ET5-TEST")).await;
        let device = FakeTracker::streaming();
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        let mut client = Client::connect(port, "experience").await;
        client.status("attached", |status| status.attached).await;

        device.unplug();
        plug(&slot, None);
        client.status("detached", |status| !status.attached).await;
        assert_eq!(service.control_connections(), 1);
        assert!(!client.request(Request::Status).await.unwrap().attached);

        plug(&slot, Some(FakeTracker::streaming()));
        client.status("attached again", |status| status.attached).await;
        assert_eq!(link.sessions_and_failures(), (2, 1));
        assert!(link.stop().await);
        service.close();
        accept.abort();
    });
}

#[test]
fn attach_applies_current_profile() {
    run(async {
        let (options, store, directory) = profile_options("attach", &[("Garulf", 0x11)], "Garulf");
        store.set_tracked_eyes("Garulf", 1).unwrap();
        std::fs::write(directory.join("display-setup.json"), r#"{"monitor":{"key":"DP-1","edid_id":"10AC4C41"}}"#)
            .unwrap();
        store.save_for("Garulf", Some("10AC4C41"), &calibration(0x22), false).unwrap();
        let (service, port, accept) = service(options).await;
        let mut client = Client::connect(port, "experience").await;
        assert!(!client.status("initial", |_| true).await.calibration_matches_profile);

        let device = FakeTracker::streaming().calibrated(0x99);
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        let applied = client.status("applied", |status| status.calibration_matches_profile).await;
        assert_eq!((applied.attached, applied.profile.as_deref()), (true, Some("Garulf")));
        assert_eq!(profile_writes(&device), [(EYE, LEFT), (APPLY, 0x22)]);
        assert_eq!(device.calibration_id(), Some(0x22));

        device.unplug();
        plug(&slot, None);
        client.status("detached", |status| !status.attached && !status.calibration_matches_profile).await;
        let matching = FakeTracker::streaming().calibrated(0x22);
        plug(&slot, Some(matching.clone()));
        client.status("attached again", |status| status.attached && status.calibration_matches_profile).await;
        assert_eq!(profile_writes(&matching), []);
        assert_eq!(store.current().unwrap().as_deref(), Some("Garulf"));
        assert!(link.stop().await);
        service.close();
        accept.abort();
        let _ = std::fs::remove_dir_all(directory);
    });
}

#[test]
fn switch_profile_over_control() {
    run(async {
        let profiles = [("Garulf", 0x11), ("Other", 0x22), (GUEST, 0x33)];
        let (options, store, directory) = profile_options("switch", &profiles, "Garulf");
        let (service, port, accept) = service(options).await;
        let mut client = Client::connect(port, "experience").await;
        client.status("initial", |_| true).await;
        let detached = client.request(Request::SwitchProfile { name: "Other".into() }).await;
        assert!(matches!(detached, Err(ControlError::Refused(_))), "{detached:?}");

        let device = FakeTracker::streaming().calibrated(0x11);
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        client.status("attached", |status| status.attached && status.calibration_matches_profile).await;
        assert_eq!(profile_writes(&device), []);

        let switched = client.request(Request::SwitchProfile { name: "Other".into() }).await.unwrap();
        assert_eq!((switched.profile.as_deref(), switched.calibration_matches_profile), (Some("Other"), true));
        assert_eq!(profile_writes(&device), [(EYE, BOTH), (APPLY, 0x22)]);
        assert_eq!(store.current().unwrap().as_deref(), Some("Other"));
        client.status("switched", |status| status.profile.as_deref() == Some("Other")).await;

        for name in ["Nobody", GUEST, "a/b"] {
            let refusal = client.request(Request::SwitchProfile { name: name.into() }).await;
            assert!(matches!(refusal, Err(ControlError::Refused(_))), "{name}: {refusal:?}");
        }
        assert_eq!(store.current().unwrap().as_deref(), Some("Other"));
        assert_eq!(profile_writes(&device).len(), 2);

        let mut setup = Client::connect(port, "setup").await;
        setup.request(Request::Release).await.unwrap();
        let released = client.request(Request::SwitchProfile { name: "Garulf".into() }).await;
        assert!(matches!(released, Err(ControlError::Refused(_))), "{released:?}");
        assert_eq!(store.current().unwrap().as_deref(), Some("Other"));
        assert!(link.stop().await);
        service.close();
        accept.abort();
        let _ = std::fs::remove_dir_all(directory);
    });
}

#[test]
fn set_tracked_eyes_over_control() {
    run(async {
        let (options, store, directory) = profile_options("eyes", &[("Garulf", 0x11)], "Garulf");
        let (service, port, accept) = service(options).await;
        let mut client = Client::connect(port, "experience").await;
        let device = FakeTracker::streaming().calibrated(0x11);
        let slot = Plugged::new(Mutex::new(Some(device.clone())));
        let mut link = Link::start(&service, false, &slot);
        client.status("attached", |status| status.attached && status.calibration_matches_profile).await;
        assert_eq!(profile_writes(&device), []);

        let right = client.request(Request::SetTrackedEyes(TrackedEyes::Right)).await.unwrap();
        assert!(right.calibration_matches_profile);
        assert_eq!(profile_writes(&device), [(EYE, RIGHT), (APPLY, 0x11)]);
        assert_eq!(store.tracked_eyes("Garulf").unwrap(), 2);
        client.request(Request::SetTrackedEyes(TrackedEyes::Right)).await.unwrap();
        assert_eq!(profile_writes(&device).len(), 2, "an unchanged eye is not sent again");
        client.request(Request::SetTrackedEyes(TrackedEyes::Both)).await.unwrap();
        assert_eq!(profile_writes(&device)[2..], [(EYE, BOTH), (APPLY, 0x11)]);
        assert_eq!(store.tracked_eyes("Garulf").unwrap(), 0);
        assert!(link.stop().await);
        service.close();
        accept.abort();
        let _ = std::fs::remove_dir_all(directory);
    });
}
