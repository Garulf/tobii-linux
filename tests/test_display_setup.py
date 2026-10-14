import argparse
import contextlib
import importlib.util
import io
import json
import math
from pathlib import Path
import tempfile
import unittest

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
