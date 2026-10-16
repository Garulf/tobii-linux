//! The service clock: microseconds at the rate of the game's Wine QueryPerformanceCounter.
//!
//! The game DLL converts stream timestamps with an offset measured by timesync, so the
//! service clock must tick at the same rate as the client's counter. Wine 10.17 and later
//! read CLOCK_BOOTTIME ("boottime"); 10.16 and earlier and every Proton read
//! CLOCK_MONOTONIC_RAW ("raw"). The two can drift apart by percent under WSL2. "auto"
//! starts on boottime and follows the clock the bridge's counter reports.

use std::convert::Infallible;
use std::sync::Arc;

use tobii_device::clock::HostClock;
use tobii_service::qpc_clock::{AUTO, ClockReader, DEFAULT_SOURCE, LinuxClock, LinuxClocks, ServiceClock};

pub const WINE_CLOCKS: [&str; 3] = [AUTO, "boottime", "raw"];

/// The host's clocks, or scripted clocks for tests.
#[derive(Clone)]
pub enum Clocks {
    Host,
    Scripted(Arc<dyn Fn(LinuxClock) -> i64 + Send + Sync>),
}

impl std::fmt::Debug for Clocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Clocks::Host => "Clocks::Host",
            Clocks::Scripted(_) => "Clocks::Scripted",
        })
    }
}

impl ClockReader for Clocks {
    type Error = Infallible;

    fn read_us(&self, clock: LinuxClock) -> Result<i64, Infallible> {
        match self {
            Clocks::Host => LinuxClocks.read_us(clock),
            Clocks::Scripted(read) => Ok(read(clock)),
        }
    }
}

pub type Clock = ServiceClock<Clocks>;

/// The service clock for a `--wine-clock` choice; `None` for an unknown name.
pub fn wine_clock(name: &str, clocks: Clocks) -> Option<Arc<Clock>> {
    if name == AUTO {
        return Some(Arc::new(ServiceClock::new(DEFAULT_SOURCE, true, clocks)));
    }
    let source = LinuxClock::from_name(name).filter(|_| WINE_CLOCKS.contains(&name))?;
    Some(Arc::new(ServiceClock::new(source, false, clocks)))
}

pub fn now_us(clock: &Clock) -> i64 {
    let Ok(now) = clock.now_us();
    now
}

pub fn readings(clock: &Clock) -> [i64; 3] {
    let Ok(readings) = clock.read_all();
    readings
}

/// The service clock as a host clock for the tracker's clock mappers.
#[derive(Debug, Clone)]
pub struct ServiceTime(pub Arc<Clock>);

impl HostClock for ServiceTime {
    fn now_us(&self) -> i64 {
        now_us(&self.0)
    }

    fn generation(&self) -> Option<i64> {
        Some(self.0.state().generation)
    }
}
