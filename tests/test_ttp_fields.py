import struct
import unittest

from tobii_linux.ttp import Field, Message, ProtocolError
from tobii_linux.ttp_fields import (
    RESPONSE_FIELD_TYPES, Reader, StructHeader, TRIPLE_FIXED42, Wire, array_fields,
    encode_payload, fixed16_field, fixed42_array_field, fixed42_field, int32_field,
    response_status, status_name, struct_field, triple_fixed42_fields, uint32_field, value)


def item(tag, data):
    return Field(tag, bytes.fromhex(data))


# Synthetic items built from the reader and writer instructions, NOT device traffic.
class ValueTests(unittest.TestCase):
    def test_scalars(self):
        self.assertEqual(value(item(Wire.INT32, "fffffffe")), -2)
        self.assertEqual(value(item(Wire.UINT32, "fffffffe")), 0xFFFFFFFE)
        self.assertEqual(value(item(Wire.FIXED16, "00018000")), 1.5)
        self.assertEqual(value(item(Wire.FIXED16, "ffffc000")), -0.25)
        self.assertEqual(value(item(Wire.FIXED42, "0000040000000000")), 1.0)
        self.assertEqual(value(item(Wire.FIXED42, "ffffffffffffffff")), -2.0 ** -42)
        self.assertEqual(value(item(Wire.INT64, "fffffffffffffffd")), -3)
        self.assertEqual(value(item(Wire.UINT64, "fffffffffffffffd")), 2**64 - 3)
        self.assertEqual(value(item(Wire.STRUCT, "001a332c")), StructHeader(0x1A, 0x332C))

    def test_fixed16_is_rounded_through_float32(self):
        self.assertEqual(value(item(Wire.FIXED16, "01000001")), 256.0)

    def test_scalar_lengths_are_exact(self):
        for tag, size in ((Wire.INT32, 8), (Wire.FIXED16, 2), (Wire.FIXED42, 4), (Wire.UINT64, 4)):
            with self.assertRaises(ProtocolError):
                value(Field(tag, bytes(size)))

    def test_counted_values(self):
        self.assertEqual(value(item(Wire.STRING, "00000003616263")), b"abc")
        self.assertEqual(value(item(Wire.BLOB, "0000000201ff")), b"\x01\xff")
        self.assertEqual(value(item(Wire.UINT32_ARRAY, "000000020000000100000002")), (1, 2))
        self.assertEqual(value(item(Wire.FIXED16_ARRAY, "0000000100010000")), (1.0,))
        self.assertEqual(value(item(Wire.FIXED42_ARRAY, "00000001fffffc0000000000")), (-1.0,))
        self.assertEqual(value(item(Wire.STRING_ARRAY, "00000002000000016100000000")), (b"a", b""))

    def test_counted_extents_must_agree(self):
        for tag, data in ((Wire.STRING, "0000000461"), (Wire.FIXED42_ARRAY, "0000000200"),
                          (Wire.STRING_ARRAY, "000000010000000561"), (Wire.STRING_ARRAY, "00000000ff"),
                          (Wire.BLOB, "000000")):
            with self.assertRaises(ProtocolError):
                value(item(tag, data))

    def test_unknown_wire_type_is_refused(self):
        with self.assertRaises(ProtocolError):
            value(Field(0x08, bytes(4)))


class EncoderTests(unittest.TestCase):
    def test_scalar_encoders(self):
        self.assertEqual(int32_field(-2).encode().hex(), "0100000004fffffffe")
        self.assertEqual(uint32_field(7).encode().hex(), "020000000400000007")
        self.assertEqual(fixed16_field(1.5).encode().hex(), "030000000400018000")
        self.assertEqual(fixed42_field(-1.0).encode().hex(), "0400000008fffffc0000000000")
        self.assertEqual(struct_field(3, 0x1F41).encode().hex(), "050000000400031f41")

    def test_fixed_point_truncates_toward_zero(self):
        self.assertEqual(fixed16_field(-1.0000001).encode()[-4:], struct.pack(">i", -65536))
        self.assertEqual(fixed42_field(2.0 ** -43).encode()[-8:], bytes(8))

    def test_fixed_point_range(self):
        for encoder, number in ((fixed16_field, 32768.0), (fixed42_field, 2.0 ** 21)):
            with self.assertRaises(ProtocolError):
                encoder(number)

    def test_array_header(self):
        encoded = b"".join(f.encode() for f in array_fields(2, 7))
        self.assertEqual(encoded.hex(), "050000000400030100" "020000000400000007")
        with self.assertRaises(ProtocolError):
            array_fields(0xFFF)

    def test_fixed42_array(self):
        self.assertEqual(fixed42_array_field((0.5,)).encode().hex(),
                         "190000000c000000010000020000000000")


class ReaderTests(unittest.TestCase):
    def payload(self, *items):
        return encode_payload(items)

    def test_sequential_reads(self):
        reader = Reader(self.payload(*array_fields(1), *triple_fixed42_fields((1.0, -2.0, 0.5)),
                                     int32_field(9), uint32_field(1)))
        self.assertEqual(reader.array(), 1)
        self.assertEqual(reader.triple_fixed42(), (1.0, -2.0, 0.5))
        self.assertEqual(reader.int32(), 9)
        self.assertEqual(reader.remaining, 9)

    def test_wire_type_mismatch(self):
        with self.assertRaises(ProtocolError):
            Reader(self.payload(uint32_field(1))).int32()

    def test_structure_header_must_match_exactly(self):
        reader = Reader(self.payload(struct_field(4, TRIPLE_FIXED42 & 0xFFFF)))
        with self.assertRaises(ProtocolError):
            reader.triple_fixed42()

    def test_array_header_checks_identifier_and_masks_count(self):
        with self.assertRaises(ProtocolError):
            Reader(self.payload(struct_field(2, 0x101), uint32_field(0))).array()
        with self.assertRaises(ProtocolError):
            Reader(self.payload(struct_field(0, 0x100), uint32_field(0))).array()
        self.assertEqual(Reader(self.payload(struct_field(0xF003, 0x100), uint32_field(0))).array(), 2)

    def test_fixed42_array_limit(self):
        reader = Reader(self.payload(fixed42_array_field([1.0] * 5), int32_field(3)))
        self.assertEqual(reader.fixed42_array(2), (1.0, 1.0))
        self.assertEqual(reader.int32(), 3)

    def test_prefix_and_truncation(self):
        with self.assertRaises(ProtocolError):
            Reader(b"\0\1" + int32_field(1).encode())
        for cut in range(3, len(self.payload(int32_field(1)))):
            with self.assertRaises(ProtocolError):
                Reader(self.payload(int32_field(1))[:cut]).int32()


class ResponseTests(unittest.TestCase):
    def test_status_mapping(self):
        self.assertEqual(response_status(Message(0x52, 1, 0, 0x848, 0)), 0)
        self.assertEqual(response_status(Message(0x52, 1, 0, 0x848, 0x20000503)), 12)
        self.assertEqual(status_name(12), "PROTOCOL_INVALID_PAYLOAD")
        self.assertEqual(response_status(Message(0x52, 1, 0, 0x848, 0x2000050A)), 2)
        self.assertEqual(status_name(2), "UNKNOWN")

    def test_hardware_info_schema(self):
        self.assertEqual(RESPONSE_FIELD_TYPES[0x848], (0x1B, 0x0C, 0x02))
        self.assertEqual(len(RESPONSE_FIELD_TYPES), 48)


if __name__ == "__main__":
    unittest.main()
