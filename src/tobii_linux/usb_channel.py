"""Explicit channel lifecycle, not complete device startup.

Entering a USBChannel issues vendor control 0x41/0x41 and leaving it 0x42. With a
native transport the channel reads the IN endpoint continuously on its own
thread while open (the tracker stalls when it is left unread for about a
second); other transports are read once per read() call. No timestamp
negotiation, metadata updates, resets or firmware writes occur. The
implementation is crates/tobii-usb/src/channel.rs.
"""
from ._native import register_errors
from ._native import usb as _usb

LIBUSB_ERROR_TIMEOUT = _usb.LIBUSB_ERROR_TIMEOUT


class ShortWriteError(RuntimeError):
    def __init__(self, transferred, expected):
        self.transferred, self.expected = transferred, expected
        super().__init__(f'Partial channel frame write: {transferred}/{expected}; do not retry blindly')


register_errors(short_write_error=ShortWriteError)


# USBChannel(transport, channel=0, *, control_timeout_ms=1000); transport is a
# usb.BulkTransport or any object with its read/write/control_out methods.
USBChannel = _usb.USBChannel
