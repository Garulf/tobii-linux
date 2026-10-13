import hmac
import sys
from pathlib import Path
import unittest

from tobii_linux import driver
from tobii_linux import ttp_commands as tc
from tobii_linux.camera_info import hardware_info_response
from tobii_linux.ttp import Decoder, Field, Message
from tobii_linux.ttp_fields import encode_payload, int32_field, uint32_field

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_camera_info import INFO  # noqa: E402
from test_frames import image_columns, stream_message  # noqa: E402

# Scripted device built from the recovered request and reply layouts, NOT device traffic.
SALT = b'0123456789abcdef'


def blob(data):
    return Field(tc.FIELD_BLOB, len(data).to_bytes(4, 'big') + data)


class FakeDevice:
    def __init__(self):
        self.requests = Decoder()
        self.outgoing = []
        self.log = []

    def reply(self, request, items=()):
        payload = encode_payload(list(items)) if items else b''
        self.outgoing.append(Message(tc.RESPONSE_KIND, request.request_id, 0, request.command, 0, payload).encode())

    def write(self, data):
        for request in self.requests.feed(data):
            self.log.append(request)
            if request.command == tc.Command.UNLOCK_PPU:
                self.reply(request, (int32_field(1), int32_field(77), int32_field(0)))
            elif request.command == tc.Command.OPEN_REALM_SALT:
                self.reply(request, (uint32_field(5), uint32_field(6), blob(SALT)))
            elif request.command == 0x848:
                self.outgoing.append(hardware_info_response(INFO, request.request_id).encode())
            elif request.command == tc.Command.SUBSCRIBE_STREAM:
                self.reply(request)
                for _ in range(2):
                    self.outgoing.append(stream_message(tc.Stream.IMAGE, image_columns()).encode())
            else:
                self.reply(request)

    def read(self, timeout_ms):
        return self.outgoing.pop(0) if self.outgoing else b''


class TrackerSessionTests(unittest.TestCase):
    def test_startup_calibration_and_frames(self):
        device = FakeDevice()
        session = driver.TrackerSession(device)
        session.initialize()
        calibration = session.camera_calibration()
        session.subscribe_images()
        frames = list(session.frames(timeout_s=0.05))
        commands = [m.command for m in device.log]
        self.assertEqual(commands, [1000, 1600, 1900, 1910, 1900, 1911, 0x848, 1915, 1220])
        unlock_answer, realm_answer = device.log[3], device.log[5]
        unlock_digest = hmac.new(driver.UNLOCK_REALM_KEY, SALT, 'md5').digest()
        realm_digest = hmac.new(driver.HEAD_POSE_REALM_KEY, SALT, 'md5').digest()
        self.assertIn(unlock_digest, unlock_answer.payload)
        self.assertIn(realm_digest, realm_answer.payload)
        self.assertIn(driver.HEAD_POSE_REALM_EXTRA, realm_answer.payload)
        self.assertEqual(calibration.translation, (1.0, -2.0, 0.5))
        self.assertEqual(calibration.angles, (0.0, 1.0, 2.0))
        self.assertEqual([(f.width, f.height) for f in frames], [(4, 3), (4, 3)])

    def test_failed_command_raises(self):
        class Refusing(FakeDevice):
            def reply(self, request, items=()):
                self.outgoing.append(Message(tc.RESPONSE_KIND, request.request_id, 0, request.command, 3, b'').encode())
        with self.assertRaises(driver.DeviceError):
            driver.TrackerSession(Refusing()).initialize()


if __name__ == '__main__':
    unittest.main()
