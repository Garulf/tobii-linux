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
matches, the one that keeps the steadiest offset.
"""

from collections import deque
from dataclasses import dataclass
import statistics
import struct
import time

LINUX_CLOCKS = {
    "boottime": time.CLOCK_BOOTTIME,
    "raw": time.CLOCK_MONOTONIC_RAW,
    "monotonic": time.CLOCK_MONOTONIC,
}
# Among clocks that fit equally well: the one current upstream Wine reads first, then Proton's.
CLOCK_PREFERENCE = ("boottime", "raw", "monotonic")
AUTO = "auto"
DEFAULT_SOURCE = "boottime"
WINE_CLOCKS = (AUTO, "boottime", "raw")

ABSOLUTE = "absolute"
RATE = "rate"
UNDECIDED = "undecided"

# A clock matches the counter when the smallest recent receipt delay is within this.
ABSOLUTE_TOLERANCE_US = 10_000
# Reports whose smallest delay decides the absolute match; a few cover latency spikes.
RECENT_REPORTS = 4
# Reports kept for the rate comparison: about a minute at the bridge's 2 s period.
RATE_WINDOW_REPORTS = 32
RATE_MIN_SPAN_US = 6_000_000
RATE_MIN_PAIR_GAP_US = 1_000_000
# Another clock replaces the current one on rate alone only when it is steadier by this much.
RATE_MARGIN_PPM = 2_000

QPC_REPORT = struct.Struct("<QQ")


def linux_clock_us(name: str) -> int:
    return time.clock_gettime_ns(LINUX_CLOCKS[name]) // 1000


@dataclass(frozen=True)
class QpcReport:
    """One bridge report: QueryPerformanceCounter, its frequency and optional runtime details."""
    counter: int
    frequency: int
    details: dict[str, str]

    @property
    def micros(self) -> int:
        return self.counter * 1_000_000 // self.frequency


def encode_qpc_report(counter: int, frequency: int, details: dict[str, str] | None = None) -> bytes:
    text = "".join(f"{key}={value}\n" for key, value in (details or {}).items())
    return QPC_REPORT.pack(counter, frequency) + text.encode("utf-8")


def decode_qpc_report(payload: bytes) -> QpcReport:
    if len(payload) < QPC_REPORT.size:
        raise ValueError("truncated QueryPerformanceCounter report")
    counter, frequency = QPC_REPORT.unpack_from(payload)
    if frequency == 0:
        raise ValueError("QueryPerformanceCounter report without a frequency")
    details = {}
    for line in payload[QPC_REPORT.size:].decode("utf-8", "replace").splitlines():
        key, separator, value = line.partition("=")
        if separator:
            details[key] = value
    return QpcReport(counter, frequency, details)


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
        delays = ", ".join(f"{name} {delay / 1000:+.1f} ms" for name, delay in self.delays_us.items())
        rates = ", ".join(f"{name} {rate:+.0f} ppm" for name, rate in self.rates_ppm.items())
        return f"{self.basis}; clock minus counter: {delays}" + (f"; drift: {rates}" if rates else "")


class QpcClockClassifier:
    """Decides which Linux clock one process's QueryPerformanceCounter follows.

    A clock whose value equals the counter (within the receipt delay) is the
    source: Wine's counter is that clock's absolute value in 100 ns units. When
    no clock matches, as with an unanchored counter, the clock whose offset to the
    counter drifts least over the retained reports is chosen instead. The current
    clock is kept while it still fits, so equal clocks do not cause switches.
    """

    def __init__(self, tolerance_us: int = ABSOLUTE_TOLERANCE_US, recent: int = RECENT_REPORTS,
                 window: int = RATE_WINDOW_REPORTS, min_rate_span_us: int = RATE_MIN_SPAN_US,
                 rate_margin_ppm: float = RATE_MARGIN_PPM):
        self.tolerance_us, self.recent = tolerance_us, recent
        self.min_rate_span_us, self.rate_margin_ppm = min_rate_span_us, rate_margin_ppm
        self.samples: deque[QpcSample] = deque(maxlen=window)

    def add(self, sample: QpcSample) -> None:
        if self.samples and sample.qpc_us < self.samples[-1].qpc_us:
            self.samples.clear()
        self.samples.append(sample)

    def clocks(self) -> list[str]:
        return [name for name in CLOCK_PREFERENCE if all(name in s.host_us for s in self.samples)]

    def recent_delays_us(self) -> dict[str, int]:
        recent = list(self.samples)[-self.recent:]
        return {name: min(sample.delay_us(name) for sample in recent) for name in self.clocks()}

    def drift_ppm(self) -> dict[str, float]:
        """Median pairwise drift of each clock against the counter (Theil-Sen slope)."""
        samples = list(self.samples)
        if len(samples) < 3 or samples[-1].qpc_us - samples[0].qpc_us < self.min_rate_span_us:
            return {}
        pairs = [(a, b) for index, a in enumerate(samples) for b in samples[index + 1:]
                 if b.qpc_us - a.qpc_us >= RATE_MIN_PAIR_GAP_US]
        if not pairs:
            return {}
        return {name: 1e6 * statistics.median((b.delay_us(name) - a.delay_us(name))
                                              / (b.qpc_us - a.qpc_us) for a, b in pairs)
                for name in self.clocks()}

    def verdict(self, current: str | None = None) -> ClockVerdict:
        if not self.samples:
            return ClockVerdict(None, UNDECIDED, {}, {})
        delays, rates = self.recent_delays_us(), self.drift_ppm()
        matches = [name for name, delay in delays.items() if abs(delay) <= self.tolerance_us]
        if matches:
            chosen = current if current in matches else min(matches, key=lambda n: abs(delays[n]))
            return ClockVerdict(chosen, ABSOLUTE, delays, rates)
        if not rates:
            return ClockVerdict(None, UNDECIDED, delays, rates)
        steadiest = min(rates, key=lambda name: abs(rates[name]))
        if current in rates and abs(rates[current]) - abs(rates[steadiest]) <= self.rate_margin_ppm:
            steadiest = current
        return ClockVerdict(steadiest, RATE, delays, rates)


@dataclass(frozen=True)
class ClockSwitch:
    old: str
    new: str
    delta_us: int


class ServiceClock:
    """Microsecond service clock reading one Linux clock; the source can change at run time.

    Calls are safe from any thread. `generation` grows with every switch, so
    users that keep offsets to this clock (driver.ClockMapper) can start over.
    `read_us(name)` reads one of LINUX_CLOCKS in microseconds.
    """

    def __init__(self, source: str = DEFAULT_SOURCE, automatic: bool = False,
                 read_us=linux_clock_us):
        if source not in LINUX_CLOCKS:
            raise ValueError(f"Unknown Linux clock {source!r}; expected one of {tuple(LINUX_CLOCKS)}")
        self.automatic, self.read_us = automatic, read_us
        self._state = (0, source, 0)

    def __call__(self) -> int:
        _, source, offset_us = self._state
        return self.read_us(source) + offset_us

    @property
    def source(self) -> str:
        return self._state[1]

    @property
    def generation(self) -> int:
        return self._state[0]

    @property
    def offset_us(self) -> int:
        return self._state[2]

    def read_all(self) -> dict[str, int]:
        """Every candidate clock, read back to back."""
        return {name: self.read_us(name) for name in CLOCK_PREFERENCE}

    def switch(self, source: str, keep_value: bool = False) -> ClockSwitch:
        """Read `source` from now on: its own value, or continuing from the current value."""
        generation, old_source, _ = self._state
        old_now = self()
        new_now = self.read_us(source)
        offset_us = old_now - new_now if keep_value else 0
        self._state = (generation + 1, source, offset_us)
        return ClockSwitch(old_source, source, new_now + offset_us - old_now)


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
        self.classifiers: dict[object, QpcClockClassifier] = {}
        self.verdicts: dict[object, str | None] = {}

    def observe(self, bridge, sample: QpcSample) -> ClockDecision:
        classifier = self.classifiers.setdefault(bridge, QpcClockClassifier())
        classifier.add(sample)
        verdict = classifier.verdict(self.clock.source)
        self.verdicts[bridge] = verdict.clock
        if not self.clock.automatic or verdict.clock in (None, self.clock.source):
            return ClockDecision(verdict)
        holder = next((other for other, clock in self.verdicts.items()
                       if other is not bridge and clock == self.clock.source), None)
        if holder is not None:
            return ClockDecision(verdict, held_by=holder)
        return ClockDecision(verdict, self.clock.switch(verdict.clock, keep_value=verdict.basis == RATE))

    def forget(self, bridge) -> None:
        self.classifiers.pop(bridge, None)
        self.verdicts.pop(bridge, None)


class CounterMapping:
    """Service-clock microseconds as the game's QueryPerformanceCounter microseconds.

    PRP clients learn the offset by timesync; routes without timesync (SESP head
    pose, tet-tcp gaze) need timestamps already on the game's counter. When the
    service clock reads the Linux clock behind the counter, the two are equal up
    to the clock's own switch offset. Otherwise (an unanchored counter) the
    offset is the smallest recent difference between the clock at receipt of a
    bridge report and the counter it carried. Without any report the counter is
    assumed to be the service clock's source, as with --wine-clock set to it.
    """

    def __init__(self, clock: ServiceClock, recent: int = RECENT_REPORTS,
                 tolerance_us: int = ABSOLUTE_TOLERANCE_US):
        self.clock, self.tolerance_us = clock, tolerance_us
        self.samples: deque[QpcSample] = deque(maxlen=recent)

    def observe(self, sample: QpcSample) -> None:
        self.samples.append(sample)

    def offset_us(self) -> int:
        """Service clock minus game counter."""
        source = self.clock.source
        delays = [sample.delay_us(source) for sample in self.samples if source in sample.host_us]
        if not delays or abs(min(delays)) <= self.tolerance_us:
            return self.clock.offset_us
        return self.clock.offset_us + min(delays)

    def __call__(self, service_us: int) -> int:
        return service_us - self.offset_us()
