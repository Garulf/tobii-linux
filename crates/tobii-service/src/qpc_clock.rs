//! Service clock that ticks at the rate of the game's Wine QueryPerformanceCounter.
//!
//! The game DLL converts stream timestamps with an offset measured by timesync only, so
//! the service clock must run at the rate of the client's counter. Every Wine and Proton
//! build examined returns a Linux clock in 100 ns units from QueryPerformanceCounter:
//! CLOCK_BOOTTIME from upstream Wine 10.17, CLOCK_MONOTONIC_RAW before that and in every
//! Proton tree. GE-Proton 11-7 and later may serve it from the TSC in user mode, calibrated
//! against and anchored on CLOCK_MONOTONIC_RAW. docs/prp-bridge.md (Time base) has the
//! source references.
//!
//! The bridge inside the prefix reports its QueryPerformanceCounter periodically. In
//! automatic mode the service compares each report with the Linux clocks read at receipt
//! and follows the clock whose value matches the counter, or, when none matches, the one
//! that keeps the steadiest offset.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Mutex, MutexGuard};

use tobii_device::clock::HostClock;

use crate::error::{Result, ServiceError};

/// The Linux clocks a Wine counter can follow, in [`LinuxClock::PREFERENCE`] order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinuxClock {
    Boottime,
    Raw,
    Monotonic,
}

impl LinuxClock {
    /// Among clocks that fit equally well: the one current upstream Wine reads first, then
    /// Proton's.
    pub const PREFERENCE: [LinuxClock; 3] = [LinuxClock::Boottime, LinuxClock::Raw, LinuxClock::Monotonic];

    pub fn name(self) -> &'static str {
        match self {
            LinuxClock::Boottime => "boottime",
            LinuxClock::Raw => "raw",
            LinuxClock::Monotonic => "monotonic",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::PREFERENCE.into_iter().find(|clock| clock.name() == name)
    }

    fn index(self) -> usize {
        self as usize
    }

    fn id(self) -> rustix::time::ClockId {
        match self {
            LinuxClock::Boottime => rustix::time::ClockId::Boottime,
            LinuxClock::Raw => rustix::time::ClockId::MonotonicRaw,
            LinuxClock::Monotonic => rustix::time::ClockId::Monotonic,
        }
    }
}

pub const DEFAULT_SOURCE: LinuxClock = LinuxClock::Boottime;
pub const AUTO: &str = "auto";

/// A clock matches the counter when the smallest recent receipt delay is within this.
pub const ABSOLUTE_TOLERANCE_US: i64 = 10_000;
/// Reports whose smallest delay decides the absolute match; a few cover latency spikes.
pub const RECENT_REPORTS: usize = 4;
/// Reports kept for the rate comparison: about a minute at the bridge's 2 s period.
pub const RATE_WINDOW_REPORTS: usize = 32;
pub const RATE_MIN_SPAN_US: i64 = 6_000_000;
pub const RATE_MIN_PAIR_GAP_US: i64 = 1_000_000;
/// Another clock replaces the current one on rate alone only when it is steadier by this much.
pub const RATE_MARGIN_PPM: f64 = 2_000.0;

pub const QPC_REPORT_SIZE: usize = 16;

/// One clock reading per [`LinuxClock`], by preference index; `None` for a clock not read.
pub type Readings = [Option<i64>; 3];

/// One bridge report: QueryPerformanceCounter, its frequency and optional runtime details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QpcReport {
    pub counter: u64,
    pub frequency: u64,
    /// `key=value` lines in order; a repeated key keeps its first place and its last value.
    pub details: Vec<(String, String)>,
}

impl QpcReport {
    /// The counter in microseconds, rounded down.
    pub fn micros(&self) -> u128 {
        micros(self.counter, self.frequency)
    }
}

pub fn micros(counter: u64, frequency: u64) -> u128 {
    u128::from(counter) * 1_000_000 / u128::from(frequency)
}

pub fn encode_qpc_report(counter: u64, frequency: u64, details: &[(String, String)]) -> Vec<u8> {
    let mut payload = counter.to_le_bytes().to_vec();
    payload.extend_from_slice(&frequency.to_le_bytes());
    for (key, value) in details {
        payload.extend_from_slice(format!("{key}={value}\n").as_bytes());
    }
    payload
}

/// Line boundaries of Python's `str.splitlines`.
fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if !is_line_break(c) {
            continue;
        }
        lines.push(&text[start..index]);
        start = index + c.len_utf8();
        if c == '\r' && chars.peek().map(|(_, next)| *next) == Some('\n') {
            chars.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

pub fn decode_qpc_report(payload: &[u8]) -> Result<QpcReport> {
    if payload.len() < QPC_REPORT_SIZE {
        return Err(ServiceError::Invalid("truncated QueryPerformanceCounter report".into()));
    }
    let counter = u64::from_le_bytes(payload[..8].try_into().expect("eight bytes"));
    let frequency = u64::from_le_bytes(payload[8..16].try_into().expect("eight bytes"));
    if frequency == 0 {
        return Err(ServiceError::Invalid("QueryPerformanceCounter report without a frequency".into()));
    }
    let text = String::from_utf8_lossy(&payload[QPC_REPORT_SIZE..]);
    let mut details: Vec<(String, String)> = Vec::new();
    for line in split_lines(&text) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match details.iter_mut().find(|(existing, _)| existing == key) {
            Some(entry) => entry.1 = value.to_string(),
            None => details.push((key.to_string(), value.to_string())),
        }
    }
    Ok(QpcReport { counter, frequency, details })
}

/// A counter value in microseconds and each Linux clock read when it arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QpcSample {
    pub qpc_us: i64,
    pub host_us: Readings,
}

impl QpcSample {
    pub fn delay_us(&self, clock: LinuxClock) -> Option<i64> {
        self.host_us[clock.index()].map(|host| host - self.qpc_us)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    Absolute,
    Rate,
    Undecided,
}

impl Basis {
    pub fn name(self) -> &'static str {
        match self {
            Basis::Absolute => "absolute",
            Basis::Rate => "rate",
            Basis::Undecided => "undecided",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClockVerdict {
    pub clock: Option<LinuxClock>,
    pub basis: Basis,
    pub delays_us: Vec<(LinuxClock, i64)>,
    pub rates_ppm: Vec<(LinuxClock, f64)>,
}

/// The log line of a verdict: basis, each clock minus the counter in ms and the drifts.
pub fn describe(basis: &str, delays_us: &[(String, i64)], rates_ppm: &[(String, f64)]) -> String {
    let delays: Vec<String> =
        delays_us.iter().map(|(name, delay)| format!("{name} {:+.1} ms", *delay as f64 / 1000.0)).collect();
    let rates: Vec<String> = rates_ppm.iter().map(|(name, rate)| format!("{name} {rate:+.0} ppm")).collect();
    let mut text = format!("{basis}; clock minus counter: {}", delays.join(", "));
    if !rates.is_empty() {
        text.push_str(&format!("; drift: {}", rates.join(", ")));
    }
    text
}

impl ClockVerdict {
    pub fn describe(&self) -> String {
        let delays: Vec<(String, i64)> = self.delays_us.iter().map(|(c, d)| (c.name().to_string(), *d)).collect();
        let rates: Vec<(String, f64)> = self.rates_ppm.iter().map(|(c, r)| (c.name().to_string(), *r)).collect();
        describe(self.basis.name(), &delays, &rates)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassifierOptions {
    pub tolerance_us: i64,
    pub recent: usize,
    pub window: usize,
    pub min_rate_span_us: i64,
    pub rate_margin_ppm: f64,
}

impl Default for ClassifierOptions {
    fn default() -> Self {
        Self {
            tolerance_us: ABSOLUTE_TOLERANCE_US,
            recent: RECENT_REPORTS,
            window: RATE_WINDOW_REPORTS,
            min_rate_span_us: RATE_MIN_SPAN_US,
            rate_margin_ppm: RATE_MARGIN_PPM,
        }
    }
}

/// Median of `values` as `statistics.median` computes it; `values` is not empty.
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 { values[middle] } else { (values[middle - 1] + values[middle]) / 2.0 }
}

/// The first entry with the smallest absolute value, as Python's `min` with a key picks it.
fn closest<T: Copy, V: Copy + PartialOrd>(entries: &[(T, V)], magnitude: impl Fn(V) -> V) -> Option<T> {
    let mut best: Option<(T, V)> = None;
    for (key, value) in entries {
        if best.is_none_or(|(_, smallest)| magnitude(*value) < smallest) {
            best = Some((*key, magnitude(*value)));
        }
    }
    best.map(|(key, _)| key)
}

/// Decides which Linux clock one process's QueryPerformanceCounter follows.
///
/// A clock whose value equals the counter (within the receipt delay) is the source: Wine's
/// counter is that clock's absolute value in 100 ns units. When no clock matches, as with
/// an unanchored counter, the clock whose offset to the counter drifts least over the
/// retained reports is chosen instead. The current clock is kept while it still fits, so
/// equal clocks do not cause switches.
#[derive(Debug, Clone)]
pub struct QpcClockClassifier {
    options: ClassifierOptions,
    samples: VecDeque<QpcSample>,
}

impl Default for QpcClockClassifier {
    fn default() -> Self {
        Self::new(ClassifierOptions::default())
    }
}

impl QpcClockClassifier {
    pub fn new(options: ClassifierOptions) -> Self {
        Self { options, samples: VecDeque::with_capacity(options.window) }
    }

    pub fn options(&self) -> &ClassifierOptions {
        &self.options
    }

    pub fn samples(&self) -> &VecDeque<QpcSample> {
        &self.samples
    }

    /// Keeps the newest `window` samples; a counter that went back starts over.
    pub fn add(&mut self, sample: QpcSample) {
        if self.samples.back().is_some_and(|last| sample.qpc_us < last.qpc_us) {
            self.samples.clear();
        }
        if self.options.window == 0 {
            return;
        }
        if self.samples.len() == self.options.window {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// Clocks every retained sample has a reading of, in preference order.
    pub fn clocks(&self) -> Vec<LinuxClock> {
        LinuxClock::PREFERENCE
            .into_iter()
            .filter(|clock| self.samples.iter().all(|sample| sample.host_us[clock.index()].is_some()))
            .collect()
    }

    /// Smallest delay of each clock over the newest `recent` samples (all of them for 0).
    pub fn recent_delays_us(&self) -> Vec<(LinuxClock, i64)> {
        let skip = match self.options.recent {
            0 => 0,
            recent => self.samples.len().saturating_sub(recent),
        };
        self.clocks()
            .into_iter()
            .filter_map(|clock| {
                let smallest = self.samples.iter().skip(skip).filter_map(|sample| sample.delay_us(clock)).min()?;
                Some((clock, smallest))
            })
            .collect()
    }

    /// Median pairwise drift of each clock against the counter (Theil-Sen slope).
    pub fn drift_ppm(&self) -> Vec<(LinuxClock, f64)> {
        let samples: Vec<&QpcSample> = self.samples.iter().collect();
        let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
            return Vec::new();
        };
        if samples.len() < 3 || last.qpc_us - first.qpc_us < self.options.min_rate_span_us {
            return Vec::new();
        }
        let mut pairs = Vec::new();
        for (index, a) in samples.iter().enumerate() {
            for b in &samples[index + 1..] {
                if b.qpc_us - a.qpc_us >= RATE_MIN_PAIR_GAP_US {
                    pairs.push((*a, *b));
                }
            }
        }
        if pairs.is_empty() {
            return Vec::new();
        }
        self.clocks()
            .into_iter()
            .map(|clock| {
                let slopes = pairs
                    .iter()
                    .map(|(a, b)| {
                        let change = b.delay_us(clock).expect("clock read") - a.delay_us(clock).expect("clock read");
                        change as f64 / (b.qpc_us - a.qpc_us) as f64
                    })
                    .collect();
                (clock, 1e6 * median(slopes))
            })
            .collect()
    }

    pub fn verdict(&self, current: Option<LinuxClock>) -> ClockVerdict {
        if self.samples.is_empty() {
            return ClockVerdict { clock: None, basis: Basis::Undecided, delays_us: Vec::new(), rates_ppm: Vec::new() };
        }
        let (delays_us, rates_ppm) = (self.recent_delays_us(), self.drift_ppm());
        let matches: Vec<(LinuxClock, i64)> =
            delays_us.iter().copied().filter(|(_, delay)| delay.abs() <= self.options.tolerance_us).collect();
        if !matches.is_empty() {
            let chosen = match current {
                Some(current) if matches.iter().any(|(clock, _)| *clock == current) => current,
                _ => closest(&matches, i64::abs).expect("matches"),
            };
            return ClockVerdict { clock: Some(chosen), basis: Basis::Absolute, delays_us, rates_ppm };
        }
        let Some(mut steadiest) = closest(&rates_ppm, f64::abs) else {
            return ClockVerdict { clock: None, basis: Basis::Undecided, delays_us, rates_ppm };
        };
        let rate = |clock: LinuxClock| rates_ppm.iter().find(|(c, _)| *c == clock).map(|(_, rate)| rate.abs());
        if let (Some(current), Some(best)) = (current, rate(steadiest))
            && rate(current).is_some_and(|rate| rate - best <= self.options.rate_margin_ppm)
        {
            steadiest = current;
        }
        ClockVerdict { clock: Some(steadiest), basis: Basis::Rate, delays_us, rates_ppm }
    }
}

/// Reads one Linux clock in microseconds.
pub trait ClockReader {
    type Error;

    fn read_us(&self, clock: LinuxClock) -> Result<i64, Self::Error>;
}

/// The host's clocks through `clock_gettime`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxClocks;

impl ClockReader for LinuxClocks {
    type Error = Infallible;

    fn read_us(&self, clock: LinuxClock) -> Result<i64, Infallible> {
        let now = rustix::time::clock_gettime(clock.id());
        Ok(now.tv_sec * 1_000_000 + now.tv_nsec / 1000)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSwitch {
    pub old: LinuxClock,
    pub new: LinuxClock,
    pub delta_us: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockState {
    pub generation: i64,
    pub source: LinuxClock,
    pub offset_us: i64,
}

/// Microsecond service clock reading one Linux clock; the source can change at run time.
///
/// Calls are safe from any thread; the state lock is held only to copy or replace the
/// state, never across a clock read. `generation` grows with every switch, so users that
/// keep offsets to this clock (`tobii_device::clock::ClockMapper`) can start over.
#[derive(Debug)]
pub struct ServiceClock<R> {
    reader: R,
    automatic: bool,
    state: Mutex<ClockState>,
}

impl<R: ClockReader> ServiceClock<R> {
    pub fn new(source: LinuxClock, automatic: bool, reader: R) -> Self {
        Self { reader, automatic, state: Mutex::new(ClockState { generation: 0, source, offset_us: 0 }) }
    }

    fn lock(&self) -> MutexGuard<'_, ClockState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn reader(&self) -> &R {
        &self.reader
    }

    pub fn automatic(&self) -> bool {
        self.automatic
    }

    pub fn state(&self) -> ClockState {
        *self.lock()
    }

    pub fn now_us(&self) -> Result<i64, R::Error> {
        let state = self.state();
        Ok(self.reader.read_us(state.source)? + state.offset_us)
    }

    /// Every candidate clock, read back to back in preference order.
    pub fn read_all(&self) -> Result<[i64; 3], R::Error> {
        Ok([
            self.reader.read_us(LinuxClock::Boottime)?,
            self.reader.read_us(LinuxClock::Raw)?,
            self.reader.read_us(LinuxClock::Monotonic)?,
        ])
    }

    /// Read `source` from now on: its own value, or continuing from the current value.
    pub fn switch(&self, source: LinuxClock, keep_value: bool) -> Result<ClockSwitch, R::Error> {
        let state = self.state();
        let old_now = self.now_us()?;
        let new_now = self.reader.read_us(source)?;
        let offset_us = if keep_value { old_now - new_now } else { 0 };
        *self.lock() = ClockState { generation: state.generation + 1, source, offset_us };
        Ok(ClockSwitch { old: state.source, new: source, delta_us: new_now + offset_us - old_now })
    }
}

impl HostClock for ServiceClock<LinuxClocks> {
    fn now_us(&self) -> i64 {
        let Ok(now) = ServiceClock::now_us(self);
        now
    }

    fn generation(&self) -> Option<i64> {
        Some(self.state().generation)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClockDecision<K> {
    pub verdict: ClockVerdict,
    pub switch: Option<ClockSwitch>,
    pub held_by: Option<K>,
}

#[derive(Debug, Clone)]
struct Bridge<K> {
    key: K,
    classifier: QpcClockClassifier,
    verdict: Option<LinuxClock>,
}

/// Keeps a ServiceClock on the clock behind the bridges' QueryPerformanceCounter.
///
/// Each bridge (one process in one prefix) gets its own classifier. A fixed clock is never
/// switched; its verdicts only report what the bridge's counter follows. An automatic
/// clock adopts a bridge's verdict unless another bridge still confirms the current
/// source, so two prefixes on different Wine builds do not make the clock flap; `held_by`
/// names that bridge.
#[derive(Debug, Clone)]
pub struct QpcClockFollower<K> {
    bridges: Vec<Bridge<K>>,
}

impl<K> Default for QpcClockFollower<K> {
    fn default() -> Self {
        Self { bridges: Vec::new() }
    }
}

impl<K: Clone + PartialEq> QpcClockFollower<K> {
    pub fn observe<R: ClockReader>(
        &mut self,
        clock: &ServiceClock<R>,
        bridge: K,
        sample: QpcSample,
    ) -> Result<ClockDecision<K>, R::Error> {
        let source = clock.state().source;
        let index = match self.bridges.iter().position(|entry| entry.key == bridge) {
            Some(index) => index,
            None => {
                self.bridges.push(Bridge {
                    key: bridge.clone(),
                    classifier: QpcClockClassifier::default(),
                    verdict: None,
                });
                self.bridges.len() - 1
            }
        };
        let entry = &mut self.bridges[index];
        entry.classifier.add(sample);
        let verdict = entry.classifier.verdict(Some(source));
        entry.verdict = verdict.clock;
        let Some(chosen) = verdict.clock.filter(|chosen| clock.automatic() && *chosen != source) else {
            return Ok(ClockDecision { verdict, switch: None, held_by: None });
        };
        let holder = self.bridges.iter().find(|other| other.key != bridge && other.verdict == Some(source));
        if let Some(holder) = holder {
            return Ok(ClockDecision { verdict, switch: None, held_by: Some(holder.key.clone()) });
        }
        let switch = clock.switch(chosen, verdict.basis == Basis::Rate)?;
        Ok(ClockDecision { verdict, switch: Some(switch), held_by: None })
    }

    pub fn forget(&mut self, bridge: &K) {
        self.bridges.retain(|entry| entry.key != *bridge);
    }

    pub fn verdicts(&self) -> Vec<(K, Option<LinuxClock>)> {
        self.bridges.iter().map(|entry| (entry.key.clone(), entry.verdict)).collect()
    }
}

/// Service-clock microseconds as the game's QueryPerformanceCounter microseconds.
///
/// PRP clients learn the offset by timesync; routes without timesync (SESP head pose,
/// tet-tcp gaze) need timestamps already on the game's counter. When the service clock
/// reads the Linux clock behind the counter, the two are equal up to the clock's own
/// switch offset. Otherwise (an unanchored counter) the offset is the smallest recent
/// difference between the clock at receipt of a bridge report and the counter it carried.
/// Without any report the counter is assumed to be the service clock's source, as with
/// --wine-clock set to it.
#[derive(Debug, Clone)]
pub struct CounterMapping {
    samples: VecDeque<QpcSample>,
    recent: usize,
    tolerance_us: i64,
}

impl Default for CounterMapping {
    fn default() -> Self {
        Self::new(RECENT_REPORTS, ABSOLUTE_TOLERANCE_US)
    }
}

impl CounterMapping {
    pub fn new(recent: usize, tolerance_us: i64) -> Self {
        Self { samples: VecDeque::with_capacity(recent), recent, tolerance_us }
    }

    pub fn observe(&mut self, sample: QpcSample) {
        if self.recent == 0 {
            return;
        }
        if self.samples.len() == self.recent {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// Service clock minus game counter, for a clock in `state`.
    pub fn offset_us(&self, state: &ClockState) -> i64 {
        let smallest = self.samples.iter().filter_map(|sample| sample.delay_us(state.source)).min();
        match smallest {
            Some(delay) if delay.abs() > self.tolerance_us => state.offset_us + delay,
            _ => state.offset_us,
        }
    }

    pub fn map(&self, state: &ClockState, service_us: i64) -> i64 {
        service_us - self.offset_us(state)
    }
}

#[cfg(test)]
mod tests;
