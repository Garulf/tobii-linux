import argparse
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import tempfile
import unittest

from tobii_linux import calibration_profiles as cp
from tobii_linux import gaze_device as gd
from tobii_linux import ttp_commands as tc
from tobii_linux.driver import TrackerSession
from tobii_linux.gaze_device import GazeCommand
from tobii_linux.ttp import Decoder, Message, fields
from tobii_linux.ttp_fields import encode_payload, struct_field, uint32_field, value

TOOLS = Path(__file__).resolve().parents[1]/'tools'
WRITES = {GazeCommand.CALIBRATION_APPLY, GazeCommand.SET_ENABLED_EYE, GazeCommand.CALIBRATION_START,
          GazeCommand.CALIBRATION_STOP, GazeCommand.CALIBRATION_CLEAR}


def blob(calibration_id):
    header = struct.pack('<5I', 110, 84, 1, 0, calibration_id) + b'human'.ljust(64, b'\0')
    return struct.pack('<I', 4 + len(header)) + header + struct.pack('<Ii', 0, 0)


class ProfileDevice:
    """A device holding one calibration; 1110 replaces it and takes the id from blob word 5."""

    def __init__(self, calibration_id=0xBC381A37):
        self.blob = blob(calibration_id)
        self.enabled_eye = 3
        self.requests = Decoder()
        self.outgoing = []
        self.log = []

    @property
    def calibration_id(self):
        return gd.blob_calibration_id(self.blob)

    def write(self, data):
        for request in self.requests.feed(data):
            self.log.append(request)
            items = []
            if request.command == tc.Command.GET_DEVICE_STATUS:
                items = [struct_field(2, gd.STATUS_STRUCT), uint32_field(gd.STATUS_CALIBRATION_ID),
                         tc.string_field(str(self.calibration_id))]
            elif request.command == GazeCommand.CALIBRATION_RETRIEVE:
                items = [tc.blob_field(self.blob)]
            elif request.command == GazeCommand.CALIBRATION_APPLY:
                self.blob = value(fields(request.payload)[0])
            elif request.command == GazeCommand.SET_ENABLED_EYE:
                self.enabled_eye = value(fields(request.payload)[0])
            payload = encode_payload(items) if items else b''
            self.outgoing.append(Message(tc.RESPONSE_KIND, request.request_id, 0, request.command, 0, payload).encode())

    def read(self, timeout_ms):
        return self.outgoing.pop(0) if self.outgoing else b''

    def commands(self):
        return [m.command for m in self.log]


def run_tool(arguments, session=None):
    spec = importlib.util.spec_from_file_location('gaze_calibrate', TOOLS/'gaze_calibrate.py')
    tool = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(tool)
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        if arguments.action == 'list':
            status = tool.list_profiles(cp.ProfileStore(arguments.directory))
        else:
            status = tool.profiles(session, arguments)
    return status, [json.loads(line) for line in output.getvalue().splitlines()]


class StoreTests(unittest.TestCase):
    def test_default_directory_follows_xdg(self):
        self.assertEqual(cp.default_directory({'XDG_DATA_HOME': '/data'}), Path('/data/tobii-linux/profiles'))
        self.assertEqual(cp.default_directory({'HOME': '/home/u'}), Path('/home/u/.local/share/tobii-linux/profiles'))

    def test_names_follow_the_windows_rules(self):
        for name in ('Garulf', 'Guest (glasses)', 'player-2', 'Åsa'):
            self.assertEqual(cp.check_name(name), name)
        for name in ('', '   ', 'a/b', '../x', 'x.y', 'a' * 256):
            with self.assertRaises(cp.ProfileError):
                cp.check_name(name)

    def test_save_load_list_and_current(self):
        with tempfile.TemporaryDirectory() as directory:
            store = cp.ProfileStore(directory)
            self.assertEqual((store.names(), store.current()), ([], None))
            path = store.save('Garulf', gd.LocalCalibration(7, blob(7)))
            self.assertEqual(path.read_bytes()[:16], struct.pack('<4I', 4, 1, len(blob(7)) + 4, 7))
            store.save('Guest', gd.LocalCalibration(9, blob(9)))
            self.assertEqual(store.names(), ['Garulf', 'Guest'])
            self.assertEqual(store.load('Guest').calibration, gd.LocalCalibration(9, blob(9)))
            with self.assertRaises(FileExistsError):
                store.save('Guest', gd.LocalCalibration(9, blob(9)))
            store.set_current('Guest')
            self.assertEqual(store.current(), 'Guest')
            with self.assertRaises(cp.ProfileError):
                store.load('Nobody')


class DeviceTests(unittest.TestCase):
    def test_save_from_device_only_reads(self):
        device = ProfileDevice()
        with tempfile.TemporaryDirectory() as directory:
            profile = cp.save_from_device(TrackerSession(device), cp.ProfileStore(directory), 'Garulf')
            self.assertEqual(profile.calibration, gd.LocalCalibration(0xBC381A37, device.blob))
        self.assertFalse(set(device.commands()) & WRITES)

    def test_switch_sends_enabled_eye_then_apply_and_marks_current(self):
        device = ProfileDevice()
        with tempfile.TemporaryDirectory() as directory:
            store = cp.ProfileStore(directory)
            store.save('Guest', gd.LocalCalibration(9, blob(9)))
            result = cp.switch(TrackerSession(device), store, 'Guest')
            self.assertEqual(store.current(), 'Guest')
        self.assertEqual(device.commands(), [tc.Command.GET_DEVICE_STATUS, GazeCommand.SET_ENABLED_EYE,
                                             GazeCommand.CALIBRATION_APPLY, tc.Command.GET_DEVICE_STATUS])
        self.assertEqual((result.device_id_before, result.device_id_after, result.applied), (0xBC381A37, 9, True))
        self.assertEqual((device.enabled_eye, device.blob), (3, blob(9)))

    def test_switch_to_the_active_calibration_sends_nothing(self):
        device = ProfileDevice()
        with tempfile.TemporaryDirectory() as directory:
            store = cp.ProfileStore(directory)
            store.save('Garulf', gd.LocalCalibration(0xBC381A37, device.blob))
            self.assertFalse(cp.switch(TrackerSession(device), store, 'Garulf').applied)
        self.assertFalse(set(device.commands()) & WRITES)

    def test_tool_profiles(self):
        device = ProfileDevice()
        with tempfile.TemporaryDirectory() as directory:
            options = dict(directory=directory, overwrite=False, dry_run=False)
            status, records = run_tool(argparse.Namespace(action='save', name='Garulf', **options),
                                       TrackerSession(device))
            self.assertEqual((status, records[0]['calibration_id']), (0, '0xbc381a37'))
            cp.ProfileStore(directory).save('Guest', gd.LocalCalibration(9, blob(9)))
            status, records = run_tool(argparse.Namespace(action='switch', name='Guest',
                                                          **{**options, 'dry_run': True}), TrackerSession(device))
            self.assertEqual(status, 0)
            self.assertEqual([r['name'] for r in records if r['kind'] == 'would_send'],
                             ['SET_ENABLED_EYE', 'CALIBRATION_APPLY'])
            switch = next(r for r in records if r['kind'] == 'profile_switch')
            self.assertEqual((switch['applied'], switch['device_id_after']), (True, '0xbc381a37'))
            self.assertIsNone(cp.ProfileStore(directory).current())
            status, records = run_tool(argparse.Namespace(action='list', name=None, **options))
            self.assertEqual([r['name'] for r in records], ['Garulf', 'Guest'])
        self.assertFalse(set(device.commands()) & WRITES)


if __name__ == '__main__':
    unittest.main()
