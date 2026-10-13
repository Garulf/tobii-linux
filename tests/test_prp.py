"""Hand-transcribed little-endian vectors from the game DLL's serializer.

These are static-code vectors, not captured live packets.
"""
import struct
import unittest

from tobii_linux.prp import (GazePoint, HeadPose, ProtocolError, decode_envelope,
                             decode_gaze_body, decode_head_body, encode_envelope,
                             encode_gaze_body, encode_head_body, encode_stream_packet)


class PrpTests(unittest.TestCase):
    def test_gaze_word_order(self):
        # Timestamp 0x1122334455667788 is high-word-first, each word LE.
        wire = bytes.fromhex("18000000 44332211 88776655 01000000 0000003f 0000803e")
        sample = GazePoint(0x1122334455667788, True, 0.5, 0.25)
        self.assertEqual(decode_gaze_body(wire), sample)
        self.assertEqual(encode_gaze_body(sample), wire)

    def test_head_independent_rotation_validity(self):
        wire = bytes.fromhex("34000000 ffffffff feffffff 01000000 "
                             "0000803f 00000040 00004040 "
                             "01000000 00000000 01000000 "
                             "000080be 0000003f 000080bf")
        sample = HeadPose(-2, True, (1.0, 2.0, 3.0), (True, False, True),
                          (-0.25, 0.5, -1.0))
        self.assertEqual(decode_head_body(wire), sample)
        self.assertEqual(encode_head_body(sample), wire)

    def test_envelope_vector_and_corruption(self):
        wire = bytes.fromhex("54505250 04000000 50505250 00000000")
        self.assertEqual(encode_envelope(bytes(4)), wire)
        self.assertEqual(decode_envelope(wire), bytes(4))
        for bad in (wire[:11], wire[:-1], wire + b"x", b"X" + wire[1:],
                    wire[:8] + bytes(4) + wire[12:]):
            with self.assertRaises(ProtocolError):
                decode_envelope(bad)
        with self.assertRaises(ProtocolError):
            decode_envelope(wire, max_payload=3)

    def test_body_validation(self):
        for decode, length in ((decode_gaze_body, 24), (decode_head_body, 52)):
            for body in (b"", bytes(length), struct.pack("<I", length) + bytes(length - 5)):
                with self.assertRaises(ProtocolError):
                    decode(body)
        bad = bytearray(encode_gaze_body(GazePoint(0, True, 0, 0)))
        bad[12] = 2
        with self.assertRaises(ProtocolError):
            decode_gaze_body(bad)
        with self.assertRaises(ProtocolError):
            encode_gaze_body(GazePoint(1 << 63, False, 0, 0))

    def test_complete_gaze_packet_from_original_encoder(self):
        # Emulated original serializer 0x18004f730, tag=42, operation=1, stream=3.
        wire = bytes.fromhex(
            "54505250 3c000000 68505250 0c000000 2a000000 01000000 "
            "04000000 08000000 03000000 04000000 18000000 44332211 "
            "88776655 01000000 0000003f 0000803e 04000000 00000000")
        sample = GazePoint(0x1122334455667788, True, .5, .25)
        self.assertEqual(encode_stream_packet(sample, message_tag=42), wire)


if __name__ == "__main__":
    unittest.main()
