//! The scenarios of tests/test_qpc_clock.py and the counter mapping cases of
//! tests/test_sesp.py: synthetic hosts give each Linux clock an offset and a rate against
//! a true elapsed time, and a report carries the counter of one of them.

use std::cell::Cell;
use std::collections::HashMap;

use super::*;

const WINE_FREQUENCY: u64 = 10_000_000;
const BOOT_US: i64 = 5_000_000_000;
/// The reference host's WSL2 kernel: CLOCK_MONOTONIC_RAW runs 9.1% faster than CLOCK_MONOTONIC.
const WSL_RAW_RATE: f64 = 1.091;

#[derive(Debug)]
struct SyntheticHost {
    elapsed_us: Cell<i64>,
    rates: [f64; 3],
    offsets: Cell<[i64; 3]>,
}

impl SyntheticHost {
    fn new() -> Self {
        Self { elapsed_us: Cell::new(0), rates: [1.0; 3], offsets: Cell::new([BOOT_US; 3]) }
    }

    fn wsl() -> Self {
        let mut host = Self::new();
        host.rates[LinuxClock::Raw.index()] = WSL_RAW_RATE;
        host.set_offset(LinuxClock::Raw, BOOT_US + 7_862_400_000);
        host
    }

    fn set_offset(&self, clock: LinuxClock, offset: i64) {
        let mut offsets = self.offsets.get();
        offsets[clock.index()] = offset;
        self.offsets.set(offsets);
    }

    fn advance(&self, us: i64) {
        self.elapsed_us.set(self.elapsed_us.get() + us);
    }

    fn read(&self, clock: LinuxClock) -> i64 {
        self.offsets.get()[clock.index()]
            + (self.rates[clock.index()] * self.elapsed_us.get() as f64).round_ties_even() as i64
    }

    fn suspend(&self, seconds: i64) {
        self.set_offset(LinuxClock::Boottime, self.offsets.get()[0] + seconds * 1_000_000);
    }

    /// Wine's QueryPerformanceCounter on `clock`, read `latency_us` before receipt.
    fn counter(&self, clock: LinuxClock, latency_us: i64) -> i64 {
        self.advance(-latency_us);
        let value = self.read(clock) * 10;
        self.advance(latency_us);
        value
    }

    fn sample(&self, counter: i64) -> QpcSample {
        QpcSample {
            qpc_us: (micros(counter as u64, WINE_FREQUENCY)) as i64,
            host_us: LinuxClock::PREFERENCE.map(|clock| Some(self.read(clock))),
        }
    }
}

impl ClockReader for &SyntheticHost {
    type Error = Infallible;

    fn read_us(&self, clock: LinuxClock) -> Result<i64, Infallible> {
        Ok(self.read(clock))
    }
}

use LinuxClock::{Boottime, Raw};

fn classify(host: &SyntheticHost, source: LinuxClock, reports: usize, current: Option<LinuxClock>) -> ClockVerdict {
    let mut classifier = QpcClockClassifier::default();
    for _ in 0..reports {
        classifier.add(host.sample(host.counter(source, 300)));
        host.advance(2_000_000);
    }
    classifier.verdict(current)
}

fn delay(verdict: &ClockVerdict, clock: LinuxClock) -> i64 {
    verdict.delays_us.iter().find(|(c, _)| *c == clock).unwrap().1
}

fn rate(verdict: &ClockVerdict, clock: LinuxClock) -> f64 {
    verdict.rates_ppm.iter().find(|(c, _)| *c == clock).unwrap().1
}

#[test]
fn report_round_trip_with_details() {
    let details = vec![("wine".to_string(), "11.0".to_string()), ("proton".to_string(), "GE-Proton11-9".to_string())];
    let report = decode_qpc_report(&encode_qpc_report(123_456_789, WINE_FREQUENCY, &details)).unwrap();
    assert_eq!((report.counter, report.frequency, report.micros()), (123_456_789, WINE_FREQUENCY, 12_345_678));
    assert_eq!(report.details, details);
}

#[test]
fn periodic_report_is_sixteen_bytes() {
    let payload = [20u64.to_le_bytes(), 10u64.to_le_bytes()].concat();
    assert!(decode_qpc_report(&payload).unwrap().details.is_empty());
    assert_eq!(encode_qpc_report(20, 10, &[]), payload);
}

#[test]
fn malformed_reports_are_rejected() {
    let no_frequency = [5u64.to_le_bytes(), 0u64.to_le_bytes()].concat();
    for payload in [&[0u8; 15][..], &no_frequency] {
        assert!(matches!(decode_qpc_report(payload), Err(ServiceError::Invalid(_))));
    }
}

#[test]
fn details_follow_python_line_rules() {
    let mut payload = encode_qpc_report(1, 1, &[]);
    payload.extend_from_slice("a=1\r\nb=2\rc\x0bd=x=y\u{2028}a=3".as_bytes());
    payload.extend_from_slice(b"\nbad=\xff\n");
    let details: HashMap<String, String> = decode_qpc_report(&payload).unwrap().details.into_iter().collect();
    let expected: HashMap<String, String> =
        [("a", "3"), ("b", "2"), ("d", "x=y"), ("bad", "\u{fffd}")].map(|(k, v)| (k.into(), v.into())).into();
    assert_eq!(details, expected);
    assert_eq!(split_lines("x\n\ny\r\n"), ["x", "", "y"]);
    assert!(split_lines("").is_empty());
}

#[test]
fn upstream_wine_follows_boottime() {
    let verdict = classify(&SyntheticHost::wsl(), Boottime, 1, Some(Boottime));
    assert_eq!((verdict.clock, verdict.basis), (Some(Boottime), Basis::Absolute));
    assert_eq!(delay(&verdict, Boottime), 300);
}

#[test]
fn proton_follows_raw_from_the_first_report() {
    let verdict = classify(&SyntheticHost::wsl(), Raw, 1, Some(Boottime));
    assert_eq!((verdict.clock, verdict.basis), (Some(Raw), Basis::Absolute));
}

#[test]
fn equal_clocks_keep_the_current_one() {
    let host = SyntheticHost::new();
    assert_eq!(classify(&host, Raw, 1, Some(Boottime)).clock, Some(Boottime));
    assert_eq!(classify(&host, Raw, 1, Some(Raw)).clock, Some(Raw));
}

#[test]
fn nearest_match_wins_without_a_current_clock() {
    let host = SyntheticHost::new();
    host.set_offset(Raw, BOOT_US + 4_000);
    assert_eq!(classify(&host, Raw, 1, None).clock, Some(Raw));
}

#[test]
fn latency_spike_does_not_unseat_the_true_clock() {
    let host = SyntheticHost::new();
    host.set_offset(Raw, BOOT_US - 15_000);
    let mut classifier = QpcClockClassifier::default();
    for latency in [300, 400, 60_000] {
        classifier.add(host.sample(host.counter(Boottime, latency)));
        host.advance(2_000_000);
    }
    let verdict = classifier.verdict(Some(Boottime));
    assert_eq!((verdict.clock, verdict.basis), (Some(Boottime), Basis::Absolute));
}

#[test]
fn unanchored_counter_waits_for_the_rate_span() {
    let host = SyntheticHost::wsl();
    let mut classifier = QpcClockClassifier::default();
    classifier.add(host.sample(host.counter(Raw, 300) + 123_456_789_000));
    assert_eq!(classifier.verdict(Some(Boottime)).basis, Basis::Undecided);
}

#[test]
fn unanchored_counter_is_matched_by_rate() {
    let host = SyntheticHost::wsl();
    let mut classifier = QpcClockClassifier::default();
    for _ in 0..5 {
        classifier.add(host.sample(host.counter(Raw, 300) + 123_456_789_000));
        host.advance(2_000_000);
    }
    let verdict = classifier.verdict(Some(Boottime));
    assert_eq!((verdict.clock, verdict.basis), (Some(Raw), Basis::Rate));
    assert!(rate(&verdict, Raw).abs() <= 1.0);
    assert!(rate(&verdict, Boottime) < -80_000.0);
}

#[test]
fn tsc_counter_drifting_ppm_stays_on_raw() {
    let mut host = SyntheticHost::new();
    host.rates[Raw.index()] = WSL_RAW_RATE;
    host.set_offset(Raw, BOOT_US - 900_000_000);
    let mut classifier = QpcClockClassifier::default();
    let anchor = host.read(Raw) as f64;
    for _ in 0..8 {
        let raw_us = (host.read(Raw) - 300) as f64;
        let tsc_counter = ((anchor + (raw_us - anchor) * (1.0 + 5e-6)) * 10.0).round_ties_even() as i64;
        classifier.add(host.sample(tsc_counter));
        host.advance(4_000_000_000);
    }
    let delays = classifier.recent_delays_us();
    assert!(delays.iter().find(|(c, _)| *c == Raw).unwrap().1.abs() > 100_000);
    for current in [Raw, Boottime] {
        let verdict = classifier.verdict(Some(current));
        assert_eq!((verdict.clock, verdict.basis), (Some(Raw), Basis::Rate));
    }
}

#[test]
fn suspend_moves_the_match_to_raw() {
    let host = SyntheticHost::new();
    let mut classifier = QpcClockClassifier::default();
    let report = |classifier: &mut QpcClockClassifier| {
        classifier.add(host.sample(host.counter(Raw, 300)));
        host.advance(2_000_000);
    };
    (0..3).for_each(|_| report(&mut classifier));
    assert_eq!(classifier.verdict(Some(Boottime)).clock, Some(Boottime));
    host.suspend(600);
    (0..2).for_each(|_| report(&mut classifier));
    assert_eq!(classifier.verdict(Some(Boottime)).clock, Some(Boottime));
    (0..3).for_each(|_| report(&mut classifier));
    let verdict = classifier.verdict(Some(Boottime));
    assert_eq!((verdict.clock, verdict.basis), (Some(Raw), Basis::Absolute));
}

#[test]
fn counter_going_back_starts_over() {
    let host = SyntheticHost::wsl();
    let mut classifier = QpcClockClassifier::default();
    classifier.add(host.sample(host.counter(Raw, 300)));
    classifier.add(host.sample(host.counter(Raw, 300) - 50_000_000));
    assert_eq!(classifier.samples().len(), 1);
}

#[test]
fn window_keeps_the_newest_reports() {
    let host = SyntheticHost::new();
    let options = ClassifierOptions { window: 2, ..ClassifierOptions::default() };
    let mut classifier = QpcClockClassifier::new(options);
    for _ in 0..3 {
        classifier.add(host.sample(host.counter(Raw, 300)));
        host.advance(1_000);
    }
    assert_eq!(classifier.samples().len(), 2);
}

#[test]
fn describe_formats_delays_and_drift() {
    let verdict = ClockVerdict {
        clock: Some(Raw),
        basis: Basis::Rate,
        delays_us: vec![(Boottime, -1_234_567), (Raw, 300)],
        rates_ppm: vec![(Boottime, -83_412.6), (Raw, 0.4)],
    };
    assert_eq!(
        verdict.describe(),
        "rate; clock minus counter: boottime -1234.6 ms, raw +0.3 ms; drift: boottime -83413 ppm, raw +0 ppm"
    );
    let absolute = ClockVerdict { rates_ppm: Vec::new(), basis: Basis::Absolute, ..verdict };
    assert_eq!(absolute.describe(), "absolute; clock minus counter: boottime -1234.6 ms, raw +0.3 ms");
}

#[test]
fn switch_to_the_new_clock_value() {
    let host = SyntheticHost::wsl();
    let clock = ServiceClock::new(Boottime, false, &host);
    let before = clock.now_us().unwrap();
    let switch = clock.switch(Raw, false).unwrap();
    let state = clock.state();
    assert_eq!((state.source, state.generation, state.offset_us), (Raw, 1, 0));
    assert_eq!(clock.now_us().unwrap(), host.read(Raw));
    assert_eq!(switch.delta_us, host.read(Raw) - before);
}

#[test]
fn switch_keeping_the_value_is_continuous() {
    let host = SyntheticHost::wsl();
    let clock = ServiceClock::new(Boottime, false, &host);
    let before = clock.now_us().unwrap();
    let switch = clock.switch(Raw, true).unwrap();
    assert_eq!((clock.now_us().unwrap(), switch.delta_us), (before, 0));
    host.advance(1_000_000);
    assert_eq!(clock.now_us().unwrap() - before, (WSL_RAW_RATE * 1_000_000.0).round() as i64);
}

#[test]
fn linux_clocks_advance() {
    let clock = ServiceClock::new(DEFAULT_SOURCE, false, LinuxClocks);
    let first = HostClock::now_us(&clock);
    assert!(HostClock::now_us(&clock) >= first);
    assert_eq!(HostClock::generation(&clock), Some(0));
    assert!(clock.read_all().unwrap().iter().all(|value| *value > 0));
    assert_eq!(LinuxClock::from_name("raw"), Some(Raw));
    assert_eq!(LinuxClock::from_name("realtime"), None);
}

#[test]
fn fixed_clock_never_switches() {
    let host = SyntheticHost::wsl();
    let clock = ServiceClock::new(Boottime, false, &host);
    let mut follower = QpcClockFollower::default();
    let decision = follower.observe(&clock, "bridge", host.sample(host.counter(Raw, 300))).unwrap();
    assert_eq!((decision.verdict.clock, decision.switch), (Some(Raw), None));
    assert_eq!(clock.state().source, Boottime);
}

#[test]
fn automatic_clock_switches_and_switches_back() {
    let host = SyntheticHost::wsl();
    let clock = ServiceClock::new(Boottime, true, &host);
    let mut follower = QpcClockFollower::default();
    let switch = follower.observe(&clock, "bridge", host.sample(host.counter(Raw, 300))).unwrap().switch.unwrap();
    assert_eq!((switch.old, switch.new), (Boottime, Raw));
    follower.forget(&"bridge");
    host.advance(10_000_000);
    let switch = follower.observe(&clock, "next", host.sample(host.counter(Boottime, 300))).unwrap().switch.unwrap();
    assert_eq!((switch.old, switch.new), (Raw, Boottime));
}

#[test]
fn disagreeing_bridges_hold_the_current_clock() {
    let host = SyntheticHost::wsl();
    let clock = ServiceClock::new(Boottime, true, &host);
    let mut follower = QpcClockFollower::default();
    follower.observe(&clock, "wine", host.sample(host.counter(Boottime, 300))).unwrap();
    let decision = follower.observe(&clock, "proton", host.sample(host.counter(Raw, 300))).unwrap();
    assert_eq!((decision.switch, decision.held_by), (None, Some("wine")));
    follower.forget(&"wine");
    host.advance(2_000_000);
    let decision = follower.observe(&clock, "proton", host.sample(host.counter(Raw, 300))).unwrap();
    assert_eq!(decision.switch.unwrap().new, Raw);
    assert_eq!(follower.verdicts(), [("proton", Some(Raw))]);
}

fn raw_sample(qpc_us: i64, raw_us: i64) -> QpcSample {
    QpcSample { qpc_us, host_us: [None, Some(raw_us), None] }
}

fn raw_state() -> ClockState {
    ClockState { generation: 0, source: Raw, offset_us: 0 }
}

#[test]
fn without_reports_the_source_is_the_counter() {
    assert_eq!(CounterMapping::default().map(&raw_state(), 5_000_000), 5_000_000);
}

#[test]
fn matching_clock_keeps_the_value() {
    let mut mapping = CounterMapping::default();
    mapping.observe(QpcSample { qpc_us: 10_000_000, host_us: [Some(12_000_000), Some(10_000_400), None] });
    assert_eq!(mapping.map(&raw_state(), 20_000_000), 20_000_000);
}

#[test]
fn unanchored_counter_uses_the_smallest_delay() {
    let mut mapping = CounterMapping::default();
    mapping.observe(raw_sample(1_000_000, 501_000_300));
    mapping.observe(raw_sample(3_000_000, 503_000_200));
    assert_eq!(mapping.map(&raw_state(), 600_000_000), 99_999_800);
    let switched = ClockState { offset_us: 50, ..raw_state() };
    assert_eq!(mapping.offset_us(&switched), 500_000_250);
}
