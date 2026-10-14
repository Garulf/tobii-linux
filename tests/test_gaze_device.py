import struct
import unittest

from tobii_linux import gaze_device as gd
from tobii_linux.gaze_device import CalibrationState, Eye, GazeCommand
from tobii_linux.ttp import ProtocolError
from tobii_linux.ttp_commands import Command
from tobii_linux.ttp_fields import StructHeader

# Bytes produced by the original builders in emulation
# (tools/gaze_device_oracle.py), NOT captured USB traffic.
ORIGINAL = {
    'retrieve': '0000005100000001000000000000044c0000000000000000',
    'collect_2d': '00000051000000070000000000000406000000000000002500000400000008000002000000000004'
                  '000000080000020000000000020000000400000003',
    'collect_3d': '0000005100000008000000000000040700000000000000320000040000000800000000000000000400'
                  '000008000000000000000004000000080009600000000000020000000400000001',
    'start_eye': '000000510000000900000000000003f3000000000000000b0000020000000400000002',
    'file_read': '000000510000000a0000000000000c08000000000000000c000014000000050000000161',
    'apply': '000000510000000c000000000000045600000000000000160000150000000f0000000b63616c6962726174696f6e',
    'display_area': '000000510000000d00000000000005a000000000000000a40000050000000400031f41040000000800'
                    '000000000000000400000008000000000000000004000000080000000000000000050000000400031f'
                    '410400000008000004000000000004000000080000080000000000040000000800000c000000000005'
                    '0000000400031f410400000008ffffee0000000000040000000800001500000000000400000008ffff'
                    'e78000000000050000000400010100020000000400003039',
}


def setting_file(payload):
    return struct.pack('<3I', 4, 1, len(payload)) + payload


def calibration_file(calibration_id, blob):
    return setting_file(struct.pack('<I', calibration_id) + blob)


def status_values(entries):
    values = []
    for key, text in entries.items():
        values += [StructHeader(2, gd.STATUS_STRUCT), key, text]
    return values


class FakeSession:
    def __init__(self, status):
        self.status = status
        self.sent = []

    def call(self, build):
        message = build(len(self.sent) + 1)
        self.sent.append(message.command)
        return message

    def reply_values(self, reply):
        return self.status if reply.command == Command.GET_DEVICE_STATUS else []


class EncoderTests(unittest.TestCase):
    def test_original_builder_bytes(self):
        cases = {
            'retrieve': gd.header_only(1, GazeCommand.CALIBRATION_RETRIEVE),
            'collect_2d': gd.calibration_collect_2d(7, 0.5, 0.5),
            'collect_3d': gd.calibration_collect_3d(8, 0.0, 0.0, 600.0, Eye.LEFT),
            'start_eye': gd.calibration_start_eye(9, Eye.RIGHT),
            'file_read': gd.persistent_file_read(10, 'a'),
            'apply': gd.calibration_apply(12, b'calibration'),
            'display_area': gd.set_display_area(13, (0, 0, 0), (1, 2, 3), (-4.5, 5.25, -6.125)),
        }
        for name, message in cases.items():
            with self.subTest(name):
                self.assertEqual(message.encode().hex(), ORIGINAL[name])

    def test_header_only_rejects_commands_with_fields(self):
        with self.assertRaises(ProtocolError):
            gd.header_only(1, GazeCommand.CALIBRATION_APPLY)

    def test_eye_values_are_validated(self):
        with self.assertRaises(ValueError):
            gd.set_enabled_eye(1, 4)

    def test_points_are_rounded_to_float32_before_fixed_point(self):
        exact = gd.calibration_collect_2d(1, 0.1, 0.9).encode()
        rounded = gd.calibration_collect_2d(1, struct.unpack('<f', struct.pack('<f', 0.1))[0], 0.9).encode()
        self.assertEqual(exact, rounded)

    def test_read_only_set_excludes_writes(self):
        writes = {GazeCommand.CALIBRATION_APPLY, GazeCommand.PERSISTENT_FILE_WRITE,
                  GazeCommand.PERSISTENT_FILE_ERASE, GazeCommand.SET_DISPLAY_AREA,
                  GazeCommand.CALIBRATION_CLEAR, GazeCommand.CALIBRATION_START}
        self.assertFalse(writes & gd.READ_ONLY)


class StatusTests(unittest.TestCase):
    def test_calibration_id_from_status_key_7(self):
        values = status_values({0: b'false', 4: b'HELLO_RGB', 7: b'3157793335', 8: b'false'})
        self.assertEqual(gd.device_calibration_id(values), 0xBC381A37)

    def test_missing_calibration_id(self):
        self.assertIsNone(gd.device_calibration_id(status_values({0: b'false'})))

    def test_malformed_status_is_refused(self):
        with self.assertRaises(ProtocolError):
            gd.status_entries([StructHeader(2, gd.STATUS_STRUCT), 7])


class SettingFileTests(unittest.TestCase):
    def test_calibration_setting(self):
        local = gd.parse_calibration_setting(calibration_file(0x1234, b'blob bytes'))
        self.assertEqual((local.calibration_id, local.blob), (0x1234, b'blob bytes'))

    def test_calibration_setting_size_mismatch(self):
        with self.assertRaises(ProtocolError):
            gd.parse_calibration_setting(calibration_file(1, b'abc') + b'\0')

    def test_calibration_setting_without_blob(self):
        with self.assertRaises(ProtocolError):
            gd.parse_calibration_setting(setting_file(struct.pack('<I', 1)))

    def test_screen_plane(self):
        corners = ((-296.5, 10.25, -3.0), (-296.5, 326.0, 111.75), (300.5, 326.0, 111.75))
        data = setting_file(struct.pack('<9f', *[c for p in corners for c in p]))
        self.assertEqual(gd.screen_plane_setting(data), corners)


class SynchronizeTests(unittest.TestCase):
    local = gd.LocalCalibration(0xBC381A37, b'blob')

    def session(self, device_id):
        entries = {7: str(device_id).encode()} if device_id is not None else {}
        return FakeSession(status_values(entries))

    def test_in_sync_sends_only_status(self):
        session = self.session(0xBC381A37)
        self.assertEqual(gd.synchronize_calibration(session, self.local), CalibrationState.IN_SYNC)
        self.assertEqual(session.sent, [Command.GET_DEVICE_STATUS])

    def test_mismatch_needs_permission_to_apply(self):
        session = self.session(5)
        self.assertEqual(gd.synchronize_calibration(session, self.local), CalibrationState.APPLY_NEEDED)
        self.assertEqual(session.sent, [Command.GET_DEVICE_STATUS])

    def test_mismatch_applies_when_allowed(self):
        session = self.session(None)
        state = gd.synchronize_calibration(session, self.local, allow_apply=True)
        self.assertEqual(state, CalibrationState.APPLIED)
        self.assertEqual(session.sent, [Command.GET_DEVICE_STATUS, GazeCommand.CALIBRATION_APPLY])

    def test_device_only_calibration_is_never_cleared(self):
        session = self.session(0xBC381A37)
        state = gd.synchronize_calibration(session, None, allow_apply=True)
        self.assertEqual(state, CalibrationState.DEVICE_ONLY)
        self.assertEqual(session.sent, [Command.GET_DEVICE_STATUS])


if __name__ == '__main__':
    unittest.main()
