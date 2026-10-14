"""Choosing the Linux clock behind Wine's QueryPerformanceCounter from bridge reports.

Synthetic hosts drive every case: a true elapsed time and, for each Linux clock,
its offset and rate against it. A report carries the counter of one of those
clocks (or of an unanchored counter) and is classified as it would be at receipt.
"""
import asyncio
import logging
import struct
import unittest

from tobii_linux import driver
from tobii_linux.head_pose_pipeline import HeadPoseSample
from tobii_linux.prp_service import (
    FRAME_BRIDGE_HELLO, FRAME_PIPES, FRAME_QPC, HeadPoseHub, PrpService, encode_frame,
    read_frame, wine_clock)
from tobii_linux.qpc_clock import (
    ABSOLUTE, AUTO, RATE, UNDECIDED, QpcClockClassifier, QpcClockFollower, QpcSample,
    ServiceClock, decode_qpc_report, encode_qpc_report)

WINE_FREQUENCY = 10_000_000
BOOT_US = 5_000_000_000
# The reference host's WSL2 kernel: CLOCK_MONOTONIC_RAW runs 9.1% faster than CLOCK_MONOTONIC.
WSL_RAW_RATE = 1.091


class SyntheticHost:
    """Linux clocks as offset + rate * elapsed microseconds, read through ServiceClock.read_us."""

    def __init__(self, rates=None, offsets=None):
        self.elapsed_us = 0
        self.rates = {"boottime": 1.0, "raw": 1.0, "monotonic": 1.0} | (rates or {})
        self.offsets = {"boottime": BOOT_US, "raw": BOOT_US, "monotonic": BOOT_US} | (offsets or {})

    def read_us(self, name: str) -> int:
        return self.offsets[name] + round(self.rates[name] * self.elapsed_us)

    def suspend(self, seconds: float) -> None:
        self.offsets["boottime"] += round(seconds * 1e6)

    def counter(self, name: str, latency_us: int = 300) -> int:
        """Wine's QueryPerformanceCounter on `name`, read `latency_us` before receipt."""
        self.elapsed_us -= latency_us
        value = self.read_us(name) * 10
        self.elapsed_us += latency_us
        return value

    def sample(self, counter: int) -> QpcSample:
        return QpcSample(counter * 1_000_000 // WINE_FREQUENCY,
                         {name: self.read_us(name) for name in ("boottime", "raw", "monotonic")})


def wsl_host() -> SyntheticHost:
    """Booted a day ago: boottime and monotonic agree, raw is far ahead and faster."""
    return SyntheticHost(rates={"raw": WSL_RAW_RATE}, offsets={"raw": BOOT_US + 7_862_400_000})


class ReportTests(unittest.TestCase):
    def test_report_round_trip_with_details(self):
        payload = encode_qpc_report(123_456_789, WINE_FREQUENCY, {"wine": "11.0", "proton": "GE-Proton11-9"})
        report = decode_qpc_report(payload)
        self.assertEqual((report.counter, report.frequency, report.micros),
                         (123_456_789, WINE_FREQUENCY, 12_345_678))
        self.assertEqual(report.details, {"wine": "11.0", "proton": "GE-Proton11-9"})

    def test_periodic_report_is_sixteen_bytes(self):
        payload = struct.pack("<QQ", 20, 10)
        self.assertEqual(decode_qpc_report(payload).details, {})
        self.assertEqual(encode_qpc_report(20, 10), payload)

    def test_malformed_reports_are_rejected(self):
        for payload in (b"\0" * 15, struct.pack("<QQ", 5, 0)):
            with self.subTest(payload=payload), self.assertRaises(ValueError):
                decode_qpc_report(payload)


class ClassifierTests(unittest.TestCase):
    def classify(self, host, source, reports=1, current="boottime", step_s=2.0, **options):
        classifier = QpcClockClassifier(**options)
        for _ in range(reports):
            classifier.add(host.sample(host.counter(source)))
            host.elapsed_us += round(step_s * 1e6)
        return classifier.verdict(current)

    def test_upstream_wine_follows_boottime(self):
        verdict = self.classify(wsl_host(), "boottime")
        self.assertEqual((verdict.clock, verdict.basis), ("boottime", ABSOLUTE))
        self.assertEqual(verdict.delays_us["boottime"], 300)

    def test_proton_follows_raw_from_the_first_report(self):
        verdict = self.classify(wsl_host(), "raw")
        self.assertEqual((verdict.clock, verdict.basis), ("raw", ABSOLUTE))

    def test_equal_clocks_keep_the_current_one(self):
        host = SyntheticHost()
        self.assertEqual(self.classify(host, "raw", current="boottime").clock, "boottime")
        self.assertEqual(self.classify(host, "raw", current="raw").clock, "raw")

    def test_nearest_match_wins_without_a_current_clock(self):
        host = SyntheticHost(offsets={"raw": BOOT_US + 4_000})
        self.assertEqual(self.classify(host, "raw", current=None).clock, "raw")

    def test_latency_spike_does_not_unseat_the_true_clock(self):
        host = SyntheticHost(offsets={"raw": BOOT_US - 15_000})
        classifier = QpcClockClassifier()
        for latency in (300, 400, 60_000):
            classifier.add(host.sample(host.counter("boottime", latency)))
            host.elapsed_us += 2_000_000
        verdict = classifier.verdict("boottime")
        self.assertEqual((verdict.clock, verdict.basis), ("boottime", ABSOLUTE))

    def test_unanchored_counter_waits_for_the_rate_span(self):
        host = wsl_host()
        unanchored = host.counter("raw") + 123_456_789_000
        classifier = QpcClockClassifier()
        classifier.add(host.sample(unanchored))
        self.assertEqual(classifier.verdict("boottime").basis, UNDECIDED)

    def test_unanchored_counter_is_matched_by_rate(self):
        host = wsl_host()
        classifier = QpcClockClassifier()
        for _ in range(5):
            classifier.add(host.sample(host.counter("raw") + 123_456_789_000))
            host.elapsed_us += 2_000_000
        verdict = classifier.verdict("boottime")
        self.assertEqual((verdict.clock, verdict.basis), ("raw", RATE))
        self.assertAlmostEqual(verdict.rates_ppm["raw"], 0, delta=1)
        self.assertLess(verdict.rates_ppm["boottime"], -80_000)

    def test_tsc_counter_drifting_ppm_stays_on_raw(self):
        # GE-Proton's user-mode TSC counter: anchored on raw, off by its calibration error.
        host = SyntheticHost(rates={"raw": WSL_RAW_RATE}, offsets={"raw": BOOT_US - 900_000_000})
        classifier = QpcClockClassifier()
        anchor = host.read_us("raw")
        for _ in range(8):
            raw_us = host.read_us("raw") - 300
            tsc_counter = round((anchor + (raw_us - anchor) * (1 + 5e-6)) * 10)
            classifier.add(host.sample(tsc_counter))
            host.elapsed_us += 4_000_000_000
        self.assertGreater(abs(classifier.recent_delays_us()["raw"]), 100_000)
        for current in ("raw", "boottime"):
            verdict = classifier.verdict(current)
            self.assertEqual((verdict.clock, verdict.basis), ("raw", RATE))

    def test_suspend_moves_the_match_to_raw(self):
        host = SyntheticHost()
        classifier = QpcClockClassifier()
        for _ in range(3):
            classifier.add(host.sample(host.counter("raw")))
            host.elapsed_us += 2_000_000
        self.assertEqual(classifier.verdict("boottime").clock, "boottime")
        host.suspend(600)
        for _ in range(2):
            classifier.add(host.sample(host.counter("raw")))
            host.elapsed_us += 2_000_000
        self.assertEqual(classifier.verdict("boottime").clock, "boottime")
        for _ in range(3):
            classifier.add(host.sample(host.counter("raw")))
            host.elapsed_us += 2_000_000
        verdict = classifier.verdict("boottime")
        self.assertEqual((verdict.clock, verdict.basis), ("raw", ABSOLUTE))

    def test_counter_going_back_starts_over(self):
        host = wsl_host()
        classifier = QpcClockClassifier()
        classifier.add(host.sample(host.counter("raw")))
        classifier.add(host.sample(host.counter("raw") - 50_000_000))
        self.assertEqual(len(classifier.samples), 1)


class ServiceClockTests(unittest.TestCase):
    def test_switch_to_the_new_clock_value(self):
        host = wsl_host()
        clock = ServiceClock("boottime", read_us=host.read_us)
        before = clock()
        switch = clock.switch("raw")
        self.assertEqual((clock.source, clock.generation, clock.offset_us), ("raw", 1, 0))
        self.assertEqual(clock(), host.read_us("raw"))
        self.assertEqual(switch.delta_us, host.read_us("raw") - before)

    def test_switch_keeping_the_value_is_continuous(self):
        host = wsl_host()
        clock = ServiceClock("boottime", read_us=host.read_us)
        before = clock()
        switch = clock.switch("raw", keep_value=True)
        self.assertEqual((clock(), switch.delta_us), (before, 0))
        host.elapsed_us += 1_000_000
        self.assertEqual(clock() - before, round(WSL_RAW_RATE * 1_000_000))

    def test_wine_clock_names(self):
        self.assertEqual(wine_clock("raw").source, "raw")
        self.assertFalse(wine_clock("boottime").automatic)
        automatic = wine_clock(AUTO)
        self.assertEqual((automatic.source, automatic.automatic), ("boottime", True))
        with self.assertRaises(ValueError):
            ServiceClock("realtime")


class FollowerTests(unittest.TestCase):
    def test_fixed_clock_never_switches(self):
        host = wsl_host()
        follower = QpcClockFollower(ServiceClock("boottime", read_us=host.read_us))
        decision = follower.observe("bridge", host.sample(host.counter("raw")))
        self.assertEqual((decision.verdict.clock, decision.switch), ("raw", None))
        self.assertEqual(follower.clock.source, "boottime")

    def test_automatic_clock_switches_and_switches_back(self):
        host = wsl_host()
        follower = QpcClockFollower(ServiceClock("boottime", automatic=True, read_us=host.read_us))
        switch = follower.observe("bridge", host.sample(host.counter("raw"))).switch
        self.assertEqual((switch.old, switch.new), ("boottime", "raw"))
        follower.forget("bridge")
        host.elapsed_us += 10_000_000
        switch = follower.observe("next", host.sample(host.counter("boottime"))).switch
        self.assertEqual((switch.old, switch.new), ("raw", "boottime"))

    def test_disagreeing_bridges_hold_the_current_clock(self):
        host = wsl_host()
        follower = QpcClockFollower(ServiceClock("boottime", automatic=True, read_us=host.read_us))
        follower.observe("wine", host.sample(host.counter("boottime")))
        decision = follower.observe("proton", host.sample(host.counter("raw")))
        self.assertEqual((decision.switch, decision.held_by), (None, "wine"))
        follower.forget("wine")
        host.elapsed_us += 2_000_000
        self.assertEqual(follower.observe("proton", host.sample(host.counter("raw"))).switch.new, "raw")


class ClockMapperTests(unittest.TestCase):
    def test_mapping_starts_over_when_the_clock_switches(self):
        host = wsl_host()
        clock = ServiceClock("boottime", read_us=host.read_us)
        mapper = driver.ClockMapper(clock)
        mapper.observe(1_000)
        self.assertEqual(mapper(1_000), host.read_us("boottime"))
        clock.switch("raw")
        self.assertEqual(mapper(1_000), host.read_us("raw"))
        host.elapsed_us += 33_000
        mapper.observe(34_000)
        self.assertEqual(mapper(34_000), host.read_us("raw"))
        self.assertEqual(len(mapper.offsets), 1)


class BridgeReportTests(unittest.IsolatedAsyncioTestCase):
    """Scripted bridges send QueryPerformanceCounter frames on the control connection."""

    async def start(self, clock_name=AUTO, host=None):
        self.host = host or wsl_host()
        clock = wine_clock(clock_name)
        self.clock = ServiceClock(clock.source, clock.automatic, read_us=self.host.read_us)
        self.service = PrpService(HeadPoseHub(), "ET5-TEST", clock=self.clock)
        self.service.attach_tracker()
        self.server = await self.service.serve("127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        self.writers = []

    async def asyncTearDown(self):
        for writer in self.writers:
            writer.close()
        await self.service.close(self.server)

    async def bridge(self):
        reader, writer = await asyncio.open_connection("127.0.0.1", self.port)
        self.writers.append(writer)
        writer.write(encode_frame(FRAME_BRIDGE_HELLO, struct.pack("<I", 1)))
        kind, _ = await read_frame(reader)
        self.assertEqual(kind, FRAME_PIPES)
        return writer

    async def report(self, writer, source, details=None, counter_offset=0):
        before = len(self.reports)
        writer.write(encode_frame(FRAME_QPC, encode_qpc_report(
            self.host.counter(source) + counter_offset, WINE_FREQUENCY, details)))
        await writer.drain()
        for _ in range(200):
            if len(self.reports) > before:
                return
            await asyncio.sleep(0.005)
        self.fail("report not handled")

    def setUp(self):
        self.reports = []

    def watch_reports(self):
        original = self.service.observe_bridge_clock

        def observed(*arguments):
            original(*arguments)
            self.reports.append(arguments)
        self.service.observe_bridge_clock = observed

    async def test_auto_follows_raw_for_proton_at_the_first_report(self):
        await self.start()
        self.watch_reports()
        writer = await self.bridge()
        with self.assertLogs("tobii_linux.prp_service", logging.INFO) as logs:
            await self.report(writer, "raw", {"wine": "11.0", "proton": "GE-Proton11-9"})
        self.assertEqual(self.clock.source, "raw")
        self.assertEqual(self.clock(), self.host.read_us("raw"))
        text = "\n".join(logs.output)
        self.assertIn("proton GE-Proton11-9", text)
        self.assertIn("follows raw", text)

    async def test_auto_stays_on_boottime_for_upstream_wine(self):
        await self.start()
        self.watch_reports()
        writer = await self.bridge()
        for _ in range(3):
            await self.report(writer, "boottime")
            self.host.elapsed_us += 2_000_000
        self.assertEqual((self.clock.source, self.clock.generation), ("boottime", 0))

    async def test_timesync_and_samples_use_the_chosen_clock(self):
        await self.start()
        self.watch_reports()
        writer = await self.bridge()
        await self.report(writer, "raw")
        sample = HeadPoseSample(self.service.clock(), True, (0.0, 0.0, 600.0), (0.0, 0.0, 0.0))
        self.assertEqual(sample.timestamp_us, self.host.read_us("raw"))
        from tobii_linux.prp_session import COMMAND_TIMESYNC

        class Request:
            tag, command = 7, COMMAND_TIMESYNC
        response = self.service.respond(None, Request())
        self.assertEqual(response.value, self.host.read_us("raw"))

    async def test_switch_mid_session_closes_clients_and_moves_presence(self):
        await self.start(host=SyntheticHost())
        self.watch_reports()
        self.service.publish_presence(self.clock(), 1)
        since = self.service.presence.since_us
        closed = []

        class Client:
            def close(self):
                closed.append(self)
        self.service.connections.add(Client())
        writer = await self.bridge()
        await self.report(writer, "raw")
        self.assertEqual(self.clock.source, "boottime")
        self.host.elapsed_us += 2_000_000
        self.host.suspend(3600)
        with self.assertLogs("tobii_linux.prp_service", logging.INFO) as logs:
            for _ in range(4):
                self.host.elapsed_us += 2_000_000
                await self.report(writer, "raw")
        self.assertEqual(self.clock.source, "raw")
        self.assertEqual(len(closed), 1)
        self.assertEqual(self.service.presence.since_us, since - 3_600_000_000)
        self.assertIn("timesync on the new clock", "\n".join(logs.output))

    async def test_fixed_clock_warns_about_a_mismatch(self):
        await self.start("boottime")
        self.watch_reports()
        writer = await self.bridge()
        with self.assertLogs("tobii_linux.prp_service", logging.WARNING) as logs:
            await self.report(writer, "raw")
        self.assertEqual(self.clock.source, "boottime")
        self.assertIn("fixed to boottime", logs.output[0])

    async def test_unanchored_counter_switches_by_rate_without_a_jump(self):
        await self.start()
        self.watch_reports()
        writer = await self.bridge()
        for _ in range(5):
            before = self.clock()
            await self.report(writer, "raw", counter_offset=987_654_321_000)
            self.assertEqual(self.clock(), before)
            self.host.elapsed_us += 2_000_000
        self.assertEqual(self.clock.source, "raw")
        self.assertNotEqual(self.clock.offset_us, 0)

    async def test_old_bridge_keeps_the_default_clock(self):
        from tobii_linux import prp_service
        await self.start()
        original_wait = prp_service.QPC_REPORT_WAIT_S
        prp_service.QPC_REPORT_WAIT_S = 0.05
        try:
            with self.assertLogs("tobii_linux.prp_service", logging.INFO) as logs:
                await self.bridge()
                await asyncio.sleep(0.2)
        finally:
            prp_service.QPC_REPORT_WAIT_S = original_wait
        self.assertEqual(self.clock.source, "boottime")
        self.assertIn("does not report its QueryPerformanceCounter", "\n".join(logs.output))

    async def test_malformed_report_is_ignored(self):
        await self.start()
        self.watch_reports()
        writer = await self.bridge()
        with self.assertLogs("tobii_linux.prp_service", logging.WARNING):
            writer.write(encode_frame(FRAME_QPC, b"\1\2"))
            await writer.drain()
            for _ in range(100):
                if self.reports:
                    break
                await asyncio.sleep(0.005)
        await self.report(writer, "raw")
        self.assertEqual(self.clock.source, "raw")


if __name__ == "__main__":
    unittest.main()
