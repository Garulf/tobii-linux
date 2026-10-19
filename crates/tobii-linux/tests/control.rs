//! The control channel of the experience app and the setup handover, driven with the
//! blocking tobii_control client against a service fed by scripted trackers.

mod common;

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::fake_tracker::{FakeTracker, GAZE_POINT};
use common::link::{BACKOFF_FIRST_S, Link, Plugged, WAIT, plug, until};
use common::*;
use tobii_control::protocol::{Event, Request, Status, TrackedEyes};
use tobii_control::{ControlClient, ControlError, GazeSample, Incoming};
use tobii_gaze::profiles::ProfileStore;
use tobii_linux::service::clock::{Clocks, wine_clock};
use tobii_linux::service::{Options, PrpService};
use tobii_protocol::commands::command;
use tobii_protocol::fields::{Value, values};

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
        for unsupported in [Request::SwitchProfile { name: "Guest".into() }, Request::SetTrackedEyes(TrackedEyes::Left)]
        {
            let refusal = client.request(unsupported).await;
            assert!(matches!(&refusal, Err(ControlError::Refused(text)) if text == "unsupported"), "{refusal:?}");
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
