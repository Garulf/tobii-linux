import unittest

from tobii_linux import ttp_commands as tc
from tobii_linux.ttp import Decoder, ProtocolError, fields, hardware_info_request
from tobii_linux.ttp_commands import Command, Stream

# Bytes produced by the original builders in emulation
# (tools/ttp_commands_oracle.py), NOT captured USB traffic.
ORIGINAL = {
    'device_info': '0000005100000001000000000000058c0000000000000000',
    'subscribe_image': '000000510000000100000000000004c400000000000000140000020000000400000501'
                       '170000000400000000',
    'subscribe_columns': '000000510000000900000000000004c40000000000000024000002000000040000050117'
                         '000000140000000400000001000000020000000510203040',
    'unsubscribe_primary': '000000510000000100000000000004ce000000000000000b000002000000040000050e',
    'columns_algo_debug': '000000510000000100000000000004ba000000000000000b0000020000000400001770',
    'open_realm_salt': '0000005100000001000000000000076c00000000000000180000020000000400002712'
                       '17000000080000000100000000',
    'close_realm': '0000005100000001000000000000077b000000000000000b0000020000000400002712',
    'realm_response': '00000051000000050000000000000776000000000000002d00000200000004000000070200'
                      '00000412345678150000001400000010101112131415161718191a1b1c1d1e1f',
    'realm_response_data': '00000051000000060000000000000777000000000000003a000002000000040000000702'
                           '0000000412345678150000001400000010101112131415161718191a1b1c1d1e1f'
                           '15000000080000000401020304',
    'pause': '00000051000000010000000000000c1c000000000000000b0000020000000400000001',
    'resume': '00000051000000010000000000000c1c000000000000000b0000020000000400000000',
    'illumination': '000000510000000300000000000007e400000000000000120000140000000b00000007'
                    '64656661756c74',
    'output_frequency': '00000051000000040000000000000654000000000000000b000003000000040021553f',
    'frequencies': '0000005100000008000000000000067c000000000000001d0000050000000400022af8020000'
                   '000400000001020000000400000002',
}
DIGEST = bytes(range(0x10, 0x20))


class TtpCommandTests(unittest.TestCase):
    def check(self, name, message):
        self.assertEqual(message.encode().hex(), ORIGINAL[name])

    def test_emulated_builder_vectors(self):
        self.check('device_info', tc.header_only_request(1, Command.GET_DEVICE_INFO))
        self.check('subscribe_image', tc.subscribe_stream(1, Stream.IMAGE))
        self.check('subscribe_columns', tc.subscribe_stream(9, Stream.IMAGE, (1, 2, 5, 0x10203040)))
        self.check('unsubscribe_primary', tc.unsubscribe_stream(1, Stream.PRIMARY_CAMERA_IMAGE))
        self.check('columns_algo_debug', tc.enumerate_stream_type_columns(1, Stream.ALGO_DEBUG))
        self.check('open_realm_salt', tc.open_realm_salt(1, 0x2712))
        self.check('close_realm', tc.close_realm(1, 0x2712))
        self.check('realm_response', tc.open_realm_response(5, (7, 0x12345678), DIGEST))
        self.check('realm_response_data',
                   tc.open_realm_response(6, (7, 0x12345678), DIGEST, b'\1\2\3\4'))
        self.check('pause', tc.pause_device(1))
        self.check('resume', tc.resume_device(1))
        self.check('illumination', tc.set_illumination_mode(3, 'default'))
        self.check('output_frequency', tc.set_output_frequency(4, 33.333))
        self.check('frequencies', tc.set_frequencies(8, 1, 2))

    def test_header_only_matches_existing_hardware_info_request(self):
        self.assertEqual(tc.header_only_request(7, Command.HARDWARE_CONFIGURATION_GET),
                         hardware_info_request(7))

    def test_requests_decode_with_inner_framing(self):
        message = tc.open_realm_response(6, (1, 2), DIGEST, b'xyz')
        decoded, = Decoder().feed(message.encode())
        self.assertEqual(decoded, message)
        self.assertEqual([field.tag for field in fields(decoded.payload)],
                         [tc.FIELD_UINT32, tc.FIELD_UINT32, tc.FIELD_BLOB, tc.FIELD_BLOB])

    def test_wire_stream_table(self):
        self.assertEqual(tc.wire_stream_id(Stream.IMAGE), 0x501)
        self.assertEqual(tc.wire_stream_id(Stream.PRIMARY_CAMERA_IMAGE), 0x50E)
        self.assertEqual(len(set(tc.WIRE_STREAM_IDS.values())), 21)
        with self.assertRaises(ProtocolError):
            tc.wire_stream_id(22)

    def test_fixed_point_truncates_like_cvttss2si(self):
        self.assertEqual(tc.fixed16_field(-1.5).data.hex(), 'fffe8000')
        self.assertEqual(tc.fixed16_field(40000.0).data.hex(), '80000000')
        self.assertEqual(tc.fixed16_field(float('nan')).data.hex(), '80000000')

    def test_realm_digest_is_hmac_md5(self):
        self.assertEqual(tc.realm_digest(bytes(range(16)), bytes(range(100, 116))).hex(),
                         '0cead0b99c4ccb9a30e2c5b8fbd55dfa')

    def test_rejections(self):
        with self.assertRaises(ProtocolError):
            tc.header_only_request(1, Command.SUBSCRIBE_STREAM)
        with self.assertRaises(ProtocolError):
            tc.open_realm_response(1, (0, 0), b'short')
        with self.assertRaises(ProtocolError):
            tc.set_illumination_mode(1, 'a\0b')
        with self.assertRaises(ProtocolError):
            tc.struct_header(0x1000, 1)
        with self.assertRaises(ProtocolError):
            tc.int32_field(2**31)


if __name__ == '__main__':
    unittest.main()
