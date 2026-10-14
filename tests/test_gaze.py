"""Gaze host path: device gaze messages to the records and packets the Windows runtime publishes.

ORIGINAL_* vectors come from tools/gaze_oracle.py: one live 0x500 message recorded on
Linux (evidence/private/gaze-device/linux-gaze-image.jsonl) run through the original
runtime parser, dispatcher, receive_gaze_data and service callbacks, and the game
DLL's packet serializer (version 0 and 0x7fffffff). REAL_* packets were captured from
the real ET5 service on albedo with tools/prp_real_service_probe.py
(evidence/private/gaze-host/gaze-probe-fou1.jsonl), with a person in front.
"""
import json
import math
import struct
import tempfile
import unittest

from tobii_linux import gaze
from tobii_linux.prp import decode_envelope, decode_gaze_body
from tobii_linux.ttp import Message

DEVICE_MESSAGE_PAYLOAD = bytes.fromhex(
    "0000050000000400270bb8050000000400020bb90200000004000000010600000008000000d18c9850490500"
    "00000400020bb9020000000400000011020000000400000004050000000400020bb902000000040000001402"
    "0000000403426327050000000400020bb902000000040000001b020000000400000001050000000400020bb9"
    "02000000040000001c050000000400021f400400000008000002a3830800000400000008000002e53cf40000"
    "050000000400020bb9020000000400000016020000000400000001050000000400020bb90200000004000000"
    "02050000000400031f410400000008fffed533e60000000400000008ffff2930820000000400000008000b39"
    "1f80000000050000000400020bb9020000000400000021020000000400000001050000000400020bb9020000"
    "000400000022050000000400031f410400000008fffecdaede000000040000000800004a7568000000040000"
    "0008000aef0540000000050000000400020bb9020000000400000003050000000400031f4104000000080000"
    "028d94d80000040000000800000265b8940000040000000800000297db3c0000050000000400020bb9020000"
    "00040000001d020000000400000001050000000400020bb9020000000400000004050000000400031f410400"
    "00000800019391ce0000000400000008000186ee020000000400000008000072f04e00000005000000040002"
    "0bb9020000000400000005050000000400021f400400000008000002a9d49800000400000008000002e45520"
    "0000050000000400020bb9020000000400000006030000000400037817050000000400020bb9020000000400"
    "000007020000000400000000050000000400020bb9020000000400000017050000000400031f410400000008"
    "fffed256680000000400000008ffff23be1f0000000400000008000b4a7690000000050000000400020bb902"
    "0000000400000019050000000400021f400400000008fffffc00000000000400000008fffffc000000000005"
    "0000000400020bb9020000000400000015020000000400000001050000000400020bb9020000000400000008"
    "050000000400031f410400000008ffffd9b3ca0000000400000008ffff3a7f670000000400000008000b4747"
    "60000000050000000400020bb9020000000400000023020000000400000001050000000400020bb902000000"
    "0400000024050000000400031f410400000008ffffd22ec2000000040000000800005f908800000004000000"
    "08000af66720000000050000000400020bb9020000000400000009050000000400031f410400000008000002"
    "11dbc8000004000000080000025d0c080000040000000800000296abc80000050000000400020bb902000000"
    "040000001e020000000400000001050000000400020bb902000000040000000a050000000400031f41040000"
    "00080001df885000000004000000080001b5a9480000000400000008000083f285000000050000000400020b"
    "b902000000040000000b050000000400021f400400000008000002ca677800000400000008000002be715000"
    "00050000000400020bb902000000040000000c030000000400040725050000000400020bb902000000040000"
    "000d020000000400000000050000000400020bb9020000000400000018050000000400031f410400000008ff"
    "ffd5265c0000000400000008ffff3559d80000000400000008000b585900000000050000000400020bb90200"
    "0000040000001a050000000400021f400400000008fffffc00000000000400000008fffffc00000000000500"
    "00000400020bb902000000040000001f020000000400000001050000000400020bb902000000040000002005"
    "0000000400021f400400000008000002ba1e0800000400000008000002d163380000050000000400020bb902"
    "0000000400000025050000000400031f41040000000800000295dd1c000004000000080000026de654000004"
    "0000000800000198bf440000050000000400020bb90200000004000000260200000004000000010500000004"
    "00020bb9020000000400000027050000000400031f4104000000080000020fb9cc0000040000000800000263"
    "3ed40000040000000800000190c6fa0000050000000400020bb9020000000400000028020000000400000001"
    "050000000400020bb90200000004000000290300000004ffff0000050000000400020bb90200000004000000"
    "2a020000000400000000050000000400020bb902000000040000002b0300000004ffff000005000000040002"
    "0bb902000000040000002c020000000400000000")

ORIGINAL_PLATMOD = {
    "gaze_point": bytes.fromhex(
        "4950988cd100000001000000c2e0283f3d4f393f"),
    "responsive_gaze_point": bytes.fromhex(
        "4950988cd10000000100000082872e3fce58343f"),
    "eye_openness": bytes.fromhex(
        "4950988cd100000000000000000080bf00000000000080bf"),
    "gaze_origin": bytes.fromhex(
        "4950988cd100000001000000912899c2d0ea944154f02e4401000000f84437c11021bf4172662f44"),
    "gaze_data": bytes.fromhex(
        "4950988cd1000000010000000d6695c27ecf56c2f8913344010000003665233f256e193fcff6253f"
        "01000000e7c8c9420177c3429ce0e54126752a3f4815393f01000000ccd496c2e1415cc269a73444"
        "01000000c0055e4001000000d83019c1998045c27674344401000000f276043f0243173ff2aa253f"
        "0100000028c4ef42a4d4da4285f20342de99323f549c2f3f0100000090662bc128a64ac290853544"
        "01000000a0e48040"),
    "user_position_guide": bytes.fromhex(
        "4950988cd1000000010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f7d63c83e"),
}

ORIGINAL_PRP = {
    "gaze_point": (0x1, 0x3, bytes.fromhex(
        "4950988cd100000001000000c2e0283f3d4f393f")),
    "responsive_gaze_point": (0x1, 0x1d, bytes.fromhex(
        "4950988cd10000000100000082872e3fce58343f")),
    "eye_openness": (0x1, 0x21, bytes.fromhex(
        "4950988cd100000000000000000080bf00000000000080bf")),
    "gaze_origin": (0x1, 0x4, bytes.fromhex(
        "4950988cd100000001000000912899c2d0ea944154f02e4401000000f84437c11021bf4172662f44")),
    "gaze_data": (0x1, 0x6, bytes.fromhex(
        "4950988cd1000000010000000d6695c27ecf56c2f8913344010000003665233f256e193fcff6253f"
        "01000000e7c8c9420177c3429ce0e54126752a3f4815393f01000000ccd496c2e1415cc269a73444"
        "01000000c0055e4001000000d83019c1998045c27674344401000000f276043f0243173ff2aa253f"
        "0100000028c4ef42a4d4da4285f20342de99323f549c2f3f0100000090662bc128a64ac290853544"
        "01000000a0e48040")),
    "user_position_guide": (0xb, 0x2, bytes.fromhex(
        "4950988cd1000000010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f7d63c83e")),
}

ORIGINAL_PACKETS = {
    ("gaze_point", 0): bytes.fromhex(
        "545052503c000000685052500c000000000000000100000004000000080000000300000004000000"
        "18000000d10000004950988c01000000c2e0283f3d4f393f0400000000000000"),
    ("gaze_point", 2147483647): bytes.fromhex(
        "545052503c000000685052500c000000000000000100000004000000080000000300000004000000"
        "18000000d10000004950988c01000000c2e0283f3d4f393f0400000000000000"),
    ("responsive_gaze_point", 0): bytes.fromhex(
        "545052503c000000685052500c000000000000000100000004000000080000001d00000004000000"
        "18000000d10000004950988c0100000082872e3fce58343f0400000000000000"),
    ("responsive_gaze_point", 2147483647): bytes.fromhex(
        "545052503c000000685052500c000000000000000100000004000000080000001d00000004000000"
        "18000000d10000004950988c0100000082872e3fce58343f0400000000000000"),
    ("gaze_origin", 0): bytes.fromhex(
        "5450525050000000045052500c000000000000000100000004000000080000000400000004000000"
        "2c000000d10000004950988c01000000912899c2d0ea944154f02e4401000000f84437c11021bf41"
        "72662f440400000000000000"),
    ("gaze_origin", 2147483647): bytes.fromhex(
        "5450525050000000045052500c000000000000000100000004000000080000000400000004000000"
        "2c000000d10000004950988c01000000912899c2d0ea944154f02e4401000000f84437c11021bf41"
        "72662f440400000000000000"),
    ("gaze_data", 0): bytes.fromhex(
        "54505250c80000009c5052500c000000000000000100000004000000080000000600000004000000"
        "a4000000d10000004950988c010000000d6695c27ecf56c2f89133443665233f256e193fcff6253f"
        "01000000e7c8c9420177c3429ce0e54126752a3f4815393f01000000ccd496c2e1415cc269a73444"
        "01000000c0055e4001000000d83019c1998045c276743444f276043f0243173ff2aa253f01000000"
        "28c4ef42a4d4da4285f20342de99323f549c2f3f0100000090662bc128a64ac29085354401000000"
        "a0e480400400000000000000"),
    ("gaze_data", 2147483647): bytes.fromhex(
        "54505250d0000000845052500c000000000000000100000004000000080000000600000004000000"
        "ac000000d10000004950988c010000000d6695c27ecf56c2f89133443665233f256e193fcff6253f"
        "01000000e7c8c9420177c3429ce0e54126752a3f4815393f01000000ccd496c2e1415cc269a73444"
        "01000000c0055e4001000000d83019c1998045c276743444f276043f0243173ff2aa253f01000000"
        "28c4ef42a4d4da4285f20342de99323f549c2f3f0100000090662bc128a64ac29085354401000000"
        "a0e4804001000000010000000400000000000000"),
    ("user_position_guide", 0): bytes.fromhex(
        "5450525050000000045052500c000000000000000b00000004000000080000000200000004000000"
        "2c000000d10000004950988c010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f"
        "7d63c83e0400000000000000"),
    ("user_position_guide", 2147483647): bytes.fromhex(
        "5450525050000000045052500c000000000000000b00000004000000080000000200000004000000"
        "2c000000d10000004950988c010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f"
        "7d63c83e0400000000000000"),
}
REAL_GAZE_POINT_VALID = bytes.fromhex(
    "545052503c000000685052500c00000000000000010000000400000008000000030000000400000018000000"
    "d1000000934429a4010000004a79083f06544e3e0400000000000000")
REAL_GAZE_POINT_INVALID = bytes.fromhex(
    "545052503c000000685052500c00000000000000010000000400000008000000030000000400000018000000"
    "d100000093b82fa400000000000080bf000080bf0400000000000000")
REAL_RESPONSIVE_VALID = bytes.fromhex(
    "545052503c000000685052500c000000000000000100000004000000080000001d0000000400000018000000"
    "d1000000934429a4010000001953093f84685c3e0400000000000000")
REAL_GAZE_ORIGIN_VALID = bytes.fromhex(
    "5450525050000000045052500c0000000000000001000000040000000800000004000000040000002c000000"
    "d1000000934429a401000000f3ca32c3e049144189ea234401000000bd00e4c2406bb8410b2f264404000000"
    "00000000")
REAL_GAZE_ORIGIN_INVALID = bytes.fromhex(
    "5450525050000000045052500c0000000000000001000000040000000800000004000000040000002c000000"
    "d100000093b82fa4000000000000000000000000000000000000000000000000000000000000000004000000"
    "00000000")


def device_message(payload=DEVICE_MESSAGE_PAYLOAD):
    return Message(0x53, 0, 1, 0x500, 0, payload)


def changed(update):
    columns = gaze.message_columns(DEVICE_MESSAGE_PAYLOAD)
    columns.update(update)
    return gaze.columns_payload(columns)


class OriginalCodeVectorTests(unittest.TestCase):
    def setUp(self):
        self.device = gaze.gaze_message(device_message())

    def test_platmod_records_match_receive_gaze_data(self):
        self.assertEqual(gaze.platmod_records(self.device), ORIGINAL_PLATMOD)

    def test_prp_records_match_the_service_callbacks(self):
        for name, (kind, stream, native) in ORIGINAL_PRP.items():
            with self.subTest(name=name):
                record = gaze.prp_record(name, gaze.platmod_from_bytes(name, ORIGINAL_PLATMOD[name]))
                self.assertEqual((record.kind, record.stream, record.native), (kind, stream, native))

    def test_packets_match_the_game_dll_serializer(self):
        for (name, version), packet in ORIGINAL_PACKETS.items():
            with self.subTest(name=name, version=version):
                record = gaze.prp_record(name, gaze.platmod_from_bytes(name, ORIGINAL_PLATMOD[name]))
                self.assertEqual(gaze.stream_packet(record, extended=bool(version)), packet)

    def frame(self):
        return gaze.gaze_frame(self.device)

    def test_eye_openness_has_no_serializer_case(self):
        self.assertIsNone(gaze.stream_packet(self.frame().prp('eye_openness')))

    def test_gaze_point_is_the_device_combined_point(self):
        frame = self.frame()
        self.assertEqual(frame.gaze_point.point, self.device.combined_point)
        self.assertEqual(frame.gaze_point.valid, 1)
        self.assertEqual(frame.responsive_gaze_point.point, self.device.responsive_point)
        self.assertEqual(frame.gaze_origin.left, self.device.left_origin_display)

    def test_host_gaze_setting_publishes_only_the_position_guide(self):
        records = gaze.platmod_records(self.device, host_gaze=True)
        self.assertEqual(records, {'user_position_guide': ORIGINAL_PLATMOD['user_position_guide']})


class RealServiceVectorTests(unittest.TestCase):
    """Real-service packets re-encode identically from the values they carry."""

    def device_for_point(self, packet, stream):
        point = decode_gaze_body(decode_envelope(packet)[28:52])
        values = {'timestamp': point.timestamp}
        if stream == gaze.PRP_GAZE_POINT:
            values.update(combined_point=(point.x, point.y), combined_validity=int(point.valid))
        else:
            values.update(responsive_point=(point.x, point.y), responsive_validity=int(point.valid))
        return gaze.DeviceGaze(**values)

    def test_gaze_points(self):
        for packet, stream in ((REAL_GAZE_POINT_VALID, 3), (REAL_GAZE_POINT_INVALID, 3),
                               (REAL_RESPONSIVE_VALID, 29)):
            with self.subTest(stream=stream):
                frame = gaze.gaze_frame(self.device_for_point(packet, stream))
                self.assertEqual(gaze.frame_packet(frame, stream), packet)

    def test_invalid_gaze_point_keeps_the_device_values(self):
        point = decode_gaze_body(decode_envelope(REAL_GAZE_POINT_INVALID)[28:52])
        self.assertEqual((point.valid, point.x, point.y), (False, -1.0, -1.0))

    def test_gaze_origin(self):
        for packet in (REAL_GAZE_ORIGIN_VALID, REAL_GAZE_ORIGIN_INVALID):
            words = struct.unpack('<11I', decode_envelope(packet)[28:72])
            floats = struct.unpack('<11f', decode_envelope(packet)[28:72])
            device = gaze.DeviceGaze(timestamp=words[1] << 32 | words[2],
                                     left_origin_display_validity=words[3],
                                     left_origin_display=floats[4:7],
                                     right_origin_display_validity=words[7],
                                     right_origin_display=floats[8:11])
            with self.subTest(valid=words[3]):
                self.assertEqual(gaze.frame_packet(gaze.gaze_frame(device), 4), packet)


class DispatcherRuleTests(unittest.TestCase):
    def test_validity_rules(self):
        payload = changed({7: gaze.uint32_column(1), 13: gaze.uint32_column(2),
                           27: gaze.uint32_column(4), 31: gaze.uint32_column(0),
                           33: gaze.uint32_column(2), 35: gaze.uint32_column(1),
                           42: gaze.uint32_column(3)})
        frame = gaze.gaze_frame(gaze.decode_gaze_message(payload))
        self.assertEqual(frame.gaze_point.valid, 1)
        self.assertEqual(frame.responsive_gaze_point.valid, 0)
        self.assertEqual((frame.gaze_origin.left_valid, frame.gaze_origin.right_valid), (0, 1))
        self.assertEqual((frame.gaze_data.left.eye_valid, frame.gaze_data.right.eye_valid), (1, 0))
        self.assertEqual(frame.eye_openness.left_valid, 1)
        self.assertEqual(frame.prp('eye_openness').native[8], 1)

    def test_validity_other_than_one_is_dropped_by_the_service(self):
        record = gaze.prp_record('gaze_origin', gaze.PairRecord(1, 2, (1.0, 2.0, 3.0), 1, (4.0, 5.0, 6.0)))
        self.assertEqual((record.native[8], record.native[24]), (0, 1))

    def test_position_guide_is_clamped(self):
        payload = changed({37: gaze.vec3_column((-0.5, 0.25, 1.75)), 38: gaze.uint32_column(1)})
        guide = gaze.gaze_frame(gaze.decode_gaze_message(payload)).user_position_guide
        self.assertEqual(guide.left, (0.0, 0.25, 1.0))
        self.assertEqual(guide.left_valid, 1)

    def test_nan_passes_the_clamp(self):
        record = gaze.user_position_guide_record(gaze.DeviceGaze(left_position_guide=(math.nan, 0.5, 2.0)))
        self.assertTrue(math.isnan(record.left[0]))
        self.assertEqual(record.left[1:], (0.5, 1.0))

    def test_eyeball_needs_its_column(self):
        frame = gaze.gaze_frame(gaze.decode_gaze_message(changed({23: None})))
        self.assertEqual(frame.gaze_data.left.eyeball_valid, 0)
        self.assertEqual(frame.gaze_data.left.eyeball_centre, (0.0, 0.0, 0.0))
        self.assertEqual(frame.gaze_data.right.eyeball_valid, 1)

    def test_known_column_with_another_type_drops_the_message(self):
        self.assertIsNone(gaze.decode_gaze_message(changed({28: gaze.vec3_column((0.5, 0.5, 0.5))})))
        self.assertIsNone(gaze.decode_gaze_message(changed({7: gaze.int32_column(0)})))

    def test_unknown_and_missing_columns(self):
        record = gaze.decode_gaze_message(changed({99: gaze.vec3_column((1.0, 2.0, 3.0)),
                                                   27: None, 28: None}))
        self.assertEqual((record.combined_validity, record.combined_point), (0, (0.0, 0.0)))

    def test_fixed_point_values_are_narrowed_to_float32(self):
        payload = changed({28: gaze.vec2_column((0.1234567891, -0.98765432123)),
                           6: gaze.fixed16_column(3.14159)})
        record = gaze.decode_gaze_message(payload)
        self.assertEqual(record.combined_point, (struct.unpack('<f', struct.pack('<f', 0.1234567891))[0],
                                                 struct.unpack('<f', struct.pack('<f', -0.98765432123))[0]))
        self.assertEqual(record.left_pupil_diameter, int(3.14159 * 65536) / 65536)

    def test_restamping(self):
        frame = gaze.gaze_frame(gaze.decode_gaze_message(DEVICE_MESSAGE_PAYLOAD))
        moved = frame.with_timestamp(42)
        self.assertEqual((moved.gaze_point.timestamp, moved.gaze_point.point),
                         (42, frame.gaze_point.point))


class PresenceTests(unittest.TestCase):
    def test_device_values_map_as_in_the_runtime(self):
        expected = {0: 1, 1: 2, 2: 3, 3: 1, 7: 1}
        self.assertEqual({v: gaze.prp_presence(gaze.platmod_presence(v)) for v in expected}, expected)

    def test_presence_message(self):
        payload = gaze.columns_payload({1: gaze.int64_column(123), 2: gaze.int32_column(2)})
        self.assertEqual(gaze.presence_message(payload), (123, 2))

    def test_recorded_presence_message_is_int32(self):
        """0x504 as a Linux capture received it with a person in front (gaze-device)."""
        payload = bytes.fromhex(
            '0000050000000400020bb8050000000400020bb90200000004000000010600000008000000d18a5be71e'
            '050000000400020bb9020000000400000002010000000400000002')
        self.assertEqual(gaze.presence_message(payload), (899969443614, 2))

    def test_presence_message_types_as_parse_presence(self):
        timestamp = gaze.int64_column(5)
        cases = {(1, 2): None, (2,): (5, 0), (3,): None}
        payloads = {
            (1, 2): {1: timestamp, 2: gaze.uint32_column(2)},
            (2,): {1: timestamp, 9: gaze.uint32_column(4)},
            (3,): {1: gaze.uint32_column(5), 2: gaze.int32_column(1)},
        }
        for key, columns in payloads.items():
            with self.subTest(columns=key):
                self.assertEqual(gaze.presence_message(gaze.columns_payload(columns)), cases[key])


class OfflineConversionTests(unittest.TestCase):
    def test_capture_rows_convert_to_published_outputs(self):
        rows = [{'kind': 'step', 'name': 'started'},
                {'kind': 'message', 'time': 1.5, 'command': 0x500, 'word2': 0, 'word4': 0,
                 'payload': DEVICE_MESSAGE_PAYLOAD.hex()},
                {'kind': 'message', 'time': 2.0, 'message_kind': 0x53, 'command': 0x504,
                 'word2': 0, 'word4': 0, 'payload': '00'},
                {'kind': 'message', 'time': 2.5, 'message_kind': 0x53, 'command': 0x500,
                 'word2': 0, 'word4': 0,
                 'payload': changed({28: gaze.vec3_column((0.5, 0.5, 0.5))}).hex()}]
        with tempfile.NamedTemporaryFile('w', suffix='.jsonl') as capture:
            capture.write('\n'.join(json.dumps(row) for row in rows))
            capture.flush()
            converted = list(gaze.convert_messages(gaze.recorded_gaze_messages(capture.name)))
        self.assertEqual(len(converted), 1)
        received, frame = converted[0]
        summary = gaze.frame_summary(frame)
        self.assertEqual(received, 1.5)
        self.assertEqual(summary['gaze_point']['xy'], list(frame.device.combined_point))
        self.assertTrue(summary['gaze_point']['valid'])


if __name__ == '__main__':
    unittest.main()
