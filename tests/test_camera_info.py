from dataclasses import replace
import unittest

from tobii_linux.camera_info import (CameraRecord, HardwareInfo, hardware_info_response,
                                     parse_hardware_info, read_hardware_info)
from tobii_linux.ttp import Decoder, Message, ProtocolError

# Hardware-info response assembled from the reader instructions (0x1402cc1c0,
# 0x1403b11b0), NOT captured from a device. One camera, one point.
CAMERA_ITEMS = (
    "0500000004001a332c"
    "0100000004fffffffe" "030000000400018000" "0300000004ffffc000"
    "050000000400031f41" "04000000080000040000000000" "0400000008fffff80000000000"
    "04000000080000020000000000"
    + "".join(f"0400000008{n << 42:016x}" for n in range(15))
    + "010000000400000280" "0100000004000001e0" "010000000400000001"
    "190000000c000000010000008000000000"
    "050000000400031f41" "04000000080000000000000000" "04000000080000000000000000"
    "04000000080000040000000000"
    "050000000400031f41" "0400000008fffffc0000000000" "04000000080000000000000000"
    "04000000080000000000000000"
    "04000000080000080000000000")
PAYLOAD = bytes.fromhex(
    "0000" "050000000400020100" "020000000400000000" + CAMERA_ITEMS
    + "050000000400020100" "020000000400000000"
    "050000000400031f41" "040000000800000c0000000000" "0400000008fffff00000000000"
    "04000000080000030000000000"
    "010000000400000005")
CAMERA = CameraRecord(-2, 1.5, -0.25, (1.0, -2.0, 0.5), tuple(float(n) for n in range(15)),
                      640, 480, 1, (0.125,), (0.0, 0.0, 1.0), (-1.0, 0.0, 0.0), 2.0)
INFO = HardwareInfo((CAMERA,), ((3.0, -4.0, 0.75),), 5)


def response(payload=PAYLOAD, word4=0, command=0x848, kind=0x52):
    return Message(kind, 7, 0, command, word4, payload)


class CameraInfoTests(unittest.TestCase):
    def test_reconstructed_vector(self):
        self.assertEqual(parse_hardware_info(response()), INFO)

    def test_encoder_reproduces_vector(self):
        self.assertEqual(hardware_info_response(INFO, 7), response())

    def test_through_stream_decoder(self):
        data = response().encode() * 2
        messages = Decoder().feed(data)
        self.assertEqual([parse_hardware_info(m) for m in messages], [INFO, INFO])

    def test_series_beyond_sixty_four_is_dropped(self):
        long_camera = replace(CAMERA, series_0xb0=tuple(float(n) for n in range(70)))
        parsed = parse_hardware_info(hardware_info_response(HardwareInfo((long_camera,), (), 0), 1))
        self.assertEqual(parsed.cameras_0x8[0].series_0xb0, tuple(float(n) for n in range(64)))
        self.assertEqual(parsed.cameras_0x8[0].series_count_0xac, 64)

    def test_points_are_float32(self):
        info = HardwareInfo((), ((0.1, 0.2, 0.3),), 0)
        x, y, z = parse_hardware_info(hardware_info_response(info, 1)).points_0xbb0[0]
        self.assertEqual((x, y, z), (0.10000000149011612, 0.20000000298023224, 0.30000001192092896))

    def test_tracker_limits(self):
        for info in (HardwareInfo((CAMERA,) * 5, (), 0), HardwareInfo((), ((0.0, 0.0, 0.0),) * 41, 0)):
            message = hardware_info_response(info, 1)
            self.assertEqual(len(read_hardware_info(message.payload).cameras_0x8), len(info.cameras_0x8))
            with self.assertRaises(ProtocolError):
                parse_hardware_info(message)

    def test_error_status_and_wrong_message(self):
        for message in (response(word4=0x20000503), response(command=0x847), response(kind=0x51)):
            with self.assertRaises(ProtocolError):
                parse_hardware_info(message)

    def test_malformed_payloads(self):
        wrong_header = PAYLOAD.replace(bytes.fromhex("001a332c"), bytes.fromhex("0019332c"))
        for payload in (PAYLOAD[:-1], PAYLOAD[:40], wrong_header, b"\0\1" + PAYLOAD[2:]):
            with self.assertRaises(ProtocolError):
                parse_hardware_info(response(payload))


if __name__ == "__main__":
    unittest.main()
