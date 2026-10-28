"""The USB layer's Python surface. The transport itself (nusb) and the channel framing
are tested in crates/tobii-usb; without a tracker only descriptor logic and errors run here."""
import unittest

from tobii_linux.usb import Alternate, Endpoint, USBContext, USBError, bulk_pair

IN = Endpoint(0x83, 2, 64, 0)
OUT = Endpoint(0x05, 2, 64, 0)


class USBTests(unittest.TestCase):
    def test_selects_interface_zero_and_discovered_addresses(self):
        video = Alternate(1, 0, 14, (Endpoint(0x81, 2, 64, 0),))
        self.assertEqual(bulk_pair([video, Alternate(0, 0, 255, (IN, OUT))]), (IN, OUT))
        with self.assertRaises(ValueError):
            bulk_pair([Alternate(0, 0, 255, (IN, OUT, IN))])
        with self.assertRaises(ValueError):
            bulk_pair([Alternate(0, 0, 255, (IN, OUT)), Alternate(0, 1, 255, (IN, OUT))])

    def test_selects_streaming_pipes_from_multi_endpoint_layout(self):
        extra = (Endpoint(0x81, 3, 64, 0), Endpoint(0x04, 2, 512, 0))
        self.assertEqual(bulk_pair([Alternate(0, 0, 255, extra + (IN, OUT))]), (IN, OUT))
        for endpoints in ((IN,), (OUT,), (Endpoint(0x81, 2, 512, 0), OUT),
                          (IN, Endpoint(0x05, 3, 512, 0))):
            with self.assertRaises(ValueError):
                bulk_pair([Alternate(0, 0, 255, endpoints)])

    def test_usb_error_keeps_code_and_partial_data(self):
        error = USBError('bulk transfer', -7, 3, b'abc')
        self.assertIsInstance(error, RuntimeError)
        self.assertEqual((error.code, error.transferred, error.partial_data), (-7, 3, b'abc'))
        self.assertEqual(str(error), 'bulk transfer: libusb error -7 (3 bytes transferred)')

    def test_context_must_be_open(self):
        context = USBContext()
        with self.assertRaises(RuntimeError):
            context.probe()
        with context:
            try:
                devices = context.probe()
            except USBError as error:
                self.skipTest(f"no USB bus to enumerate: {error}")
            self.assertIsInstance(devices, list)
            with self.assertRaises(ValueError):
                context.open_transport(bus=255, address=255)
        with self.assertRaises(RuntimeError):
            context.probe()


if __name__ == '__main__':
    unittest.main()
