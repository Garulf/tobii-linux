import ctypes as C
import unittest

from tobii_linux.usb import (Alternate, Endpoint, USBContext,
                             USBError, bulk_pair,
                             ConfigDescriptor, Interface, InterfaceDescriptor,
                             EndpointDescriptor, DeviceDescriptor)

IN = Endpoint(0x83, 2, 64, 0)
OUT = Endpoint(0x05, 2, 64, 0)


class FakeUSB:
    def __init__(self):
        self.events = []
        self.result = 0
        self.count = 3
        self.claim_result = 0
        self.descriptor_result = 0
        self.devices = (C.c_void_p * 1)(123)
        self.endpoints = (EndpointDescriptor * 2)()
        for ep, data in zip(self.endpoints, (IN, OUT)):
            ep.bEndpointAddress, ep.bmAttributes = data.address, data.attributes
            ep.wMaxPacketSize = data.max_packet_size
        self.alt = InterfaceDescriptor()
        self.alt.bNumEndpoints = 2
        self.alt.endpoint = self.endpoints
        self.interfaces = (Interface * 1)()
        self.interfaces[0].num_altsetting = 1
        self.interfaces[0].altsetting = C.pointer(self.alt)
        self.config = ConfigDescriptor()
        self.config.bNumInterfaces = 1
        self.config.bConfigurationValue = 1
        self.config.interface = self.interfaces

    def libusb_init(self, pointer):
        C.cast(pointer, C.POINTER(C.c_void_p))[0] = 7
        self.events.append('init')
        return 0

    def libusb_exit(self, ctx):
        self.events.append('exit')

    def libusb_get_device_list(self, ctx, pointer):
        C.cast(pointer, C.POINTER(C.POINTER(C.c_void_p)))[0] = self.devices
        return 1

    def libusb_free_device_list(self, devices, unref):
        assert unref == 1
        self.events.append('free-list')

    def libusb_get_device_descriptor(self, device, pointer):
        descriptor = C.cast(pointer, C.POINTER(DeviceDescriptor)).contents
        descriptor.idVendor, descriptor.idProduct = 0x2104, 0x0313
        descriptor.bNumConfigurations = 1
        return self.descriptor_result

    def libusb_get_bus_number(self, device):
        return 1

    def libusb_get_device_address(self, device):
        return 2

    def libusb_get_active_config_descriptor(self, device, pointer):
        C.cast(pointer, C.POINTER(C.POINTER(ConfigDescriptor)))[0] = C.pointer(self.config)
        return 0

    def libusb_get_config_descriptor(self, device, index, pointer):
        return self.libusb_get_active_config_descriptor(device, pointer)

    def libusb_free_config_descriptor(self, descriptor):
        self.events.append('free-config')

    def libusb_open(self, device, pointer):
        C.cast(pointer, C.POINTER(C.c_void_p))[0] = 8
        self.events.append('open')
        return 0

    def libusb_claim_interface(self, handle, interface):
        assert interface == 0
        self.events.append('claim')
        return self.claim_result

    def libusb_release_interface(self, handle, interface):
        assert interface == 0
        self.events.append('release')
        return -4  # Disconnect must not prevent close.

    def libusb_close(self, handle):
        self.events.append('close')

    def libusb_control_transfer(self, handle, kind, request, value, index, buffer, length, timeout):
        self.events.append(('control', kind, request, value, index, length, timeout))
        if self.result < 0:
            return self.result
        count = min(self.count, length)
        if kind & 0x80:
            buffer[:count] = b'abc'[:count]
        return count

    def libusb_bulk_transfer(self, handle, endpoint, buffer, length, transferred, timeout):
        self.events.append(('transfer', endpoint, timeout))
        C.cast(transferred, C.POINTER(C.c_int))[0] = self.count
        if endpoint & 0x80:
            buffer[:3] = b'abc'
        return self.result


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

    def test_probe_does_not_open_and_frees_descriptors(self):
        lib = FakeUSB()
        with USBContext(lib) as ctx:
            result = ctx.probe()
        self.assertEqual(result[0]['configurations'][0]['interfaces'][0]['endpoints'][0]['address'], 0x83)
        self.assertEqual(lib.events, ['init', 'free-config', 'free-list', 'exit'])

    def test_claim_failure_closes_handle_and_list(self):
        lib = FakeUSB()
        lib.claim_result = -6
        with USBContext(lib) as ctx:
            with self.assertRaises(USBError):
                ctx.open_transport(bus=1, address=2)
        self.assertEqual(lib.events[-3:], ['close', 'free-list', 'exit'])
        self.assertNotIn('release', lib.events)

    def test_descriptor_failure_frees_list(self):
        lib = FakeUSB()
        lib.descriptor_result = -4
        with self.assertRaises(USBError):
            with USBContext(lib) as ctx:
                ctx.probe()
        self.assertEqual(lib.events, ['init', 'free-list', 'exit'])

    def test_context_owns_transport_and_close_is_idempotent(self):
        lib = FakeUSB()
        with USBContext(lib) as ctx:
            transport = ctx.open_transport(bus=1, address=2)
        transport.close()
        self.assertEqual(lib.events[-3:], ['release', 'close', 'exit'])
        self.assertEqual(lib.events.count('close'), 1)
        with self.assertRaises(RuntimeError):
            transport.read(64)

    def test_short_read_write_and_timeout_retain_partial(self):
        lib = FakeUSB()
        with USBContext(lib) as ctx:
            with ctx.open_transport(bus=1, address=2) as transport:
                self.assertEqual(transport.read(64), b'abc')
                self.assertEqual(transport.write(b'abcdef'), 3)
                lib.result = -7
                with self.assertRaises(USBError) as error:
                    transport.read(64, timeout_ms=20)
                self.assertEqual(error.exception.partial_data, b'abc')
                self.assertEqual(error.exception.transferred, 3)
                with self.assertRaises(USBError) as error:
                    transport.write(b'abcdef')
                self.assertEqual(error.exception.transferred, 3)
                self.assertEqual(error.exception.partial_data, b'')

    def test_control_fields_short_read_zero_length_and_timeout(self):
        lib = FakeUSB()
        with USBContext(lib) as ctx:
            transport = ctx.open_transport(bus=1, address=2)
            self.assertEqual(transport.control_in(0x46, value=1, length=8), b'abc')
            self.assertEqual(transport.control_out(0x41, value=7), 0)
            self.assertEqual(lib.events[-1], ('control', 0x41, 0x41, 7, 0, 0, 1000))
            lib.result = -7
            with self.assertRaises(USBError) as error:
                transport.control_out(0x42, value=7)
            self.assertEqual(error.exception.code, -7)
            for kwargs in ({'value': -1}, {'index': 65536}, {'timeout_ms': 0}):
                with self.assertRaises(ValueError):
                    transport.control_out(0x41, **kwargs)

    def test_invalid_bounds_never_call_transfer(self):
        lib = FakeUSB()
        with USBContext(lib) as ctx:
            transport = ctx.open_transport(bus=1, address=2)
            for size in (0, -1, 63, 2**31):
                with self.assertRaises(ValueError):
                    transport.read(size)
            for timeout in (0, -1, 60001):
                with self.assertRaises(ValueError):
                    transport.read(64, timeout_ms=timeout)
            with self.assertRaises(ValueError):
                transport.write(b'')
        self.assertFalse(any(isinstance(event, tuple) for event in lib.events))


if __name__ == '__main__':
    unittest.main()
