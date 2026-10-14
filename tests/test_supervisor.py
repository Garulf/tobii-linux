"""Session supervision: backoff, lost sessions, and the service across an unplugged tracker.

The devices are scripted from the recovered request and reply layouts (see
test_driver.FakeDevice), NOT device traffic.
"""
import asyncio
from contextlib import contextmanager
from pathlib import Path
import sys
import threading
import time
import unittest

from tobii_linux import cli, driver, gaze
from tobii_linux import ttp_commands as tc
from tobii_linux.head_pose_pipeline import HeadPoseSample
from tobii_linux.prp import HEAD_POSE, decode_envelope
from tobii_linux.prp_service import HeadPoseHub, PrpService
from tobii_linux.prp_session import (COMMAND_STREAM_START, decode_command_response,
                                     decode_property_subscribe_response,
                                     encode_property_subscribe_request, encode_stream_request)
from tobii_linux.supervisor import Backoff, TrackerLink

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_driver import FakeDevice  # noqa: E402
from test_frames import column, image_columns, int64_field, stream_message  # noqa: E402
from test_prp_service import ServiceTests  # noqa: E402


class BackoffTests(unittest.TestCase):
    def test_delays_double_up_to_the_limit_and_reset_after_a_healthy_session(self):
        backoff = Backoff(first_s=0.5, max_s=4.0, healthy_s=10.0)
        self.assertEqual([backoff.delay(0.1) for _ in range(5)], [0.5, 1.0, 2.0, 4.0, 4.0])
        self.assertEqual(backoff.delay(30.0), 0.5)
        self.assertEqual(backoff.delay(0.1), 1.0)


class TrackerLinkTests(unittest.TestCase):
    def test_lost_and_missing_sessions_are_retried_until_stopped(self):
        states, attempts = [], []
        link = None

        @contextmanager
        def open_session():
            attempts.append(len(attempts))
            if len(attempts) == 1:
                raise driver.NoTrackerError('No Eye Tracker 5 is visible to libusb')
            yield len(attempts)

        def run_session(session, stopping):
            if session == 2:
                raise driver.DeviceError('The tracker sent no stream data for 3 s')
            link.stop()

        link = TrackerLink(open_session, run_session, lambda state, detail: states.append(state),
                           Backoff(first_s=0.01, max_s=0.02))
        link.run()
        self.assertEqual(states, ['starting', 'searching', 'starting', 'lost', 'starting', 'stopped'])
        self.assertEqual((link.sessions, link.failures), (2, 1))

    def test_stop_interrupts_the_wait(self):
        @contextmanager
        def open_session():
            raise driver.NoTrackerError('absent')
            yield

        link = TrackerLink(open_session, None, backoff=Backoff(first_s=60.0))
        thread = threading.Thread(target=link.run)
        thread.start()
        time.sleep(0.1)
        started = time.monotonic()
        link.stop()
        thread.join(2.0)
        self.assertFalse(thread.is_alive())
        self.assertLess(time.monotonic() - started, 1.0)


class StreamingDevice(FakeDevice):
    """FakeDevice that streams images and one presence record after subscribing; can be unplugged."""
    def __init__(self):
        super().__init__()
        self.subscribed = set()
        self.unplugged = False
        self.timestamp = 900_000_000_000

    def write(self, data):
        if self.unplugged:
            raise OSError('LIBUSB_ERROR_NO_DEVICE')
        for request in self.requests.feed(data):
            if request.command == tc.Command.SUBSCRIBE_STREAM:
                self.log.append(request)
                self.reply(request)
                self.subscribed.add(request.payload)
                if request.payload == tc.subscribe_stream(0, tc.Stream.PRESENCE).payload:
                    self.outgoing.append(presence_message(self.timestamp, gaze.PLATMOD_PRESENT).encode())
            else:
                super().write(request.encode())

    def read(self, timeout_ms):
        if self.unplugged:
            raise OSError('LIBUSB_ERROR_NO_DEVICE')
        if self.outgoing:
            return self.outgoing.pop(0)
        if self.subscribed:
            time.sleep(0.01)
            self.timestamp += 30_207
            columns = image_columns()
            columns[0] = column(1, int64_field(self.timestamp))
            return stream_message(tc.Stream.IMAGE, columns).encode()
        time.sleep(timeout_ms / 1000)
        return b''


def presence_message(timestamp, value):
    from tobii_linux.ttp import Message
    payload = gaze.columns_payload({1: gaze.int64_column(timestamp), 2: gaze.int32_column(value)})
    return Message(tc.STREAM_DATA_KIND, 0, 0, tc.wire_stream_id(tc.Stream.PRESENCE), 0, payload)


class FakeProcessor:
    def __init__(self):
        self.starts = 0

    def start(self, calibration):
        self.starts += 1

    def __call__(self, frame):
        return HeadPoseSample(frame.timestamp, False, (0.0,) * 3, (0.0,) * 3)


class ReplugTests(unittest.IsolatedAsyncioTestCase):
    """cli.service_link with a tracker that is unplugged and plugged in again."""
    channel = ServiceTests.channel
    session = ServiceTests.session
    closed = ServiceTests.closed

    async def asyncSetUp(self):
        self.hub = HeadPoseHub()
        self.service = PrpService(self.hub, "ET5-TEST", clock=self.clock)
        self.server = await self.service.serve("127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        self.channels = []
        self.device = None
        self.processor = FakeProcessor()
        loop = asyncio.get_running_loop()
        self.link = cli.service_link(self.service, loop, self.processor, self.clock, self.open_session)
        self.link.backoff = Backoff(first_s=0.02, max_s=0.05)
        self.worker = threading.Thread(target=self.link.run)

    @staticmethod
    def clock():
        return time.clock_gettime_ns(time.CLOCK_MONOTONIC) // 1000

    @contextmanager
    def open_session(self):
        device = self.device
        if device is None:
            raise driver.NoTrackerError('No Eye Tracker 5 is visible to libusb')
        session = driver.TrackerSession(device)
        session.initialize()
        yield session

    async def asyncTearDown(self):
        self.link.stop()
        if self.worker.is_alive():
            await asyncio.get_running_loop().run_in_executor(None, self.worker.join, 5.0)
        for channel in self.channels:
            channel.close()
        await self.service.close(self.server)

    async def until(self, condition, timeout_s=3.0):
        deadline = time.monotonic() + timeout_s
        while not condition():
            if time.monotonic() > deadline:
                raise AssertionError('condition not reached')
            await asyncio.sleep(0.02)

    async def streaming_client(self):
        command, subscription, _, tags, _, packet = await self.session()
        self.assertEqual(decode_command_response(packet).status, 0)
        command.send_packet(encode_property_subscribe_request(tags.next_command_tag(), 0xB))
        presence = decode_property_subscribe_response(await command.packet())
        command.send_packet(encode_stream_request(tags.next_command_tag(), COMMAND_STREAM_START,
                                                  HEAD_POSE))
        self.assertEqual(decode_command_response(await command.packet()).status, 0)
        return command, subscription, presence

    async def test_service_survives_unplug_and_replug(self):
        self.worker.start()
        await asyncio.sleep(0.1)
        refused = await self.channel("ET5-TEST")
        refused.send(b"\0" * 8)
        await self.closed(refused)
        self.device = StreamingDevice()
        await self.until(lambda: self.service.tracker_attached)
        command, subscription, presence = await self.streaming_client()
        self.assertEqual(presence.status, 0)
        packages = [decode_envelope(await subscription.packet()) for _ in range(3)]
        self.assertTrue(all(p[8] == 1 for p in packages))

        self.device.unplugged, self.device = True, None
        await self.closed(command)
        await self.closed(subscription)
        await self.until(lambda: not self.service.tracker_attached)
        self.assertEqual(self.hub.queues, set())

        self.device = StreamingDevice()
        await self.until(lambda: self.service.tracker_attached)
        command, subscription, presence = await self.streaming_client()
        self.assertEqual(struct_value(presence.block), gaze.PRESENCE_PRESENT)
        self.assertTrue(await subscription.packet())
        self.assertEqual(self.processor.starts, 2)
        self.assertEqual((self.link.sessions, self.link.failures), (2, 1))

    async def test_stopping_unsubscribes_and_disconnects_clients(self):
        device = self.device = StreamingDevice()
        self.worker.start()
        await self.until(lambda: self.service.tracker_attached)
        command, subscription, _ = await self.streaming_client()
        self.link.stop()
        await asyncio.get_running_loop().run_in_executor(None, self.worker.join, 5.0)
        self.assertFalse(self.worker.is_alive())
        unsubscribed = [m.payload for m in device.log if m.command == tc.Command.UNSUBSCRIBE_STREAM]
        self.assertEqual(sorted(unsubscribed), sorted(tc.unsubscribe_stream(0, s).payload for s in (
            tc.Stream.GAZE, tc.Stream.IMAGE, tc.Stream.PRESENCE)))
        await self.closed(command)
        await self.closed(subscription)

    async def test_presence_starts_unknown_then_follows_the_device(self):
        self.device = StreamingDevice()
        self.worker.start()
        await self.until(lambda: self.service.presence.value == gaze.PRESENCE_PRESENT)
        self.assertEqual(self.service.presence.value, gaze.PRESENCE_PRESENT)


def struct_value(block):
    import struct
    return struct.unpack("<IIII", block)[3]


del ServiceTests

if __name__ == '__main__':
    unittest.main()
