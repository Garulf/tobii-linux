import base64
import gzip
import json
from pathlib import Path
import re
import tempfile
import unittest

from tobii_linux import monitors as mon

# albedo's two LG 27GL850 EDIDs (base blocks; Windows registry through WMI, live, read only).
EDID_UID41221 = bytes.fromhex(
    '00ffffffffffff001e6d7f5b0b220100061e0104b53c22789f8cb5af4f43ab260e5054254b007140818081c0a9c0b300d1c08100'
    'd1cf28de0050a0a038500830080455502100001a000000fd003090e6e63c010a202020202020000000fc003237474c3835300a'
    '2020202020000000ff003030364e54435a32363235310a01fa')
EDID_UID41219 = bytes.fromhex(
    '00ffffffffffff001e6d7f5b5fc20300061e0104b53c22789f8cb5af4f43ab260e5054254b007140818081c0a9c0b300d1c08100'
    'd1cf28de0050a0a038500830080455502100001a000000fd003090e6e63c010a202020202020000000fc003237474c3835300a'
    '2020202020000000ff003030364e544e4837383336370a01fc')
# displayinfo.setpm on albedo (live): the display id Tobii stored for the tracked monitor.
ALBEDO_DISPLAY_ID = 'DISPLAY\\GSM5B7F\\5&5028A6B&0&UID41221##eId:1E6D7F5B0B220100061E#or:1#v:1.0'

# What WINDOWS_QUERY prints for albedo's layout (screens as the user reported them: DISPLAY2 2560x1440 primary at
# 0,0 and DISPLAY1 portrait at -1440,-588); synthetic composition of live values, NOT a captured query output.
WINDOWS_OUTPUT = json.dumps([
    {'name': '\\\\.\\DISPLAY2', 'primary': True, 'bounds': [0, 0, 2560, 1440], 'orientation': 0,
     'interface': '\\\\?\\DISPLAY#GSM5B7F#5&5028a6b&0&UID41221#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}',
     'edid': base64.b64encode(EDID_UID41221).decode()},
    {'name': '\\\\.\\DISPLAY1', 'primary': False, 'bounds': [-1440, -588, 1440, 2560], 'orientation': 1,
     'interface': '\\\\?\\DISPLAY#GSM5B7F#5&5028a6b&0&UID41219#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}',
     'edid': base64.b64encode(EDID_UID41219).decode()}])

# xrandr --prop for the same layout under X11 (synthetic, in xrandr 1.5's format).
XRANDR_OUTPUT = """Screen 0: minimum 320 x 200, current 4000 x 2560, maximum 16384 x 16384
DP-1 connected primary 2560x1440+1440+588 (normal left inverted right x axis y axis) 597mm x 336mm
\tEDID:
\t\t""" + EDID_UID41221[:64].hex() + """
\t\t""" + EDID_UID41221[64:].hex() + """
\tnon-desktop: 0
\t\tsupported: 0, 1
   2560x1440    143.97*+  59.95
HDMI-1 connected 1440x2560+0+0 left (normal left inverted right x axis y axis) 597mm x 336mm
\tEDID:
\t\t""" + EDID_UID41219.hex() + """
   2560x1440     59.95*+
DP-2 disconnected (normal left inverted right x axis y axis)
HDMI-2 connected (normal left inverted right x axis y axis)
"""


class WindowsTests(unittest.TestCase):
    def test_query_output_gives_instance_ids_bounds_and_tobii_display_id(self):
        primary, portrait = mon.parse_windows_monitors(WINDOWS_OUTPUT)
        self.assertEqual(primary.key, 'DISPLAY\\GSM5B7F\\5&5028A6B&0&UID41221')
        self.assertEqual(primary.display_id, ALBEDO_DISPLAY_ID)
        self.assertEqual((primary.bounds, primary.primary, primary.size_mm), ((0, 0, 2560, 1440), True, (597.0, 336.0)))
        self.assertEqual((portrait.rotation, portrait.size_mm), (90, (336.0, 597.0)))
        self.assertTrue(portrait.display_id.endswith('#or:2#v:1.0'))

    def test_a_single_screen_is_printed_as_an_object(self):
        single = json.dumps(json.loads(WINDOWS_OUTPUT)[0])
        self.assertEqual(len(mon.parse_windows_monitors(single)), 1)

    def test_encoded_command_fits_a_command_line_and_holds_the_query(self):
        encoded = mon.encoded_command(mon.WINDOWS_QUERY)
        self.assertLess(len(encoded) + 60, 8191)
        bootstrap = base64.b64decode(encoded).decode('utf-16-le')
        packed = re.search(r"FromBase64String\('([^']+)'\)", bootstrap)[1]
        self.assertEqual(gzip.decompress(base64.b64decode(packed)).decode(), mon.WINDOWS_QUERY)

    def test_windows_monitors_runs_powershell(self):
        calls = []

        class Result:
            returncode, stdout, stderr = 0, WINDOWS_OUTPUT, ''

        def run(command, **kwargs):
            calls.append(command)
            return Result()
        self.assertEqual(len(mon.windows_monitors(run)), 2)
        self.assertEqual(calls[0][:4], ['powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand'])


class LinuxTests(unittest.TestCase):
    def test_xrandr_connected_outputs_with_geometry_rotation_and_edid(self):
        monitors = mon.parse_xrandr(XRANDR_OUTPUT)
        self.assertEqual([m.name for m in monitors], ['DP-1', 'HDMI-1'])
        dp, hdmi = monitors
        self.assertEqual((dp.bounds, dp.primary, dp.rotation, dp.edid), ((1440, 588, 2560, 1440), True, 0, EDID_UID41221))
        self.assertEqual((hdmi.bounds, hdmi.rotation, hdmi.edid), ((0, 0, 1440, 2560), 90, EDID_UID41219))
        self.assertEqual(hdmi.size_mm, (336.0, 597.0))
        self.assertIsNone(dp.display_id)

    def test_drm_fallback_has_edids_but_no_geometry(self):
        with tempfile.TemporaryDirectory() as root:
            for connector, status, edid in (('card0-DP-1', 'connected', EDID_UID41221),
                                            ('card0-HDMI-A-1', 'disconnected', b'')):
                (Path(root)/connector).mkdir()
                (Path(root)/connector/'status').write_text(status + '\n')
                (Path(root)/connector/'edid').write_bytes(edid)
            monitors = mon.drm_fallback(root)
        self.assertEqual([(m.name, m.bounds) for m in monitors], [('DP-1', None)])
        self.assertEqual(monitors[0].size_mm, (597.0, 336.0))


class RuleTests(unittest.TestCase):
    def setUp(self):
        self.primary, self.portrait = mon.parse_windows_monitors(WINDOWS_OUTPUT)
        self.right = mon.Monitor('third', 'third', (2560, 0, 1920, 1080))
        self.all = [self.portrait, self.primary, self.right]

    def test_previous_and_next_follow_the_left_edges(self):
        self.assertEqual(mon.previous_monitor(self.all, self.primary), self.portrait)
        self.assertEqual(mon.next_monitor(self.all, self.primary), self.right)
        self.assertIsNone(mon.previous_monitor(self.all, self.portrait))
        self.assertIsNone(mon.next_monitor(self.all, self.right))
        self.assertIsNone(mon.next_monitor([self.primary], self.primary))

    def test_initial_monitor(self):
        self.assertEqual(mon.initial_monitor([self.portrait]), self.portrait)
        self.assertEqual(mon.initial_monitor(self.all, self.portrait.key), self.portrait)
        self.assertEqual(mon.initial_monitor(self.all, 'unplugged'), self.primary)
        self.assertEqual(mon.initial_monitor(self.all), self.primary)

    def test_aspect_check_ignores_orientation(self):
        self.assertTrue(mon.has_recommended_aspect(self.primary))
        self.assertTrue(mon.has_recommended_aspect(self.portrait))
        self.assertFalse(mon.has_recommended_aspect(self.primary, (1280, 1024)))
        self.assertTrue(mon.has_recommended_aspect(mon.Monitor('x', 'x', (0, 0, 1280, 1024))))


if __name__ == '__main__':
    unittest.main()
