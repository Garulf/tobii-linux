"""Small Linux libusb backend. No vendor protocol or firmware commands.

ABI source: libusb 1.0 public descriptor and synchronous I/O documentation.
The backend never detaches kernel drivers or changes device configuration.
"""
from __future__ import annotations

import ctypes as C
import ctypes.util
import os
from contextlib import contextmanager
from dataclasses import asdict, dataclass

VID, PID = 0x2104, 0x0313
MAX_TRANSFER = 1024 * 1024
P = C.POINTER
U8, U16 = C.c_uint8, C.c_uint16


class DeviceDescriptor(C.Structure):
    _fields_ = [(n, t) for n, t in [
        ('bLength', U8), ('bDescriptorType', U8), ('bcdUSB', U16),
        ('bDeviceClass', U8), ('bDeviceSubClass', U8), ('bDeviceProtocol', U8),
        ('bMaxPacketSize0', U8), ('idVendor', U16), ('idProduct', U16),
        ('bcdDevice', U16), ('iManufacturer', U8), ('iProduct', U8),
        ('iSerialNumber', U8), ('bNumConfigurations', U8)]]


class EndpointDescriptor(C.Structure):
    _fields_ = [('bLength', U8), ('bDescriptorType', U8), ('bEndpointAddress', U8),
                ('bmAttributes', U8), ('wMaxPacketSize', U16), ('bInterval', U8),
                ('bRefresh', U8), ('bSynchAddress', U8), ('extra', P(U8)),
                ('extra_length', C.c_int)]


class InterfaceDescriptor(C.Structure):
    _fields_ = [(n, U8) for n in ('bLength', 'bDescriptorType', 'bInterfaceNumber',
        'bAlternateSetting', 'bNumEndpoints', 'bInterfaceClass',
        'bInterfaceSubClass', 'bInterfaceProtocol', 'iInterface')] + [
        ('endpoint', P(EndpointDescriptor)), ('extra', P(U8)), ('extra_length', C.c_int)]


class Interface(C.Structure):
    _fields_ = [('altsetting', P(InterfaceDescriptor)), ('num_altsetting', C.c_int)]


class ConfigDescriptor(C.Structure):
    _fields_ = [('bLength', U8), ('bDescriptorType', U8), ('wTotalLength', U16),
                ('bNumInterfaces', U8), ('bConfigurationValue', U8),
                ('iConfiguration', U8), ('bmAttributes', U8), ('MaxPower', U8),
                ('interface', P(Interface)), ('extra', P(U8)), ('extra_length', C.c_int)]


class USBError(RuntimeError):
    def __init__(self, operation, code, transferred=0, partial_data=b''):
        self.code, self.transferred, self.partial_data = code, transferred, partial_data
        super().__init__(f'{operation}: libusb error {code} ({transferred} bytes transferred)')


def check(code, operation):
    if code < 0:
        raise USBError(operation, code)
    return code


@dataclass(frozen=True)
class Endpoint:
    address: int
    attributes: int
    max_packet_size: int
    interval: int


@dataclass(frozen=True)
class Alternate:
    interface: int
    alternate: int
    interface_class: int
    endpoints: tuple[Endpoint, ...]


def describe_config(config):
    alternatives = []
    for i in range(config.bNumInterfaces):
        interface = config.interface[i]
        for j in range(interface.num_altsetting):
            alt = interface.altsetting[j]
            alternatives.append(Alternate(alt.bInterfaceNumber, alt.bAlternateSetting,
                alt.bInterfaceClass, tuple(Endpoint(ep.bEndpointAddress, ep.bmAttributes,
                ep.wMaxPacketSize, ep.bInterval) for ep in
                (alt.endpoint[k] for k in range(alt.bNumEndpoints)))))
    return alternatives


def bulk_pair(alternatives):
    """Require descriptor-confirmed streaming pipes from the researched runtime.

    Other pipes (including the firmware download pipe 0x04) remain unused.
    This mapping has static/emulated evidence, but no USB capture validation.
    """
    matches = [alt for alt in alternatives if alt.interface == 0]
    if len(matches) != 1 or matches[0].alternate != 0:
        raise ValueError('Expected exactly one interface 0 / alternate 0 descriptor')
    endpoints = [ep for ep in matches[0].endpoints if ep.attributes & 3 == 2]
    incoming = [ep for ep in endpoints if ep.address == 0x83]
    outgoing = [ep for ep in endpoints if ep.address == 0x05]
    if len(incoming) != 1 or len(outgoing) != 1:
        raise ValueError('Interface 0 requires exactly one bulk IN 0x83 and OUT 0x05')
    return incoming[0], outgoing[0]


def load_library():
    path = os.environ.get('TOBII_LIBUSB_PATH') or ctypes.util.find_library('usb-1.0')
    if not path:
        raise RuntimeError('libusb-1.0 not found; set TOBII_LIBUSB_PATH to its shared library')
    lib = C.CDLL(path)
    signatures = {
        'init': ([P(C.c_void_p)], C.c_int), 'exit': ([C.c_void_p], None),
        'get_device_list': ([C.c_void_p, P(P(C.c_void_p))], C.c_ssize_t),
        'free_device_list': ([P(C.c_void_p), C.c_int], None),
        'get_device_descriptor': ([C.c_void_p, P(DeviceDescriptor)], C.c_int),
        'get_bus_number': ([C.c_void_p], U8), 'get_device_address': ([C.c_void_p], U8),
        'get_config_descriptor': ([C.c_void_p, U8, P(P(ConfigDescriptor))], C.c_int),
        'get_active_config_descriptor': ([C.c_void_p, P(P(ConfigDescriptor))], C.c_int),
        'free_config_descriptor': ([P(ConfigDescriptor)], None),
        'open': ([C.c_void_p, P(C.c_void_p)], C.c_int), 'close': ([C.c_void_p], None),
        'claim_interface': ([C.c_void_p, C.c_int], C.c_int),
        'release_interface': ([C.c_void_p, C.c_int], C.c_int),
        'bulk_transfer': ([C.c_void_p, U8, P(U8), C.c_int, P(C.c_int), C.c_uint], C.c_int),
        'control_transfer': ([C.c_void_p, U8, U8, U16, U16, P(U8), U16, C.c_uint], C.c_int),
    }
    for name, (args, result) in signatures.items():
        func = getattr(lib, 'libusb_' + name)
        func.argtypes, func.restype = args, result
    return lib


class USBContext:
    def __init__(self, lib=None):
        self.lib = lib if lib is not None else load_library()
        self.ctx = C.c_void_p()
        self._open = False
        self._transports = []

    def __enter__(self):
        if self._open:
            raise RuntimeError('USB context already open')
        check(self.lib.libusb_init(C.byref(self.ctx)), 'init')
        self._open = True
        return self

    def __exit__(self, *exc):
        try:
            for transport in self._transports[:]:
                transport.close()
        finally:
            if self._open:
                self.lib.libusb_exit(self.ctx)
                self._open = False

    def _require_open(self):
        if not self._open:
            raise RuntimeError('USB context is closed')

    @contextmanager
    def _devices(self):
        self._require_open()
        devices = P(C.c_void_p)()
        count = check(self.lib.libusb_get_device_list(self.ctx, C.byref(devices)), 'enumerate')
        try:
            yield devices, count
        finally:
            self.lib.libusb_free_device_list(devices, 1)

    def _descriptor(self, device):
        descriptor = DeviceDescriptor()
        check(self.lib.libusb_get_device_descriptor(device, C.byref(descriptor)), 'device descriptor')
        return descriptor

    def _config(self, device, index=None):
        config = P(ConfigDescriptor)()
        if index is None:
            result = self.lib.libusb_get_active_config_descriptor(device, C.byref(config))
        else:
            result = self.lib.libusb_get_config_descriptor(device, index, C.byref(config))
        check(result, 'configuration descriptor')
        try:
            return config.contents.bConfigurationValue, describe_config(config.contents)
        finally:
            self.lib.libusb_free_config_descriptor(config)

    def probe(self):
        """Read standard descriptors only, without opening/claiming a device."""
        found = []
        with self._devices() as (devices, count):
            for i in range(count):
                device = devices[i]
                descriptor = self._descriptor(device)
                if (descriptor.idVendor, descriptor.idProduct) != (VID, PID):
                    continue
                item = {'bus': self.lib.libusb_get_bus_number(device),
                        'address': self.lib.libusb_get_device_address(device),
                        'device': {n: getattr(descriptor, n) for n, _ in descriptor._fields_},
                        'configurations': []}
                for index in range(descriptor.bNumConfigurations):
                    value, alts = self._config(device, index)
                    item['configurations'].append({'value': value,
                        'interfaces': [asdict(alt) for alt in alts]})
                found.append(item)
        return found

    def open_transport(self, *, bus, address):
        """Explicit future-protocol operation; claims only interface zero.

        Bus/address selects a current device, not a persistent identity. This
        operation does not change configuration, detach drivers, or send data.
        """
        with self._devices() as (devices, count):
            for i in range(count):
                device = devices[i]
                if (self.lib.libusb_get_bus_number(device),
                    self.lib.libusb_get_device_address(device)) != (bus, address):
                    continue
                descriptor = self._descriptor(device)
                if (descriptor.idVendor, descriptor.idProduct) != (VID, PID):
                    raise ValueError('Selected device is not Eye Tracker 5')
                _, alts = self._config(device)
                incoming, outgoing = bulk_pair(alts)
                handle = C.c_void_p()
                check(self.lib.libusb_open(device, C.byref(handle)), 'open')
                try:
                    check(self.lib.libusb_claim_interface(handle, 0), 'claim interface 0')
                except BaseException:
                    self.lib.libusb_close(handle)
                    raise
                transport = BulkTransport(self, handle, incoming, outgoing)
                self._transports.append(transport)
                return transport
        raise ValueError('Selected Eye Tracker 5 is not connected')


class BulkTransport:
    def __init__(self, context, handle, incoming, outgoing):
        self.context, self.handle = context, handle
        self.incoming, self.outgoing = incoming, outgoing
        self.closed = False

    def __enter__(self):
        if self.closed:
            raise RuntimeError('USB transport is closed')
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        if not self.closed:
            self.closed = True
            try:
                # A disconnected device may no longer allow release; close regardless.
                self.context.lib.libusb_release_interface(self.handle, 0)
            finally:
                self.context.lib.libusb_close(self.handle)
                if self in self.context._transports:
                    self.context._transports.remove(self)

    def _transfer(self, endpoint, buffer, timeout_ms):
        self.context._require_open()
        if self.closed:
            raise RuntimeError('USB transport is closed')
        if not 1 <= timeout_ms <= 60000:
            raise ValueError('timeout_ms must be between 1 and 60000')
        if not 1 <= len(buffer) <= MAX_TRANSFER:
            raise ValueError(f'transfer size must be between 1 and {MAX_TRANSFER}')
        transferred = C.c_int()
        result = self.context.lib.libusb_bulk_transfer(self.handle, endpoint.address,
            buffer, len(buffer), C.byref(transferred), timeout_ms)
        size = transferred.value
        if not 0 <= size <= len(buffer):
            raise RuntimeError('libusb returned invalid transfer length')
        if result < 0:
            raise USBError('bulk transfer', result, size,
                           bytes(buffer[:size]) if endpoint.address & 0x80 else b'')
        return size

    def read(self, size, *, timeout_ms=1000):
        if not 1 <= size <= MAX_TRANSFER:
            raise ValueError('Invalid read size')
        # Packet multiples prevent libusb overflow if a final packet exceeds
        # the remaining buffer. Callers retain short reads for framing.
        if not self.incoming.max_packet_size or size % self.incoming.max_packet_size:
            raise ValueError('Read size must be a multiple of endpoint max packet size')
        buffer = (U8 * size)()
        count = self._transfer(self.incoming, buffer, timeout_ms)
        return bytes(buffer[:count])

    def write(self, data, *, timeout_ms=1000):
        if not 1 <= len(data) <= MAX_TRANSFER:
            raise ValueError('Invalid write size')
        buffer = (U8 * len(data)).from_buffer_copy(data)
        # Return short-write count; never retry silently (framing is unknown).
        return self._transfer(self.outgoing, buffer, timeout_ms)


    def _control(self, request_type, request, value, index, buffer, timeout_ms):
        self.context._require_open()
        if self.closed:
            raise RuntimeError('USB transport is closed')
        if not 1 <= timeout_ms <= 60000:
            raise ValueError('timeout_ms must be between 1 and 60000')
        if not 0 <= request <= 255 or not 0 <= value <= 65535 or not 0 <= index <= 65535:
            raise ValueError('Control setup fields out of range')
        if len(buffer) > 4096:
            raise ValueError('Control transfer exceeds local 4096-byte limit')
        # libusb control timeout does not expose a partial-transfer count.
        result = self.context.lib.libusb_control_transfer(self.handle, request_type,
            request, value, index, buffer, len(buffer), timeout_ms)
        check(result, 'control transfer')
        if result > len(buffer):
            raise RuntimeError('libusb returned invalid control transfer length')
        return result

    def control_in(self, request, *, value=0, index=0, length, timeout_ms=1000):
        """Explicit vendor/interface control IN; no automatic initialization."""
        if not 0 <= length <= 4096:
            raise ValueError('Control length must be between 0 and 4096')
        buffer = (U8 * length)()
        count = self._control(0xC1, request, value, index, buffer, timeout_ms)
        return bytes(buffer[:count])

    def control_out(self, request, *, value=0, index=0, data=b'', timeout_ms=1000):
        """Explicit vendor/interface control OUT; returns actual byte count."""
        if len(data) > 4096:
            raise ValueError('Control payload exceeds local 4096-byte limit')
        buffer = (U8 * len(data)).from_buffer_copy(data)
        return self._control(0x41, request, value, index, buffer, timeout_ms)
