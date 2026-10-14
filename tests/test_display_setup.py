import argparse
import contextlib
import importlib.util
import io
import json
import math
from pathlib import Path
import tempfile
import unittest
import urllib.error
import urllib.request

from tobii_linux import display_setup as ds
from tobii_linux import ttp_commands as tc
from tobii_linux.driver import TrackerSession
from tobii_linux.gaze_device import GazeCommand
from tobii_linux.ttp import Decoder, Message, fields
from tobii_linux.ttp_fields import value

TOOLS = Path(__file__).resolve().parents[1]/'tools'

# Reply payloads the albedo tracker returned for 1430 and 2110
# (evidence/private/gaze-capture/device-query.jsonl, live).
ALBEDO_1430_PAYLOAD = bytes.fromhex(
    '0000050000000400031f410400000008fffb5d85080000000400000008000518042800000004000000080001bf4666000000'
    '050000000400031f4104000000080004b185080000000400000008000518042800000004000000080001bf46660000000500'
    '00000400031f410400000008fffb5d8508000000040000000800002911bf0000000400000008fffff3999450000005000000'
    '0400010100020000000400003039')
ALBEDO_2110_PAYLOAD = bytes.fromhex(
    '0000020000000400000002030000000400b80000030000000400140000050000000400031f41040000000800000000000000'
    '000400000008ffffff5c28f5c29004000000080000376666666666050000000400031f410400000008000000000000000004'
    '00000008000015851eb851eb040000000800002770a3d70a3d')
ALBEDO_AREA = ds.DisplayArea((-296.6200866699219, 326.0040588378906, 111.8187484741211),
                             (300.3799133300781, 326.0040588378906, 111.8187484741211),
                             (-296.6200866699219, 10.267330169677734, -3.100020170211792))
# Base EDID block of albedo's LG 27GL850 (Windows registry, live).
ALBEDO_EDID = bytes.fromhex(
    '00ffffffffffff001e6d7f5b5fc20300061e0104b53c22789f8cb5af4f43ab260e5054254b007140818081c0a9c0b300d1c0'
    '8100d1cf28de0050a0a038500830080455502100001a000000fd003090e6e63c010a202020202020000000fc003237474c38'
    '35300a2020202020000000ff003030364e544e4837383336370a01fc')
# screenplane.setpm on albedo (live): version 4, kind 1, 36 bytes, then bottom-left, top-left, top-right.
ALBEDO_SCREEN_PLANE = bytes.fromhex(
    '0400000001000000240000005f4f94c3fc462441bb6646c05f4f94c38500a34333a3df42a13096438500a34333a3df42')


class DisplayDevice:
    """Answers 1430 and 2110 with albedo's replies and stores what 1440 sends."""

    def __init__(self):
        self.requests = Decoder()
        self.outgoing = []
        self.log = []
        self.area_payload = ALBEDO_1430_PAYLOAD

    def write(self, data):
        for request in self.requests.feed(data):
            self.log.append(request)
            payload = b''
            if request.command == tc.Command.GET_DISPLAY_AREA:
                payload = self.area_payload
            elif request.command == tc.Command.GET_GEOMETRY_MOUNTING:
                payload = ALBEDO_2110_PAYLOAD
            elif request.command == GazeCommand.SET_DISPLAY_AREA:
                self.area_payload = request.payload
            self.outgoing.append(Message(tc.RESPONSE_KIND, request.request_id, 0, request.command, 0, payload).encode())

    def read(self, timeout_ms):
        return self.outgoing.pop(0) if self.outgoing else b''

    def commands(self):
        return [m.command for m in self.log]


def load_tool():
    spec = importlib.util.spec_from_file_location('display_setup_tool', TOOLS/'display_setup.py')
    tool = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(tool)
    return tool


class ModelTests(unittest.TestCase):
    def test_albedo_area_from_edid_size_and_device_offset_is_exact(self):
        size = ds.edid_size_mm(ALBEDO_EDID)
        self.assertEqual(size, (597.0, 336.0))
        offset = ds.decompose_display_area(ALBEDO_AREA).tracker_offset_mm
        self.assertEqual(ds.compose_display_area(size, ds.ET5_MOUNTING, offset), ALBEDO_AREA)

    def test_rounded_offset_leaves_y_and_z_exact(self):
        area = ds.compose_display_area((597.0, 336.0), ds.ET5_MOUNTING, -1.88)
        for mine, device in zip(area.corners, ALBEDO_AREA.corners):
            self.assertEqual(mine[1:], device[1:])
            self.assertAlmostEqual(mine[0], device[0], delta=1e-4)

    def test_decompose_recovers_size_offset_and_tilt(self):
        setup = ds.decompose_display_area(ALBEDO_AREA)
        self.assertAlmostEqual(setup.size_mm[0], 597.0, places=4)
        self.assertAlmostEqual(setup.size_mm[1], 336.0, places=4)
        self.assertAlmostEqual(setup.tracker_offset_mm, -1.8799, places=4)
        self.assertAlmostEqual(setup.extra_angle_deg, 0.0, places=5)

    def test_compose_then_decompose_round_trips(self):
        for size, offset in (((520.0, 290.0), 12.5), ((336.0, 597.0), -40.0), ((800.0, 340.0), 0.0)):
            setup = ds.decompose_display_area(ds.compose_display_area(size, ds.ET5_MOUNTING, offset))
            self.assertAlmostEqual(setup.size_mm[0], size[0], places=3)
            self.assertAlmostEqual(setup.size_mm[1], size[1], places=3)
            self.assertAlmostEqual(setup.tracker_offset_mm, offset, places=3)

    def test_flat_mounting_puts_the_bottom_edge_on_the_offsets(self):
        mounting = ds.MountingGeometry(0, 184.0, 0.0, (0.0, -10.0, 0.0), (0.0, 0.0, 5.0))
        area = ds.compose_display_area((400.0, 200.0), mounting, 0.0)
        self.assertEqual(area.bottom_left, (-200.0, 10.0, 5.0))
        self.assertEqual(area.top_right, (200.0, 210.0, 5.0))

    def test_tracker_lines_round_trip(self):
        left, right = ds.tracker_lines((597.0, 336.0), -1.88, 184.0)
        size, offset = ds.size_from_tracker_lines(left, right, 184.0, 597.0 / 336.0)
        self.assertAlmostEqual(size[0], 597.0, places=6)
        self.assertAlmostEqual(size[1], 336.0, places=6)
        self.assertAlmostEqual(offset, -1.88, places=6)

    def test_size_from_diagonal(self):
        width, height = ds.size_from_diagonal(27.0)
        self.assertAlmostEqual(math.hypot(width, height), 27.0 * 25.4)
        self.assertAlmostEqual(width / height, 16 / 9)

    def test_edid_resolution_and_plausibility(self):
        self.assertEqual(ds.edid_native_resolution(ALBEDO_EDID), (2560, 1440))
        self.assertTrue(ds.size_hint_is_plausible((597.0, 336.0), (2560, 1440)))
        self.assertFalse(ds.size_hint_is_plausible((600.0, 0.0), (2560, 1440)))
        self.assertFalse(ds.size_hint_is_plausible((160.0, 90.0), (1280, 1024)))
        with self.assertRaises(ValueError):
            ds.edid_size_mm(b'\0' * 128)

    def test_drm_monitors_reads_sysfs_edid(self):
        with tempfile.TemporaryDirectory() as root:
            connector = Path(root)/'card0-DP-1'
            connector.mkdir()
            (connector/'edid').write_bytes(ALBEDO_EDID)
            (connector/'status').write_text('connected\n')
            empty = Path(root)/'card0-HDMI-A-1'
            empty.mkdir()
            (empty/'edid').write_bytes(b'')
            monitors = ds.drm_monitors(root)
        self.assertEqual(monitors, [ds.Monitor('card0-DP-1', 'connected', (597.0, 336.0), (2560, 1440))])


class FileTests(unittest.TestCase):
    def test_screen_plane_setting_matches_the_windows_file(self):
        self.assertEqual(ds.screen_plane_setting(ALBEDO_AREA), ALBEDO_SCREEN_PLANE)
        self.assertEqual(ds.parse_screen_plane_setting(ALBEDO_SCREEN_PLANE), ALBEDO_AREA)

    def test_save_and_load(self):
        with tempfile.TemporaryDirectory() as directory:
            path = ds.save_display_area(Path(directory)/'area.setpm', ALBEDO_AREA)
            self.assertEqual(ds.load_display_area(path), ALBEDO_AREA)
            with self.assertRaises(FileExistsError):
                ds.save_display_area(path, ALBEDO_AREA)
            text = Path(directory)/'area.txt'
            text.write_text(', '.join(repr(v) for v in ALBEDO_AREA.floats))
            self.assertEqual(ds.load_display_area(text), ALBEDO_AREA)

    def test_tool_args_prints_the_display_option_values(self):
        tool = load_tool()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'screenplane.setpm'
            path.write_bytes(ALBEDO_SCREEN_PLANE)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                tool.print_args(argparse.Namespace(file=str(path)))
        self.assertEqual([float(v) for v in output.getvalue().split()], list(ALBEDO_AREA.floats))


class DeviceTests(unittest.TestCase):
    def test_replies_decode(self):
        session = TrackerSession(DisplayDevice())
        self.assertEqual(ds.read_display_area(session), ALBEDO_AREA)
        mounting = ds.read_mounting(session)
        self.assertEqual((mounting.guides, mounting.width_mm, mounting.angle_deg), (2, 184.0, 20.0))
        self.assertEqual(mounting.as_float32(), ds.ET5_MOUNTING.as_float32())

    def test_apply_sends_1440_and_reads_back(self):
        device = DisplayDevice()
        wanted = ds.compose_display_area((520.0, 290.0), ds.ET5_MOUNTING, 3.0)
        self.assertEqual(ds.apply_display_area(TrackerSession(device), wanted), wanted)
        self.assertEqual(device.commands(), [GazeCommand.SET_DISPLAY_AREA, tc.Command.GET_DISPLAY_AREA])
        sent = [value(f) for f in fields(device.log[0].payload)]
        self.assertEqual([v for v in sent if isinstance(v, float)], list(wanted.floats))

    def tool_apply(self, device, **options):
        values = dict(dry_run=False, from_device=False, file=None, edid=None, connector=None, size=None,
                      diagonal=None, aspect='16:9', portrait=False, offset=0.0, lines=None)
        values.update(options)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = load_tool().apply(TrackerSession(device), argparse.Namespace(**values))
        return status, [json.loads(line) for line in output.getvalue().splitlines()]

    def test_tool_apply_from_device_rewrites_the_same_area(self):
        device = DisplayDevice()
        status, records = self.tool_apply(device, from_device=True)
        self.assertEqual(status, 0)
        self.assertTrue(records[0]['identical_to_device'])
        self.assertTrue(records[1]['device_holds_wanted'])
        self.assertEqual(device.log[1].payload, device.area_payload)

    def test_tool_apply_dry_run_sends_no_write(self):
        device = DisplayDevice()
        status, records = self.tool_apply(device, dry_run=True, size=[520.0, 290.0], offset=3.0)
        self.assertEqual(status, 0)
        self.assertNotIn(GazeCommand.SET_DISPLAY_AREA, device.commands())
        self.assertIn(tc.Command.GET_GEOMETRY_MOUNTING, device.commands())
        self.assertEqual([r['name'] for r in records if r['kind'] == 'would_send'], ['SET_DISPLAY_AREA'])
        self.assertFalse(next(r for r in records if r['kind'] == 'apply')['identical_to_device'])


if __name__ == '__main__':
    unittest.main()


class LineSetupTests(unittest.TestCase):
    def test_initial_lines_from_the_stored_area_reproduce_it(self):
        lines = ds.initial_lines(ds.ET5_MOUNTING, ALBEDO_AREA, ds.edid_size_mm(ALBEDO_EDID), (2560, 1440))
        self.assertEqual((lines.size_mm, lines.source), ((597.0, 336.0), 'edid size and stored offset'))
        self.assertAlmostEqual(lines.left, 0.3427472, places=6)
        self.assertAlmostEqual(lines.right, 0.6509549, places=6)
        size, offset, area = ds.area_from_lines(lines.left, lines.right, lines.aspect)
        self.assertEqual(area, ALBEDO_AREA)
        self.assertAlmostEqual(offset, -1.87991, places=5)

    def test_lines_placed_for_597_mm_and_minus_1_88_give_the_device_area(self):
        left, right = ds.tracker_lines((597.0, 336.0), -1.88, 184.0)
        size, offset, area = ds.area_from_lines(left, right, 597.0 / 336.0)
        self.assertAlmostEqual(size[0], 597.0, places=9)
        self.assertAlmostEqual(size[1], 336.0, places=9)
        self.assertTrue(area.almost_equals(ALBEDO_AREA, squared_mm=1e-8))
        for mine, device in zip(area.corners, ALBEDO_AREA.corners):
            self.assertEqual(mine[1:], device[1:])

    def test_without_a_stored_area_the_wizard_estimates(self):
        hinted = ds.initial_lines(ds.ET5_MOUNTING, None, (597.0, 336.0), (2560, 1440))
        self.assertEqual(hinted.source, 'edid size estimate')
        self.assertAlmostEqual(hinted.right - hinted.left, 184.0 / 597.0)
        self.assertAlmostEqual((hinted.left + hinted.right) / 2, 0.5)
        self.assertAlmostEqual(hinted.aspect, 16 / 9)
        rejected = ds.initial_lines(ds.ET5_MOUNTING, None, (597.0, 336.0), (1280, 1024))
        self.assertEqual(rejected.source, 'default width estimate')
        self.assertAlmostEqual(rejected.right - rejected.left, 184.0 / 376.3)
        self.assertAlmostEqual(rejected.aspect, 1.25)

    def test_intro_only_without_a_stored_area(self):
        self.assertTrue(ds.is_mapped(ALBEDO_AREA))
        self.assertFalse(ds.is_mapped(None))
        self.assertFalse(ds.is_mapped(ds.DisplayArea((0, 0, 0), (0, 0, 0), (0, 0, 0))))
        self.assertEqual(ds.line_instructions(ds.ET5_MOUNTING), 'Move the lines to the marks on top of your eye tracker.')
        one_guide = ds.MountingGeometry(1, 184.0, 20.0, (0, 0, 0), (0, 0, 0))
        self.assertIn('sides', ds.line_instructions(one_guide))

    def test_line_limits_follow_the_view_model(self):
        self.assertEqual(ds.clamp_lines(0.0, 1.0), (0.02, 0.98))
        self.assertEqual(ds.clamp_lines(0.5, 0.51), (0.5, 0.55))
        self.assertEqual(ds.clamp_lines(0.99, 0.995), (0.95, 0.98))

    def test_picture_is_wider_than_the_lines_for_two_guides(self):
        self.assertAlmostEqual(ds.picture_proportion(ds.ET5_MOUNTING), 286.0 / 184.0)
        self.assertEqual(ds.picture_proportion(ds.MountingGeometry(1, 184.0, 20.0, (0, 0, 0), (0, 0, 0))), 1.0)


class SetupPageTests(unittest.TestCase):
    def start_page(self, evaluate=None):
        from tobii_linux.display_setup_page import DisplaySetupPage
        configured = []

        def configure(screen_px):
            configured.append(screen_px)
            return {'left': 0.25, 'right': 0.75}
        page = DisplaySetupPage(configure, evaluate or (lambda l, r, v: {'width': 184.0 / (r - l)}), port=0).start()
        self.addCleanup(page.close)
        return page, configured

    @staticmethod
    def post(url, body):
        request = urllib.request.Request(url + 'result', data=json.dumps(body).encode(), method='POST')
        return json.loads(urllib.request.urlopen(request, timeout=5).read())

    def test_page_config_and_result(self):
        page, configured = self.start_page()
        self.assertIn(b'display setup', urllib.request.urlopen(page.url, timeout=5).read())
        config = json.loads(urllib.request.urlopen(page.url + 'setup?sw=2560&sh=1440', timeout=5).read())
        self.assertEqual((config, configured), ({'left': 0.25, 'right': 0.75}, [(2560, 1440)]))
        with self.assertRaises(urllib.error.HTTPError) as error:
            self.post(page.url, {'left': 0.5, 'right': 0.52})
        self.assertEqual(error.exception.code, 400)
        reply = self.post(page.url, {'left': 0.25, 'right': 0.75, 'viewport': {'width': 2560}})
        self.assertEqual(reply, {'width': 368.0})
        result = page.wait_result(1)
        self.assertEqual((result['left'], result['right'], result['reply']), (0.25, 0.75, reply))
        page.finish('saved')
        self.assertEqual(json.loads(urllib.request.urlopen(page.url + 'status', timeout=5).read()),
                         {'message': 'saved', 'final': True})

    def test_cancel(self):
        from tobii_linux.display_setup_page import SetupCancelled
        page, _ = self.start_page()
        self.post(page.url, {'cancelled': True})
        with self.assertRaises(SetupCancelled):
            page.wait_result(1)

    def test_tool_setup_saves_and_sends_nothing_without_apply(self):
        import threading
        import time
        tool = load_tool()
        device = DisplayDevice()
        output = io.StringIO()
        with tempfile.TemporaryDirectory() as directory:
            args = argparse.Namespace(edid=None, connector=None, output=str(Path(directory)/'area'), overwrite=False,
                                      apply=False, host='127.0.0.1', port=0, page_timeout_s=10,
                                      then_calibrate=False, calibration_port=8775)
            outcome = {}
            with contextlib.redirect_stdout(output):
                worker = threading.Thread(target=lambda: outcome.update(status=tool.setup(TrackerSession(device), args)))
                worker.start()
                while '"page"' not in output.getvalue():
                    time.sleep(0.01)
                url = next(json.loads(line)['url'] for line in output.getvalue().splitlines() if '"page"' in line)
                config = json.loads(urllib.request.urlopen(url + 'setup?sw=2560&sh=1440', timeout=5).read())
                self.assertFalse(config['intro'])
                reply = self.post(url, {'left': config['left'], 'right': config['right']})
                worker.join(10)
            self.assertEqual(outcome['status'], 0)
            self.assertTrue(reply['identical_to_device'])
            self.assertEqual(ds.load_display_area(Path(directory)/'area.setpm'), ALBEDO_AREA)
            saved = json.loads((Path(directory)/'area.json').read_text())
            self.assertEqual(saved['display_args'], list(ALBEDO_AREA.floats))
        records = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertNotIn(GazeCommand.SET_DISPLAY_AREA, device.commands())
        self.assertEqual([r['name'] for r in records if r['kind'] == 'would_send'], ['SET_DISPLAY_AREA'])
        self.assertTrue(next(r for r in records if r['kind'] == 'apply')['identical_to_device'])
