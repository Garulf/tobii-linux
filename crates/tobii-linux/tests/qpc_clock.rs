//! Following the Linux clock behind Wine's QueryPerformanceCounter: BridgeReportTests,
//! test_wine_clock_names and ClockMapperTests of tests/test_qpc_clock.py. Scripted bridges
//! send counter reports read from a synthetic host: a true elapsed time and, for each Linux
//! clock, its offset and rate against it. The report, classifier, follower and ServiceClock
//! cases are tobii_service's own tests.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use tobii_device::clock::ClockMapper;
use tobii_linux::service::clock::{Clock, Clocks, ServiceTime, now_us, wine_clock};
use tobii_linux::service::{PrpService, QPC_REPORT_WAIT};
use tobii_service::prp_service::FRAME_QPC;
use tobii_service::prp_session::{
    COMMAND_TIMESYNC, ResponseBody, TIMESYNC_CHANNEL, decode_command_response, encode_timesync_request,
};
use tobii_service::qpc_clock::{AUTO, LinuxClock, encode_qpc_report};

const WINE_FREQUENCY: u64 = 10_000_000;
const BOOT_US: i64 = 5_000_000_000;
/// The reference host's WSL2 kernel: CLOCK_MONOTONIC_RAW runs 9.1% faster than CLOCK_MONOTONIC.
const WSL_RAW_RATE: f64 = 1.091;
const RECEIPT_LATENCY_US: i64 = 300;

#[derive(Debug)]
struct HostState {
    elapsed_us: i64,
    rates: [f64; 3],
    offsets: [i64; 3],
}

fn slot(clock: LinuxClock) -> usize {
    match clock {
        LinuxClock::Boottime => 0,
        LinuxClock::Raw => 1,
        LinuxClock::Monotonic => 2,
    }
}

/// Linux clocks as offset + rate * elapsed microseconds.
#[derive(Debug, Clone)]
struct SyntheticHost(Arc<Mutex<HostState>>);

impl Default for SyntheticHost {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(HostState { elapsed_us: 0, rates: [1.0; 3], offsets: [BOOT_US; 3] })))
    }
}

impl SyntheticHost {
    /// Booted a day ago: boottime and monotonic agree, raw is far ahead and faster.
    fn wsl() -> Self {
        let host = Self::default();
        {
            let mut state = host.0.lock().unwrap();
            state.rates[slot(LinuxClock::Raw)] = WSL_RAW_RATE;
            state.offsets[slot(LinuxClock::Raw)] = BOOT_US + 7_862_400_000;
        }
        host
    }

    fn read_us(&self, clock: LinuxClock) -> i64 {
        let state = self.0.lock().unwrap();
        state.offsets[slot(clock)] + (state.rates[slot(clock)] * state.elapsed_us as f64).round_ties_even() as i64
    }

    fn advance(&self, microseconds: i64) {
        self.0.lock().unwrap().elapsed_us += microseconds;
    }

    fn suspend(&self, seconds: i64) {
        self.0.lock().unwrap().offsets[slot(LinuxClock::Boottime)] += seconds * 1_000_000;
    }

    /// Wine's QueryPerformanceCounter on `clock`, read shortly before receipt.
    fn counter(&self, clock: LinuxClock) -> u64 {
        self.advance(-RECEIPT_LATENCY_US);
        let value = self.read_us(clock) * 10;
        self.advance(RECEIPT_LATENCY_US);
        value as u64
    }

    fn clocks(&self) -> Clocks {
        let host = self.clone();
        Clocks::Scripted(Arc::new(move |clock| host.read_us(clock)))
    }
}

struct Fixture {
    host: SyntheticHost,
    clock: Arc<Clock>,
    service: PrpService,
    port: u16,
}

impl Fixture {
    async fn start(clock_name: &str, host: SyntheticHost) -> Self {
        let clock = wine_clock(clock_name, host.clocks()).unwrap();
        let (service, port, _) = service(options("ET5-TEST"), Arc::clone(&clock)).await;
        service.attach_tracker(None);
        Self { host, clock, service, port }
    }

    async fn auto() -> Self {
        Self::start(AUTO, SyntheticHost::wsl()).await
    }

    async fn report(&self, bridge: &mut Bridge, source: LinuxClock) {
        self.report_with(bridge, source, &[], 0).await;
    }

    async fn report_with(&self, bridge: &mut Bridge, source: LinuxClock, details: &[(&str, &str)], offset: u64) {
        let details: Vec<(String, String)> = details.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        let payload = encode_qpc_report(self.host.counter(source) + offset, WINE_FREQUENCY, &details);
        bridge.report(&self.service, &payload).await;
    }

    fn source(&self) -> LinuxClock {
        self.clock.state().source
    }
}

#[test]
fn auto_follows_raw_for_proton_at_the_first_report() {
    run(async {
        let fixture = Fixture::auto().await;
        let mut bridge = Bridge::attach(fixture.port).await;
        capture_logs();
        fixture.report_with(&mut bridge, LinuxClock::Raw, &[("wine", "11.0"), ("proton", "GE-Proton11-9")], 0).await;
        let text = take_logs(log::Level::Info).join("\n");
        assert_eq!(fixture.source(), LinuxClock::Raw);
        assert_eq!(now_us(&fixture.clock), fixture.host.read_us(LinuxClock::Raw));
        assert!(text.contains("proton GE-Proton11-9"), "{text}");
        assert!(text.contains("follows raw"), "{text}");
    });
}

#[test]
fn auto_stays_on_boottime_for_upstream_wine() {
    run(async {
        let fixture = Fixture::auto().await;
        let mut bridge = Bridge::attach(fixture.port).await;
        for _ in 0..3 {
            fixture.report(&mut bridge, LinuxClock::Boottime).await;
            fixture.host.advance(2_000_000);
        }
        let state = fixture.clock.state();
        assert_eq!((state.source, state.generation), (LinuxClock::Boottime, 0));
    });
}

#[test]
fn timesync_and_samples_use_the_chosen_clock() {
    run(async {
        let fixture = Fixture::auto().await;
        let mut bridge = Bridge::attach(fixture.port).await;
        fixture.report(&mut bridge, LinuxClock::Raw).await;
        let raw_us = fixture.host.read_us(LinuxClock::Raw);
        assert_eq!(now_us(fixture.service.clock()), raw_us);
        let mut timesync = RelayedChannel::open(fixture.port, &bridge.pipes[3], CLIENT_PID).await;
        timesync.handshake(TIMESYNC_CHANNEL, "client_ts", CLIENT_PID).await;
        timesync.send_packet(&encode_timesync_request(7)).await;
        let response = decode_command_response(&timesync.packet().await, true).unwrap();
        assert_eq!((response.tag, response.command), (7, COMMAND_TIMESYNC));
        assert_eq!(response.body, ResponseBody::Timesync(raw_us as u64));
    });
}

#[test]
fn switch_mid_session_closes_clients_and_moves_presence() {
    run(async {
        let fixture = Fixture::start(AUTO, SyntheticHost::default()).await;
        fixture.service.publish_presence(now_us(&fixture.clock), 1);
        let since = fixture.service.presence().since_us;
        let mut client = RelayedChannel::open(fixture.port, "ET5-TEST", CLIENT_PID).await;
        client.handshake(tobii_service::prp_session::COMMAND_CHANNEL, "client_cmd", CLIENT_PID).await;
        let mut bridge = Bridge::attach(fixture.port).await;
        fixture.report(&mut bridge, LinuxClock::Raw).await;
        assert_eq!(fixture.source(), LinuxClock::Boottime);
        fixture.host.advance(2_000_000);
        fixture.host.suspend(3600);
        capture_logs();
        for _ in 0..4 {
            fixture.host.advance(2_000_000);
            fixture.report(&mut bridge, LinuxClock::Raw).await;
        }
        let text = take_logs(log::Level::Info).join("\n");
        assert_eq!(fixture.source(), LinuxClock::Raw);
        assert!(client.closed().await);
        assert_eq!(fixture.service.presence().since_us, since - 3_600_000_000);
        assert!(text.contains("timesync on the new clock"), "{text}");
    });
}

#[test]
fn fixed_clock_warns_about_a_mismatch() {
    run(async {
        let fixture = Fixture::start("boottime", SyntheticHost::wsl()).await;
        let mut bridge = Bridge::attach(fixture.port).await;
        capture_logs();
        fixture.report(&mut bridge, LinuxClock::Raw).await;
        let warnings = take_logs(log::Level::Warn);
        assert_eq!(fixture.source(), LinuxClock::Boottime);
        assert!(warnings[0].contains("fixed to boottime"), "{warnings:?}");
    });
}

#[test]
fn unanchored_counter_switches_by_rate_without_a_jump() {
    run(async {
        let fixture = Fixture::auto().await;
        let mut bridge = Bridge::attach(fixture.port).await;
        for _ in 0..5 {
            let before = now_us(&fixture.clock);
            fixture.report_with(&mut bridge, LinuxClock::Raw, &[], 987_654_321_000).await;
            assert_eq!(now_us(&fixture.clock), before);
            fixture.host.advance(2_000_000);
        }
        assert_eq!(fixture.source(), LinuxClock::Raw);
        assert_ne!(fixture.clock.state().offset_us, 0);
    });
}

/// QPC_REPORT_WAIT is a constant here, so this waits the real five seconds the Python test
/// shortens by patching QPC_REPORT_WAIT_S.
#[test]
fn old_bridge_keeps_the_default_clock() {
    run(async {
        let fixture = Fixture::auto().await;
        capture_logs();
        let _bridge = Bridge::attach(fixture.port).await;
        tokio::time::sleep(QPC_REPORT_WAIT + Duration::from_millis(200)).await;
        let text = take_logs(log::Level::Info).join("\n");
        assert_eq!(fixture.source(), LinuxClock::Boottime);
        assert!(text.contains("does not report its QueryPerformanceCounter"), "{text}");
    });
}

#[test]
fn malformed_report_is_ignored() {
    run(async {
        let fixture = Fixture::auto().await;
        let mut bridge = Bridge::attach(fixture.port).await;
        capture_logs();
        bridge.send(FRAME_QPC, b"\x01\x02").await;
        bridge.sync(&fixture.service).await;
        assert!(!take_logs(log::Level::Warn).is_empty());
        fixture.report(&mut bridge, LinuxClock::Raw).await;
        assert_eq!(fixture.source(), LinuxClock::Raw);
    });
}

#[test]
fn wine_clock_names() {
    let clocks = || SyntheticHost::default().clocks();
    assert_eq!(wine_clock("raw", clocks()).unwrap().state().source, LinuxClock::Raw);
    assert!(!wine_clock("boottime", clocks()).unwrap().automatic());
    let automatic = wine_clock(AUTO, clocks()).unwrap();
    assert_eq!((automatic.state().source, automatic.automatic()), (LinuxClock::Boottime, true));
    assert!(wine_clock("realtime", clocks()).is_none());
}

#[test]
fn mapping_starts_over_when_the_clock_switches() {
    let host = SyntheticHost::wsl();
    let clock = wine_clock("boottime", host.clocks()).unwrap();
    let mut mapper = ClockMapper::new(ServiceTime(Arc::clone(&clock)));
    mapper.observe(1_000);
    assert_eq!(mapper.map(1_000), host.read_us(LinuxClock::Boottime));
    let Ok(_) = clock.switch(LinuxClock::Raw, false);
    assert_eq!(mapper.map(1_000), host.read_us(LinuxClock::Raw));
    host.advance(33_000);
    mapper.observe(34_000);
    assert_eq!(mapper.map(34_000), host.read_us(LinuxClock::Raw));
    assert_eq!(mapper.offsets().len(), 1);
}
