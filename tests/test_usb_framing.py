import struct
import unittest
from tobii_linux.usb_framing import IncomingFrames, outgoing_frames
from tobii_linux.usb_channel import USBChannel, ShortWriteError


def inbound(mask, payload):
    return struct.pack('<II', mask, len(payload) + 8) + payload


class FramingTests(unittest.TestCase):
    def test_original_emulated_tx_vectors(self):
        self.assertEqual(list(outgoing_frames(7, b'x')), [bytes.fromhex('0700000001000000') + b'x'])
        frames = list(outgoing_frames(3, b'x' * 512))
        self.assertEqual([len(f) for f in frames], [519, 9])
        self.assertEqual([struct.unpack('<II', f[:8]) for f in frames], [(3, 511), (3, 1)])
        frames = list(outgoing_frames(11, b'x' * 8192))
        self.assertEqual([len(f) for f in frames], [4095, 4095, 26])
        self.assertEqual(b''.join(f[8:] for f in frames), b'x' * 8192)
        self.assertEqual(list(outgoing_frames(0, b'')), [])

    def test_every_two_way_fragmentation_and_coalesced_frames(self):
        wire = inbound(0x805, b'hello') + inbound(2, b'world') + inbound(0, b'')
        for split in range(len(wire) + 1):
            parser = IncomingFrames()
            frames = parser.feed(wire[:split]) + parser.feed(wire[split:])
            self.assertEqual([(f.recipients, f.payload) for f in frames],
                             [(0x805, b'hello'), (2, b'world'), (0, b'')])
            self.assertTrue(frames[0].for_channel(11))
            self.assertFalse(frames[0].for_channel(1))

    def test_bytewise_and_length_validation_fail_closed(self):
        parser = IncomingFrames()
        frames = []
        for byte in inbound(1, b'abc'):
            frames += parser.feed(bytes([byte]))
        self.assertEqual(frames[0].payload, b'abc')
        for length in (0, 7, 1024 * 1024 + 1, 0xffffffff):
            parser = IncomingFrames()
            with self.assertRaises(ValueError):
                parser.feed(struct.pack('<II', 1, length))
            self.assertFalse(parser.buffer)
            with self.assertRaises(ValueError):
                parser.feed(inbound(1, b'abc'))

    def test_rx_length_includes_header_unlike_tx(self):
        parser = IncomingFrames()
        # A three-byte payload in TX has length=3; RX rejects that same header.
        with self.assertRaises(ValueError):
            parser.feed(list(outgoing_frames(1, b'abc'))[0])


class FakeTransport:
    def __init__(self):
        self.controls = []
        self.writes = []
        self.short = False
        self.input = []

    def control_out(self, request, **kwargs):
        self.controls.append((request, kwargs['value']))
        return 0

    def write(self, data, **kwargs):
        self.writes.append(data)
        return 3 if self.short else len(data)

    def read(self, size, **kwargs):
        return self.input.pop(0)


class ChannelTests(unittest.TestCase):
    def test_explicit_lifecycle_and_demultiplex(self):
        transport = FakeTransport()
        transport.input = [inbound(2, b'ignore') + inbound(1, b'ab')[:9], b'b']
        channel = USBChannel(transport)
        self.assertEqual(transport.controls, [])
        with channel:
            self.assertEqual(channel.write(b'abc'), 3)
            self.assertEqual(channel.read(), b'')
            self.assertEqual(channel.read(), b'ab')
        self.assertEqual(transport.controls, [(0x41, 0), (0x42, 0)])
        channel.close()
        self.assertEqual(len(transport.controls), 2)

    def test_short_write_is_not_retried_and_close_still_happens(self):
        transport = FakeTransport()
        transport.short = True
        with USBChannel(transport, channel=7) as channel:
            with self.assertRaises(ShortWriteError):
                channel.write(b'x' * 8192)
            with self.assertRaises(RuntimeError):
                channel.write(b'retry')
        self.assertEqual(len(transport.writes), 1)
        self.assertEqual(transport.controls, [(0x41, 7), (0x42, 7)])


if __name__ == '__main__':
    unittest.main()
