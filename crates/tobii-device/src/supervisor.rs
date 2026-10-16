//! Keeps a tracker session running across unplugging, USB errors, stalls and host suspend.
//!
//! The Windows runtime treats every such event as the device being removed: it shuts its
//! client server down and starts the device again from scratch once it is back
//! (docs/service-behaviour.md). [`TrackerLink`] does the same for one Linux process: it
//! opens a session, runs it until it fails, reports the loss, waits with a growing delay
//! and opens a new one. A session that ran long enough resets the delay.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::error::DeviceError;
use crate::stop::StopSignal;

/// Delays between session attempts: doubling from `first_s` up to `max_s`.
#[derive(Debug, Clone, PartialEq)]
pub struct Backoff {
    pub first_s: f64,
    pub max_s: f64,
    pub healthy_s: f64,
    pub next_s: f64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(0.5, 2.0, 10.0)
    }
}

impl Backoff {
    pub fn new(first_s: f64, max_s: f64, healthy_s: f64) -> Self {
        Self { first_s, max_s, healthy_s, next_s: first_s }
    }

    /// Delay before the next attempt after a session that lasted `session_s` seconds.
    pub fn delay(&mut self, session_s: f64) -> f64 {
        if session_s >= self.healthy_s {
            self.next_s = self.first_s;
        }
        let delay = self.next_s;
        self.next_s = (self.next_s * 2.0).min(self.max_s);
        delay
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// No tracker is connected.
    Searching,
    Starting,
    /// A session ended, with or without an error.
    Lost,
    Stopped,
}

impl LinkState {
    pub fn name(self) -> &'static str {
        match self {
            LinkState::Searching => "searching",
            LinkState::Starting => "starting",
            LinkState::Lost => "lost",
            LinkState::Stopped => "stopped",
        }
    }
}

/// How a failed attempt is reported.
pub trait LinkFailure: std::fmt::Display {
    /// No tracker was found; reported as searching rather than lost.
    fn is_no_tracker(&self) -> bool;
    /// Error kind printed before the message, as `Kind: message`.
    fn kind(&self) -> String;
}

impl LinkFailure for DeviceError {
    fn is_no_tracker(&self) -> bool {
        matches!(self, DeviceError::NoTracker(_))
    }

    fn kind(&self) -> String {
        match self {
            DeviceError::Protocol(_) => "ProtocolError",
            DeviceError::Channel(_) | DeviceError::Usb(_) => "USBError",
            _ => "DeviceError",
        }
        .into()
    }
}

/// What one attempt and its reports need from the caller.
pub trait LinkHost {
    type Error: LinkFailure;

    fn on_state(&mut self, state: LinkState, detail: &str) -> Result<(), Self::Error>;

    /// Opens a session, counts it with `opened`, runs it until it ends and closes it.
    fn attempt(&mut self, stop: &dyn StopSignal, opened: &mut dyn FnMut()) -> Result<(), Self::Error>;

    /// Seconds on a monotonic clock.
    fn now_s(&mut self) -> f64;
}

/// Sessions opened and lost so far; readable from other threads while the link runs.
#[derive(Debug, Default)]
pub struct LinkCounters {
    sessions: AtomicU64,
    failures: AtomicU64,
}

impl LinkCounters {
    pub fn sessions(&self) -> u64 {
        self.sessions.load(Ordering::SeqCst)
    }

    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrackerLink {
    pub backoff: Backoff,
    pub counters: Arc<LinkCounters>,
}

impl TrackerLink {
    pub fn new(backoff: Backoff) -> Self {
        Self { backoff, counters: Arc::default() }
    }

    pub fn sessions(&self) -> u64 {
        self.counters.sessions()
    }

    pub fn failures(&self) -> u64 {
        self.counters.failures()
    }

    /// Runs sessions until `stop` is set; every error of an attempt counts as a lost session.
    pub fn run<H: LinkHost>(&mut self, host: &mut H, stop: &dyn StopSignal) {
        self.run_with(host, stop, |backoff, session_s| backoff.delay(session_s));
    }

    /// [`TrackerLink::run`] with a caller-chosen delay policy.
    pub fn run_with<H: LinkHost>(
        &mut self,
        host: &mut H,
        stop: &dyn StopSignal,
        mut delay: impl FnMut(&mut Backoff, f64) -> f64,
    ) {
        while !stop.is_set() {
            let started = host.now_s();
            let counters = Arc::clone(&self.counters);
            let outcome = host.on_state(LinkState::Starting, "Starting the tracker").and_then(|()| {
                host.attempt(stop, &mut || {
                    counters.sessions.fetch_add(1, Ordering::SeqCst);
                })?;
                if !stop.is_set() {
                    host.on_state(LinkState::Lost, "The tracker session ended")?;
                }
                Ok(())
            });
            if let Err(error) = outcome {
                let report = if error.is_no_tracker() {
                    host.on_state(LinkState::Searching, &error.to_string())
                } else {
                    self.counters.failures.fetch_add(1, Ordering::SeqCst);
                    log::warn!("tracker session ended: {}: {error}", error.kind());
                    host.on_state(LinkState::Lost, &format!("{}: {error}", error.kind()))
                };
                if report.is_err() {
                    log::warn!("a state report failed");
                }
            }
            if stop.is_set() {
                break;
            }
            let session_s = host.now_s() - started;
            stop.wait(Duration::from_secs_f64(delay(&mut self.backoff, session_s).max(0.0)));
        }
        let _ = host.on_state(LinkState::Stopped, "Stopped");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::stop::StopFlag;

    #[test]
    fn delays_double_up_to_the_limit_and_reset_after_a_healthy_session() {
        let mut backoff = Backoff::new(0.5, 4.0, 10.0);
        let delays: Vec<f64> = (0..5).map(|_| backoff.delay(0.1)).collect();
        assert_eq!(delays, [0.5, 1.0, 2.0, 4.0, 4.0]);
        assert_eq!(backoff.delay(30.0), 0.5);
        assert_eq!(backoff.delay(0.1), 1.0);
    }

    struct Scripted {
        attempts: usize,
        states: Vec<&'static str>,
        stop: StopFlag,
        started: Instant,
    }

    impl LinkHost for Scripted {
        type Error = DeviceError;

        fn on_state(&mut self, state: LinkState, _detail: &str) -> Result<(), DeviceError> {
            self.states.push(state.name());
            Ok(())
        }

        fn attempt(&mut self, _stop: &dyn StopSignal, opened: &mut dyn FnMut()) -> Result<(), DeviceError> {
            self.attempts += 1;
            match self.attempts {
                1 => Err(DeviceError::NoTracker("No Eye Tracker 5 is visible".into())),
                2 => {
                    opened();
                    Err(DeviceError::Silent { seconds: 3.0 })
                }
                _ => {
                    opened();
                    self.stop.set();
                    Ok(())
                }
            }
        }

        fn now_s(&mut self) -> f64 {
            self.started.elapsed().as_secs_f64()
        }
    }

    #[test]
    fn lost_and_missing_sessions_are_retried_until_stopped() {
        let stop = StopFlag::new();
        let mut host = Scripted { attempts: 0, states: Vec::new(), stop: stop.clone(), started: Instant::now() };
        let mut link = TrackerLink::new(Backoff::new(0.01, 0.02, 10.0));
        link.run(&mut host, &stop);
        assert_eq!(host.states, ["starting", "searching", "starting", "lost", "starting", "stopped"]);
        assert_eq!((link.sessions(), link.failures()), (2, 1));
    }

    #[test]
    fn stop_interrupts_the_wait() {
        struct Absent;
        impl LinkHost for Absent {
            type Error = DeviceError;
            fn on_state(&mut self, _: LinkState, _: &str) -> Result<(), DeviceError> {
                Ok(())
            }
            fn attempt(&mut self, _: &dyn StopSignal, _: &mut dyn FnMut()) -> Result<(), DeviceError> {
                Err(DeviceError::NoTracker("absent".into()))
            }
            fn now_s(&mut self) -> f64 {
                0.0
            }
        }
        let stop = StopFlag::new();
        let remote = stop.clone();
        let thread =
            std::thread::spawn(move || TrackerLink::new(Backoff::new(60.0, 60.0, 10.0)).run(&mut Absent, &stop));
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        remote.set();
        thread.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
