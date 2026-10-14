import itertools
import json
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
    payload = gaze.columns_payload({1: gaze.int64_column(timestamp), 2: gaze.uint32_column(value)})
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
        with GazeAndImageStreams(driver.TrackerSession(device), clock, published.append,
                                 lambda t, v: presences.append((t, v)), silence_timeout_s=0.2) as live:
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
        self.assertEqual(presences[0][1], 2)
        self.assertGreater(mapped, first.timestamp)
        self.assertLessEqual(len([first, *remaining]), 2)
        commands = [m.command for m in device.log]
        self.assertEqual(commands, [1220, 1220, 1220, 1230, 1230, 1230])

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


if __name__ == '__main__':
    unittest.main()
