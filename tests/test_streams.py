import itertools
import json
import time
from pathlib import Path
import sys
import unittest
from unittest import mock

from tobii_linux import driver, gaze, streams
from tobii_linux import ttp_commands as tc
from tobii_linux.frames import ImageFrame
from tobii_linux.head_pose_pipeline import HeadPoseSample
from tobii_linux.streams import GazeAndImageStreams, Presence, StreamReader
from tobii_linux.ttp import Decoder, Message

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_frames import image_columns, stream_message  # noqa: E402

# Gaze payloads recorded from a live Eye Tracker 5 by tools/linux_stream_capture.py.
PAYLOADS = {name: bytes.fromhex(payload) for name, payload in json.loads(
    (Path(__file__).resolve().parent/'data/gaze-payloads.json').read_text())['payloads'].items()}


def gaze_message(payload):
    return Message(tc.STREAM_DATA_KIND, 0, 0, tc.wire_stream_id(tc.Stream.GAZE), 0, payload)


def image_message():
    return stream_message(tc.Stream.IMAGE, image_columns())


def presence_message(timestamp, value):
    payload = gaze.columns_payload({1: gaze.int64_column(timestamp), 2: gaze.int32_column(value)})
    return Message(tc.STREAM_DATA_KIND, 0, 0, tc.wire_stream_id(tc.Stream.PRESENCE), 0, payload)


class ScriptedTracker:
    """Answers every request and, after `after` subscriptions, sends a scripted mix of stream messages."""
    def __init__(self, script, after=2, refuse=()):
        self.requests = Decoder()
        self.outgoing = []
        self.log = []
        self.script = script
        self.after = after
        self.refuse = {tc.subscribe_stream(0, s).payload for s in refuse}
        self.subscriptions = 0

    def write(self, data):
        for request in self.requests.feed(data):
            self.log.append(request)
            refused = request.command == tc.Command.SUBSCRIBE_STREAM and request.payload in self.refuse
            status = 3 if refused else 0
            self.outgoing.append(Message(tc.RESPONSE_KIND, request.request_id, 0, request.command, status,
                                         b'').encode())
            if request.command == tc.Command.SUBSCRIBE_STREAM:
                self.subscriptions += 1
                if self.subscriptions == self.after:
                    self.outgoing.extend(message.encode() for message in self.script)

    def read(self, timeout_ms):
        return self.outgoing.pop(0) if self.outgoing else b''


def take(reader, items, count):
    collected = []
    for item in items:
        collected.append(item)
        if len(collected) == count:
            reader.stop()
    return collected


class RecordedGazeTests(unittest.TestCase):
    """The recorded payloads through gaze.py, the decoder every stream consumer uses."""
    def test_validity_patterns(self):
        frames = {name: gaze.gaze_frame(gaze.decode_gaze_message(payload)) for name, payload in PAYLOADS.items()}
        both, left, right, none = (frames[name] for name in ('both', 'left', 'right', 'none'))
        self.assertEqual(both.timestamp_us, 898743954739)
        self.assertEqual((both.gaze_point.valid, both.gaze_data.left.eye_valid, both.gaze_data.right.eye_valid),
                         (1, 1, 1))
        self.assertAlmostEqual(both.gaze_point.point[0], 0.471703, places=5)
        self.assertAlmostEqual(both.gaze_data.left.pupil_diameter, 3.121475, places=5)
        self.assertEqual((left.gaze_data.left.eye_valid, left.gaze_data.right.eye_valid), (1, 0))
        self.assertEqual((right.gaze_data.left.eye_valid, right.gaze_data.right.eye_valid), (0, 1))
        self.assertEqual((none.gaze_point.valid, none.gaze_point.point), (0, (-1.0, -1.0)))


class StreamReaderTests(unittest.TestCase):
    def test_gaze_and_images_are_demultiplexed_in_arrival_order(self):
        presence = Message(tc.STREAM_DATA_KIND, 0, 0, tc.wire_stream_id(tc.Stream.PRESENCE), 0, b'\0\0')
        broken = gaze_message(PAYLOADS['both'][:-9])
        mistyped = gaze.gaze_stream_message({1: gaze.int64_column(5), 7: gaze.int32_column(0)})
        script = [gaze_message(PAYLOADS['both']), image_message(), presence, broken, mistyped,
                  gaze_message(PAYLOADS['none']), image_message()]
        device = ScriptedTracker(script)
        reader = StreamReader(driver.TrackerSession(device))
        with reader:
            items = take(reader, reader.items(silence_timeout_s=1), 4)
        self.assertEqual([item.stream for item in items],
                         [tc.Stream.GAZE, tc.Stream.IMAGE, tc.Stream.GAZE, tc.Stream.IMAGE])
        self.assertIsInstance(items[0].value, gaze.GazeFrame)
        self.assertIsInstance(items[1].value, ImageFrame)
        self.assertEqual(items[2].value.gaze_point.valid, 0)
        self.assertEqual(reader.received[tc.Stream.GAZE], 4)
        self.assertEqual(reader.received[tc.Stream.IMAGE], 2)
        self.assertEqual(reader.decode_errors[tc.Stream.GAZE], 1)
        self.assertEqual(reader.dropped[tc.Stream.GAZE], 1)
        self.assertIn('gaze message', reader.last_error)
        self.assertEqual(reader.unexpected, 1)
        commands = [(m.command, m.payload) for m in device.log]
        self.assertEqual([c for c, _ in commands], [1220, 1220, 1230, 1230])
        self.assertEqual(commands[0][1], tc.subscribe_stream(0, tc.Stream.GAZE).payload)
        self.assertEqual(commands[1][1], tc.subscribe_stream(0, tc.Stream.IMAGE).payload)
        self.assertEqual({p for _, p in commands[2:]},
                         {tc.unsubscribe_stream(0, s).payload for s in (tc.Stream.GAZE, tc.Stream.IMAGE)})

    def test_presence_is_decoded_and_optional(self):
        device = ScriptedTracker([presence_message(77, 2)], after=3)
        reader = StreamReader(driver.TrackerSession(device), (tc.Stream.GAZE, tc.Stream.IMAGE, tc.Stream.PRESENCE),
                              optional=(tc.Stream.PRESENCE,))
        with reader:
            items = take(reader, reader.items(silence_timeout_s=1), 1)
        self.assertEqual(items[0].value, Presence(77, 2))
        refusing = ScriptedTracker([], refuse=(tc.Stream.PRESENCE,))
        reader = StreamReader(driver.TrackerSession(refusing), (tc.Stream.GAZE, tc.Stream.IMAGE, tc.Stream.PRESENCE),
                              optional=(tc.Stream.PRESENCE,))
        reader.subscribe()
        self.assertEqual(reader.subscribed, [tc.Stream.GAZE, tc.Stream.IMAGE])
        self.assertIn(tc.Stream.PRESENCE, reader.refused)

    def test_required_streams_must_subscribe(self):
        reader = StreamReader(driver.TrackerSession(ScriptedTracker([], refuse=(tc.Stream.IMAGE,))))
        with self.assertRaises(driver.DeviceError):
            reader.subscribe()

    def test_messages_received_during_a_request_are_kept(self):
        session = driver.TrackerSession(ScriptedTracker([]))
        session.pending_streams = [gaze_message(PAYLOADS['left'])]
        reader = StreamReader(session)
        items = take(reader, reader.items(silence_timeout_s=1), 1)
        self.assertEqual(items[0].value.gaze_data.left.eye_valid, 1)

    def test_silence_raises(self):
        reader = StreamReader(driver.TrackerSession(ScriptedTracker([])))
        with self.assertRaises(driver.DeviceError):
            list(reader.items(silence_timeout_s=0.05))

    def test_unsubscribe_reaches_every_stream_after_a_failure(self):
        class FailingFirst(ScriptedTracker):
            def write(self, data):
                for request in self.requests.feed(data):
                    self.log.append(request)
                    status = 3 if request.command == tc.Command.UNSUBSCRIBE_STREAM and len(self.log) == 3 else 0
                    self.outgoing.append(Message(tc.RESPONSE_KIND, request.request_id, 0, request.command,
                                                 status, b'').encode())

        device = FailingFirst([])
        reader = StreamReader(driver.TrackerSession(device))
        reader.subscribe()
        with self.assertRaises(driver.DeviceError):
            reader.unsubscribe()
        self.assertEqual([m.command for m in device.log], [1220, 1220, 1230, 1230])
        self.assertEqual(reader.subscribed, [])


class HostClock:
    """Host time that starts 5 s after device time and advances 1 ms per reading."""
    def __init__(self):
        self.ticks = itertools.count()

    def __call__(self):
        return 898743954739 + 5_000_000 + next(self.ticks) * 1000


class GazeAndImageStreamsTests(unittest.TestCase):
    def test_every_gaze_frame_is_published_on_the_host_clock(self):
        script = [gaze_message(PAYLOADS['both']), image_message(), gaze_message(PAYLOADS['left']),
                  presence_message(898743954739, 2), gaze_message(PAYLOADS['none']), image_message()]
        device = ScriptedTracker(script, after=3)
        published, presences = [], []
        clock = HostClock()
        any_order = streams.DeviceClockWatch(rewind_us=1 << 62)
        with GazeAndImageStreams(driver.TrackerSession(device), clock, published.append,
                                 lambda t, v: presences.append((t, v)), silence_timeout_s=0.2,
                                 device_clock=any_order) as live:
            frames = live.newest_frames()
            first = next(frames)
            remaining = []
            try:
                remaining = list(frames)
            except driver.DeviceError:
                pass
            mapped = live.image_time(first.timestamp)
        self.assertEqual(len(published), 3)
        self.assertTrue(all(isinstance(frame, gaze.GazeFrame) for frame in published))
        self.assertEqual([f.gaze_point.valid for f in published], [1, 1, 0])
        device_times = [898743954739, 898734316869, 898734226487]
        for frame, device_us in zip(published, device_times):
            self.assertGreater(frame.timestamp_us, device_us + 4_000_000)
            self.assertEqual(frame.gaze_point.timestamp, frame.timestamp_us)
        self.assertEqual([value for _, value in presences], [gaze.PLATMOD_UNKNOWN, gaze.PLATMOD_PRESENT])
        self.assertGreater(mapped, first.timestamp)
        self.assertLessEqual(len([first, *remaining]), 2)
        commands = [m.command for m in device.log]
        self.assertEqual(commands, [1220, 1220, 1220], 'a silent session is not asked to unsubscribe')

    def test_stopping_unsubscribes_every_stream(self):
        device = ScriptedTracker([image_message()] * 3, after=3)
        with GazeAndImageStreams(driver.TrackerSession(device), HostClock(), lambda frame: None,
                                 lambda t, v: None) as live:
            next(live.newest_frames())
            live.reader.stop()
        self.assertEqual([m.command for m in device.log], [1220, 1220, 1220, 1230, 1230, 1230])

    def test_device_clock_going_back_ends_the_session(self):
        late, early = gaze.int64_column(900_000_000_000), gaze.int64_column(800_000_000_000)
        script = [gaze.gaze_stream_message({1: late}), gaze.gaze_stream_message({1: early})]
        device = ScriptedTracker(script)
        session = driver.TrackerSession(device)
        with self.assertRaisesRegex(driver.DeviceError, 'restarted'):
            with GazeAndImageStreams(session, HostClock(), lambda frame: None) as live:
                list(live.newest_frames())
        self.assertTrue(session.dead)
        self.assertEqual([m.command for m in device.log], [1220, 1220])

    def test_head_pose_and_gaze_yields_samples_on_the_host_clock(self):
        def processor(session, model_directory, display_area, camera=0):
            return lambda frame: HeadPoseSample(frame.timestamp, True, (1.0, 2.0, 3.0), (0.0, 0.0, 0.0))

        device = ScriptedTracker([image_message(), gaze_message(PAYLOADS['both'])])
        published = []
        with mock.patch.object(streams, 'head_pose_processor', processor):
            samples = streams.head_pose_and_gaze(driver.TrackerSession(device), None, None, HostClock(),
                                                 published.append)
            sample = next(samples)
            samples.close()
        self.assertGreater(sample.timestamp_us, 123456789)
        self.assertEqual([m.command for m in device.log][-2:], [1230, 1230])


class StallingTracker(ScriptedTracker):
    """Streams one message per read while subscribed; goes silent for good after an unread gap.

Models the live finding that the tracker stops sending stream data and replies
for the rest of a session when its endpoint is not read for about a second.
"""
    def __init__(self, stall_s=0.3, after=2):
        super().__init__([], after=after)
        self.stall_s = stall_s
        self.last_read = None
        self.stalled = False
        self.sent = 0

    def write(self, data):
        if not self.stalled:
            super().write(data)

    def read(self, timeout_ms):
        now = time.monotonic()
        streaming = self.subscriptions >= self.after
        if streaming and self.last_read is not None and now - self.last_read > self.stall_s:
            self.stalled = True
        self.last_read = now
        if self.stalled:
            time.sleep(timeout_ms / 1000)
            return b''
        if self.outgoing:
            return self.outgoing.pop(0)
        if streaming:
            self.sent += 1
            message = image_message() if self.sent % 2 else gaze_message(PAYLOADS['both'])
            time.sleep(0.005)
            return message.encode()
        return b''


class SlowConsumerTests(unittest.TestCase):
    """The endpoint keeps being read while the consumer is slower than the stall limit."""
    def test_live_streams_keep_reading_under_slow_processing(self):
        device = StallingTracker()
        gaze_frames = []
        any_order = streams.DeviceClockWatch(rewind_us=1 << 62)
        with GazeAndImageStreams(driver.TrackerSession(device), HostClock(), gaze_frames.append,
                                 device_clock=any_order) as live:
            frames = live.newest_frames()
            for _ in range(2):
                next(frames)
                time.sleep(0.5)
            next(frames)
            live.reader.stop()
        self.assertFalse(device.stalled)
        self.assertGreater(len(gaze_frames), 20)

    def test_head_pose_samples_keep_reading_under_slow_processing(self):
        def processor(session, model_directory, display_area, camera=0):
            return lambda frame: (time.sleep(0.5),
                                  HeadPoseSample(frame.timestamp, True, (0.0,) * 3, (0.0,) * 3))[1]

        device = StallingTracker(after=1)
        with mock.patch.object(driver, 'head_pose_processor', processor):
            samples = driver.head_pose_samples(driver.TrackerSession(device), None, None)
            for _ in range(3):
                next(samples)
        self.assertFalse(device.stalled)

    def test_drained_items_survive_a_slow_consumer(self):
        device = StallingTracker()
        reader = StreamReader(driver.TrackerSession(device))
        with reader:
            items = streams.drained(reader.items(silence_timeout_s=1))
            for _ in range(3):
                next(items)
                time.sleep(0.5)
            reader.stop()
            remaining = list(items)
        self.assertFalse(device.stalled)
        self.assertGreater(len(remaining), 20)

    def test_a_stalled_session_is_dead_and_not_asked_again(self):
        device = StallingTracker()
        session = driver.TrackerSession(device)
        reader = StreamReader(session)
        with self.assertRaises(driver.DeviceError):
            with reader:
                items = reader.items(silence_timeout_s=0.3)
                next(items)
                time.sleep(0.5)
                list(items)
        self.assertTrue(device.stalled)
        self.assertTrue(session.dead)
        self.assertEqual([m.command for m in device.log], [1220, 1220])


class SuspendTests(unittest.TestCase):
    def test_suspend_watch_notices_boottime_running_ahead(self):
        gaps = iter([10.0, 10.1, 10.2, 75.0, 75.1])
        watch = streams.SuspendWatch(lambda: next(gaps))
        self.assertEqual([watch.resumed() for _ in range(4)], [False, False, True, False])

    def test_resume_ends_the_session(self):
        gaps = iter([0.0] + [0.0] * 3 + [60.0] * 100)
        device = ScriptedTracker([image_message()] * 50)
        session = driver.TrackerSession(device)
        reader = StreamReader(session, suspend_watch=streams.SuspendWatch(lambda: next(gaps)))
        with self.assertRaisesRegex(driver.DeviceError, 'suspended'):
            with reader:
                list(reader.items(silence_timeout_s=1))
        self.assertTrue(session.dead)


if __name__ == '__main__':
    unittest.main()
