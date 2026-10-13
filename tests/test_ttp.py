import struct
import unittest

from tobii_linux.ttp import (Decoder, Field, Message, ProtocolError, fields,
                            protocol_upgrade, read_uint32_array)


# Static reconstruction from runtime instructions, NOT a captured USB fixture.
UPGRADE = bytes.fromhex(
    "00000051 00000001 00000000 000003e8 00000000 0000002f "
    "0000 17 00000028 00000009 "
    "00010000 00010001 00010002 00010003 00010004 "
    "00010005 00010006 00010007 00010008")


class TtpTests(unittest.TestCase):
    def test_observed_upgrade_encoding(self):
        self.assertEqual(protocol_upgrade(1).encode(), UPGRADE)

    def test_every_split_point(self):
        for split in range(len(UPGRADE) + 1):
            decoder = Decoder()
            messages = decoder.feed(UPGRADE[:split]) + decoder.feed(UPGRADE[split:])
            decoder.finish()
            self.assertEqual(messages, [protocol_upgrade(1)])

    def test_bytewise_and_coalesced_messages(self):
        empty = Message(0x52, 2, 3, 4, 5)
        data = UPGRADE + empty.encode() + UPGRADE
        decoder = Decoder()
        messages = []
        for byte in data:
            messages += decoder.feed(bytes([byte]))
        decoder.finish()
        self.assertEqual(messages, Decoder().feed(data))
        self.assertEqual(messages, [protocol_upgrade(1), empty, protocol_upgrade(1)])

    def test_eof_every_truncation(self):
        for length in range(1, len(UPGRADE)):
            decoder = Decoder()
            decoder.feed(UPGRADE[:length])
            with self.assertRaises(ProtocolError):
                decoder.finish()

    def test_oversize_header_poisoning(self):
        decoder = Decoder(max_payload=128)
        with self.assertRaises(ProtocolError):
            decoder.feed(struct.pack(">6I", 0x51, 1, 0, 1000, 0, 0xFFFFFFFF))
        self.assertEqual(len(decoder.pending), 0)
        with self.assertRaises(ProtocolError):
            decoder.feed(UPGRADE)

    def test_unknown_fields_preserved(self):
        value = Field(0xFE, b"opaque")
        self.assertEqual(fields(b"\0\0" + value.encode()), [value])
        array, = fields(protocol_upgrade(1).payload)
        self.assertEqual(read_uint32_array(array), tuple(range(0x10000, 0x10009)))

    def test_malformed_payloads(self):
        for payload in (b"\0", b"\0\1", b"\0\0\x17", b"\0\0\x17\0\0\0\x05abc"):
            with self.assertRaises(ProtocolError):
                fields(payload)
        with self.assertRaises(ProtocolError):
            read_uint32_array(Field(0x17, b"\xff\xff\xff\xff"))
        with self.assertRaises(ProtocolError):
            protocol_upgrade(-1)


if __name__ == "__main__":
    unittest.main()
