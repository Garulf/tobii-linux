"""Eye Tracker 5 USB access on Linux. No vendor protocol or firmware commands.

The backend is the Rust crate tobii-usb (nusb on usbfs): probing reads the
standard descriptors from sysfs without opening the device, and a transport
claims interface 0 only. It never detaches kernel drivers or changes the device
configuration. Errors keep libusb's error codes (-7 is a timeout).
"""
from __future__ import annotations

from dataclasses import dataclass

from ._native import register_errors
from ._native import usb as _usb

VID, PID = _usb.VID, _usb.PID
MAX_TRANSFER = _usb.MAX_TRANSFER


class USBError(RuntimeError):
    def __init__(self, operation, code, transferred=0, partial_data=b''):
        self.operation = operation
        self.code, self.transferred, self.partial_data = code, transferred, partial_data
        super().__init__(f'{operation}: libusb error {code} ({transferred} bytes transferred)')


register_errors(usb_error=USBError)


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


def bulk_pair(alternatives):
    """Require descriptor-confirmed streaming pipes from the researched runtime.

    Other pipes (including the firmware download pipe 0x04) remain unused.
    """
    incoming, outgoing = _usb.bulk_pair(alternatives)
    return Endpoint(*incoming), Endpoint(*outgoing)


# Bulk IN 0x83 / OUT 0x05 of interface 0: read, write, control_in, control_out, close.
BulkTransport = _usb.BulkTransport


class USBContext:
    """Owns the transports it opens and closes them on exit."""
    def __init__(self):
        self._open = False
        self._transports = []

    def __enter__(self):
        if self._open:
            raise RuntimeError('USB context already open')
        self._open = True
        return self

    def __exit__(self, *exc):
        try:
            for transport in self._transports[:]:
                transport.close()
        finally:
            self._transports.clear()
            self._open = False

    def _require_open(self):
        if not self._open:
            raise RuntimeError('USB context is closed')

    def probe(self):
        """Read standard descriptors only, without opening/claiming a device."""
        self._require_open()
        return _usb.probe()

    def open_transport(self, *, bus, address):
        """Explicit future-protocol operation; claims only interface zero.

        Bus/address selects a current device, not a persistent identity. This
        operation does not change configuration, detach drivers, or send data.
        """
        self._require_open()
        transport = _usb.open_transport(bus, address)
        self._transports.append(transport)
        return transport
