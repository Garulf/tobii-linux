import asyncio
import base64
from dataclasses import dataclass
import http.client
import importlib.util
import json
from pathlib import Path
import struct
import sys
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
import zlib

HAS_NUMPY = importlib.util.find_spec('numpy') is not None

from tobii_linux.demo import events
from tobii_linux.demo.app import build_parser, run
from tobii_linux.demo.pipeline import DemoPipeline, DeviceCombinedGaze, LatestSlot, ProcessedFrame
from tobii_linux.demo.server import DemoServer, EventHub, Subscriber
from tobii_linux.driver import CameraCalibration
from tobii_linux.face_tracking import TRACKING, TrackedFrame
from tobii_linux.frames import ImageFrame
from tobii_linux import gaze
from tobii_linux.head_pose_pipeline import DisplayArea, HeadPoseSample
from tobii_linux.streams import Presence

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_streams import PAYLOADS  # noqa: E402

def recorded(name):
    return gaze.gaze_frame(gaze.decode_gaze_message(PAYLOADS[name]))


AREA = DisplayArea((-296.62, 326.0, 111.82), (300.38, 326.0, 111.82), (-296.62, 10.27, -3.1))


def png_chunks(data):
    assert data[:8] == events.PNG_SIGNATURE
    offset, chunks = 8, []
    while offset < len(data):
        length, kind = struct.unpack_from('>I4s', data, offset)
        body = data[offset + 8:offset + 8 + length]
        crc = struct.unpack_from('>I', data, offset + 8 + length)[0]
        assert crc == zlib.crc32(kind + body)
        chunks.append((kind, body))
        offset += 12 + length
    return chunks


def tracked_frame(accepted=True):
    points = [(0.25, 0.5), (0.75, 0.125)]
    return TrackedFrame(TRACKING, (10, 20, 30, 40), (96, 96), points, points, 0.8125, accepted,
                        {'rigid_state': (0.0,) * 6}, 1.5)


@unittest.skipUnless(HAS_NUMPY, 'Optional NumPy is not installed')
class EventEncodingTests(unittest.TestCase):
    def test_grayscale_png_round_trip(self):
        pixels = bytes(range(12))
        chunks = png_chunks(events.png_grayscale(4, 3, pixels))
        self.assertEqual([kind for kind, _ in chunks], [b'IHDR', b'IDAT', b'IEND'])
        self.assertEqual(struct.unpack('>II5B', chunks[0][1]), (4, 3, 8, 0, 0, 0, 0))
        rows = zlib.decompress(chunks[1][1])
        self.assertEqual(rows, b'\0' + pixels[0:4] + b'\0' + pixels[4:8] + b'\0' + pixels[8:12])

    def test_png_rejects_wrong_size(self):
        with self.assertRaises(ValueError):
            events.png_grayscale(4, 3, b'\0' * 11)

    def test_frame_event_carries_image_and_overlay(self):
        frame = ImageFrame(5_000_000, 8, 4, 3, bytes(range(12)))
        event = events.frame_event(7, frame, tracked_frame(), TRACKING)
        self.assertEqual(event['state'], 'tracking')
        self.assertEqual(event['face'], [10, 20, 30, 40])
        self.assertEqual(event['landmarks'], [0.25, 0.5, 0.75, 0.125])
        self.assertEqual(event['quality'], 0.812)
        self.assertTrue(png_chunks(base64.b64decode(event['png'])))
        bare = events.frame_event(8, frame, None, None)
        self.assertIsNone(bare['state'])
        self.assertIsNone(bare['landmarks'])

    def test_pose_event_in_degrees(self):
        sample = HeadPoseSample(1_000, True, (1.234, -5.0, 620.04), (0.1, -0.2, 0.0))
        event = events.pose_event(sample, tracked_frame(), TRACKING, 9.87)
        self.assertEqual(event['position'], [1.2, -5.0, 620.0])
        self.assertEqual(event['rotation'], [5.73, -11.46, 0.0])
        self.assertEqual(event['ms'], 9.9)
        invalid = events.pose_event(HeadPoseSample(1, False, (0.0,) * 3, (0.0,) * 3), tracked_frame(False),
                                    TRACKING, 1.0)
        self.assertIsNone(invalid['position'])
        self.assertFalse(invalid['accepted'])

    def test_gaze_event_from_a_device_record(self):
        record = recorded('left')
        event = events.gaze_event(record, DeviceCombinedGaze()(record), 'device')
        self.assertEqual(event['point'], [0.6322, 0.0999])
        self.assertTrue(event['left']['valid'])
        self.assertEqual(event['left']['pupil'], 4.52)
        self.assertFalse(event['right']['valid'])
        self.assertIsNone(event['right']['point'])
        text = events.compact_json(event)
        self.assertNotIn(' ', text)
        missing = recorded('none')
        self.assertIsNone(DeviceCombinedGaze()(missing))
        self.assertIsNone(events.gaze_event(missing, None, 'device')['point'])

    def test_gaze_columns_event_lists_every_column(self):
        event = events.gaze_columns_event(recorded('both'))
        self.assertEqual(len(event['columns']), 35)
        self.assertEqual(event['columns'][0], [1, 'timestamp', 898743954739])
        self.assertEqual(event['columns'][-1][:2], [44, 'right_openness_validity'])

    def test_non_finite_values_become_null(self):
        self.assertIsNone(events.rounded([1.0, float('nan')], 2))
        self.assertIsNone(events.rounded_scalar(float('inf'), 2))

    def test_sse_block(self):
        self.assertEqual(events.sse_block('gaze', '{"a":1}'), b'event: gaze\ndata: {"a":1}\n\n')


@dataclass
class FakeTracker:
    state: int = TRACKING


class FakeProcessor:
    def __init__(self, model_directory, display_area):
        self.tracker = FakeTracker()
        self.output = None
        self.calibrations = []

    def calibrate(self, calibration):
        self.calibrations.append(calibration)
        self.output = True

    def topology(self):
        return events.mesh_topology_event(((0, 1), (1, 2)), 3, [(0.0, 0.0, 0.0)] * 4, (0.0, -200.0, 30.0))

    def __call__(self, frame):
        sample = HeadPoseSample(frame.timestamp, True, (1.0, 2.0, 600.0), (0.0, 0.0, 0.0))
        return ProcessedFrame(tracked_frame(), sample, [(10.0, 20.0), (30.5, 40.0), (50.0, 60.0)],
                              [(1.0, 2.0, 600.0)] * 3)


class Collector:
    def __init__(self):
        self.events = []
        self.condition = threading.Condition()

    def __call__(self, topic, payload, retain=False):
        with self.condition:
            self.events.append((topic, payload))
            self.condition.notify_all()

    def wait_for(self, topic, timeout_s=5):
        with self.condition:
            self.condition.wait_for(lambda: any(t == topic for t, _ in self.events), timeout_s)
            return [p for t, p in self.events if t == topic]


@unittest.skipUnless(HAS_NUMPY, 'Optional NumPy is not installed')
class PipelineTests(unittest.TestCase):
    def test_frames_are_processed_and_published_with_their_overlay(self):
        published = Collector()
        pipeline = DemoPipeline(published, AREA, image_rate=1000, processor_factory=FakeProcessor)
        pipeline.on_session(CameraCalibration((0.0, 0.0, 4.14), (0.0, 0.0, 0.0)), {'model': 'test'})
        pipeline.start()
        try:
            pipeline.on_frame(ImageFrame(2_000_000, 8, 4, 3, bytes(12)))
            pose = published.wait_for('pose')[0]
            frame = published.wait_for('frame')[0]
        finally:
            pipeline.stop()
        self.assertEqual(pose['position'], [1.0, 2.0, 600.0])
        self.assertEqual(struct.unpack('<9h', base64.b64decode(pose['mesh3d'])), (10, 20, 6000) * 3)
        self.assertEqual(struct.unpack('<6h', base64.b64decode(frame['mesh'])), (100, 200, 305, 400, 500, 600))
        topology = published.wait_for('mesh_topology')[0]
        self.assertEqual(topology['edges'], [0, 1, 1, 2])
        self.assertEqual(pose['state'], 'tracking')
        self.assertEqual(frame['face'], [10, 20, 30, 40])
        status = pipeline.status()
        self.assertEqual(status['head_pose']['state'], 'running')
        self.assertEqual(status['counters']['processed'], 1)
        self.assertEqual(status['device'], {'model': 'test'})
        self.assertEqual(status['display_area']['width_mm'], 597.0)

    def test_gaze_is_published_through_the_selected_source(self):
        published = Collector()
        pipeline = DemoPipeline(published, AREA, head_pose=False)
        pipeline.on_gaze(recorded('both'))
        pipeline.on_gaze(recorded('none'))
        pipeline.on_presence(Presence(1, 2))
        gaze = published.wait_for('gaze')
        self.assertEqual(gaze[0]['point'], [0.4717, 1.1246])
        self.assertIsNone(gaze[1]['point'])
        self.assertEqual(len(published.wait_for('gaze_columns')), 1)
        self.assertEqual(pipeline.status()['counters']['gaze_valid'], 1)
        self.assertEqual(pipeline.status()['presence'], {'value': 2, 'state': 'present'})

    def test_missing_models_leave_camera_images_running(self):
        def broken(model_directory, display_area):
            raise FileNotFoundError('no models here')
        published = Collector()
        pipeline = DemoPipeline(published, AREA, image_rate=1000, processor_factory=broken)
        with self.assertLogs('tobii_linux.demo', 'WARNING'):
            pipeline.start()
            try:
                pipeline.on_frame(ImageFrame(1, 8, 4, 3, bytes(12)))
                frame = published.wait_for('frame')[0]
            finally:
                pipeline.stop()
        self.assertIsNone(frame['state'])
        status = pipeline.status()
        self.assertEqual(status['head_pose']['state'], 'unavailable')
        self.assertIn('no models here', status['errors'][0]['message'])

    def test_latest_slot_drops_frames_that_were_never_taken(self):
        slot = LatestSlot()
        for item in (1, 2, 3):
            slot.put(item)
        self.assertEqual(slot.take(0.01), 3)
        self.assertEqual(slot.dropped, 2)
        self.assertIsNone(slot.take(0.01))


def fetch(url):
    with urllib.request.urlopen(url, timeout=5) as response:
        return response.status, response.headers.get('Content-Type'), response.read()


def read_events(url, wanted, timeout_s=10):
    """Topics and payloads from the SSE stream until every wanted topic has arrived."""
    host, port = url.split('//')[1].rstrip('/').split(':')
    connection = http.client.HTTPConnection(host, int(port), timeout=timeout_s)
    connection.request('GET', '/events')
    response = connection.getresponse()
    assert response.status == 200 and response.headers['Content-Type'] == 'text/event-stream'
    received, topic, deadline = {}, None, time.monotonic() + timeout_s
    while not wanted <= received.keys() and time.monotonic() < deadline:
        line = response.fp.readline().decode().rstrip('\n')
        if line.startswith('event: '):
            topic = line[len('event: '):]
        elif line.startswith('data: ') and topic is not None:
            received.setdefault(topic, []).append(json.loads(line[len('data: '):]))
    connection.close()
    return received


class ServerTests(unittest.TestCase):
    def test_page_status_and_event_stream(self):
        async def scenario():
            hub = EventHub()
            hub.attach(asyncio.get_running_loop())
            server = await DemoServer(hub, '127.0.0.1', 0).start()
            base = f'http://127.0.0.1:{server.port}/'
            loop = asyncio.get_running_loop()
            try:
                page = await loop.run_in_executor(None, fetch, base)
                script = await loop.run_in_executor(None, fetch, base + 'static/demo.js')
                empty_status = await loop.run_in_executor(None, fetch, base + 'api/status')
                hub.publish('status', {'connection': 'streaming'}, retain=True)
                status = await loop.run_in_executor(None, fetch, base + 'api/status')
                with self.assertRaises(urllib.error.HTTPError) as missing:
                    await loop.run_in_executor(None, fetch, base + 'static/../app.py')
                reading = loop.run_in_executor(None, read_events, base, {'status', 'gaze'})
                for _ in range(100):
                    if hub.subscribers:
                        break
                    await asyncio.sleep(0.01)
                hub.publish_threadsafe('gaze', {'point': [0.5, 0.5]})
                received = await reading
            finally:
                await server.close()
            return page, script, empty_status, status, missing.exception.code, received

        page, script, empty_status, status, missing_code, received = asyncio.run(scenario())
        self.assertEqual(page[0], 200)
        self.assertTrue(page[1].startswith('text/html'))
        self.assertIn(b'/static/demo.js', page[2])
        self.assertTrue(script[1].startswith('text/javascript'))
        self.assertIn(b'EventSource', script[2])
        self.assertEqual(json.loads(empty_status[2]), {'connection': 'starting'})
        self.assertEqual(json.loads(status[2]), {'connection': 'streaming'})
        self.assertEqual(missing_code, 404)
        self.assertEqual(received['status'], [{'connection': 'streaming'}])
        self.assertEqual(received['gaze'], [{'point': [0.5, 0.5]}])

    def test_slow_browsers_get_the_newest_image_and_a_bounded_backlog(self):
        async def scenario():
            subscriber = Subscriber(limit=3)
            for index in range(5):
                subscriber.push('gaze', f'g{index}'.encode())
                subscriber.push('frame', f'f{index}'.encode())
            return subscriber.take(), subscriber.dropped
        data, dropped = asyncio.run(scenario())
        self.assertEqual(data, b'g2g3g4f4')
        self.assertEqual(dropped, 2)


@unittest.skipUnless(HAS_NUMPY, 'Optional NumPy is not installed')
class ReplayTests(unittest.TestCase):
    def test_replay_serves_recorded_frames_and_gaze(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'frames').mkdir()
            log_rows = [{'kind': 'step', 'name': 'calibration', 'ok': True, 'translation': [0, 0, 4.14],
                         'angles': [0, 0, 0]}]
            for index in range(3):
                name = f'frame-{index:04d}-4x3.gray'
                (root/'frames'/name).write_bytes(bytes([index * 40] * 12))
                log_rows.append({'kind': 'frame', 'timestamp': 1_000_000 + index * 30_000, 'width': 4, 'height': 3,
                                 'path': f'/mnt/c/elsewhere/frames/{name}'})
            (root/'frames.jsonl').write_text('\n'.join(json.dumps(row) for row in log_rows) + '\n')
            gaze_rows = [{'kind': 'message', 'command': 0x500, 'stream': 1, 'payload': PAYLOADS[name].hex()}
                         for name in ('both', 'left', 'none')]
            (root/'gaze.jsonl').write_text('\n'.join(json.dumps(row) for row in gaze_rows) + '\n')
            args = build_parser().parse_args([
                '--port', '0', '--no-headpose', '--image-rate', '100',
                '--replay-frames', str(root/'frames.jsonl'), '--replay-gaze', str(root/'gaze.jsonl')])

            async def scenario():
                stop, urls = asyncio.Event(), []
                demo = asyncio.create_task(run(args, ready=urls.append, stop=stop))
                while not urls:
                    await asyncio.sleep(0.01)
                received = await asyncio.get_running_loop().run_in_executor(
                    None, read_events, urls[0], {'frame', 'gaze', 'status'})
                stop.set()
                await asyncio.wait_for(demo, 10)
                return received

            received = asyncio.run(scenario())
        self.assertEqual(received['status'][0]['mode'], 'replay')
        self.assertEqual(received['frame'][0]['width'], 4)
        self.assertIn(received['gaze'][0]['point'], ([0.4717, 1.1246], [0.6322, 0.0999], None))


if __name__ == '__main__':
    unittest.main()
