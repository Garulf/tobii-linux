"""Service clock that ticks at the rate of the game's Wine QueryPerformanceCounter.

The game DLL converts stream timestamps with an offset measured by timesync only,
so the service clock must run at the rate of the client's counter. Every Wine and
Proton build examined returns a Linux clock in 100 ns units from QueryPerformanceCounter:
CLOCK_BOOTTIME from upstream Wine 10.17, CLOCK_MONOTONIC_RAW before that and in
every Proton tree. GE-Proton 11-7 and later may serve it from the TSC in user mode,
calibrated against and anchored on CLOCK_MONOTONIC_RAW. docs/prp-bridge.md (Time
base) has the source references.

The bridge inside the prefix reports its QueryPerformanceCounter periodically. In
automatic mode the service compares each report with the Linux clocks read at
receipt and follows the clock whose value matches the counter, or, when none
matches, the one that keeps the steadiest offset. Implemented in
crates/tobii-service/src/qpc_clock.rs; the dataclasses here carry its values.
"""

from dataclasses import dataclass
import itertools
import time

from ._native import qpc_clock as _qpc

LINUX_CLOCKS = {
    "boottime": time.CLOCK_BOOTTIME,
    "raw": time.CLOCK_MONOTONIC_RAW,
    "monotonic": time.CLOCK_MONOTONIC,
}
# Among clocks that fit equally well: the one current upstream Wine reads first, then Proton's.
CLOCK_PREFERENCE = _qpc.CLOCK_PREFERENCE
AUTO = "auto"
DEFAULT_SOURCE = _qpc.DEFAULT_SOURCE
WINE_CLOCKS = (AUTO, "boottime", "raw")

ABSOLUTE = "absolute"
RATE = "rate"
UNDECIDED = "undecided"

# A clock matches the counter when the smallest recent receipt delay is within this.
ABSOLUTE_TOLERANCE_US = _qpc.ABSOLUTE_TOLERANCE_US
# Reports whose smallest delay decides the absolute match; a few cover latency spikes.
RECENT_REPORTS = _qpc.RECENT_REPORTS
# Reports kept for the rate comparison: about a minute at the bridge's 2 s period.
RATE_WINDOW_REPORTS = _qpc.RATE_WINDOW_REPORTS
RATE_MIN_SPAN_US = _qpc.RATE_MIN_SPAN_US
RATE_MIN_PAIR_GAP_US = _qpc.RATE_MIN_PAIR_GAP_US
# Another clock replaces the current one on rate alone only when it is steadier by this much.
RATE_MARGIN_PPM = _qpc.RATE_MARGIN_PPM


def linux_clock_us(name: str) -> int:
    return _qpc.linux_clock_us(name)


@dataclass(frozen=True)
class QpcReport:
    """One bridge report: QueryPerformanceCounter, its frequency and optional runtime details."""
    counter: int
    frequency: int
    details: dict[str, str]

    @property
    def micros(self) -> int:
        return _qpc.micros(self.counter, self.frequency)


def encode_qpc_report(counter: int, frequency: int, details: dict[str, str] | None = None) -> bytes:
    return _qpc.encode_qpc_report(counter, frequency, details)


def decode_qpc_report(payload: bytes) -> QpcReport:
    return QpcReport(*_qpc.decode_qpc_report(payload))


@dataclass(frozen=True)
class QpcSample:
    """A counter value in microseconds and each Linux clock read when it arrived."""
    qpc_us: int
    host_us: dict[str, int]

    def delay_us(self, clock: str) -> int:
        return self.host_us[clock] - self.qpc_us


@dataclass(frozen=True)
class ClockVerdict:
    clock: str | None
    basis: str
    delays_us: dict[str, int]
    rates_ppm: dict[str, float]

    def describe(self) -> str:
        return _qpc.describe(self.basis, list(self.delays_us.items()), list(self.rates_ppm.items()))


class QpcClockClassifier(_qpc.QpcClockClassifier):
    """Decides which Linux clock one process's QueryPerformanceCounter follows.

    A clock whose value equals the counter (within the receipt delay) is the
    source: Wine's counter is that clock's absolute value in 100 ns units. When
    no clock matches, as with an unanchored counter, the clock whose offset to the
    counter drifts least over the retained reports is chosen instead. The current
    clock is kept while it still fits, so equal clocks do not cause switches.
    """

    @property
    def samples(self) -> list[QpcSample]:
        return [QpcSample(*values) for values in self.sample_values()]

    def verdict(self, current: str | None = None) -> ClockVerdict:
        return ClockVerdict(*super().verdict(current))


@dataclass(frozen=True)
class ClockSwitch:
    old: str
    new: str
    delta_us: int


class ServiceClock(_qpc.ServiceClock):
    """Microsecond service clock reading one Linux clock; the source can change at run time.

    Calls are safe from any thread. `generation` grows with every switch, so
    users that keep offsets to this clock (driver.ClockMapper) can start over.
    `read_us(name)` reads one of LINUX_CLOCKS in microseconds; a `read_us`
    callable given to the constructor replaces the host clocks.
    """

    def switch(self, source: str, keep_value: bool = False) -> ClockSwitch:
        """Read `source` from now on: its own value, or continuing from the current value."""
        return ClockSwitch(*super().switch(source, keep_value))


@dataclass(frozen=True)
class ClockDecision:
    verdict: ClockVerdict
    switch: ClockSwitch | None = None
    held_by: object = None


class QpcClockFollower:
    """Keeps a ServiceClock on the clock behind the bridges' QueryPerformanceCounter.

    Each bridge (one process in one prefix) gets its own classifier. A fixed
    clock is never switched; its verdicts only report what the bridge's counter
    follows. An automatic clock adopts a bridge's verdict unless another bridge
    still confirms the current source, so two prefixes on different Wine builds
    do not make the clock flap; `held_by` names that bridge.
    """

    def __init__(self, clock: ServiceClock):
        self.clock = clock
        self._native = _qpc.QpcClockFollower(clock)
        self._keys: dict[object, int] = {}
        self._numbers = itertools.count()

    def observe(self, bridge, sample: QpcSample) -> ClockDecision:
        if bridge not in self._keys:
            self._keys[bridge] = next(self._numbers)
        verdict, switch, holder = self._native.observe(self._keys[bridge], sample)
        held_by = next((other for other, key in self._keys.items() if key == holder), None)
        return ClockDecision(ClockVerdict(*verdict), None if switch is None else ClockSwitch(*switch),
                             held_by)

    def forget(self, bridge) -> None:
        key = self._keys.pop(bridge, None)
        if key is not None:
            self._native.forget(key)


# Service-clock microseconds as the game's QueryPerformanceCounter microseconds:
# CounterMapping(clock)(service_us), fed with every bridge report through observe().
CounterMapping = _qpc.CounterMapping
