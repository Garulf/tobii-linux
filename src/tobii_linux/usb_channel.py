"""Explicit experimental channel lifecycle, not complete device startup.

Entering this context issues vendor control 0x41/0x41. Use only after an
explicit experiment has selected a known descriptor-confirmed transport.
No timestamp negotiation, metadata updates, resets or firmware writes occur.
"""
import math
import time
from .usb import USBError
from .usb_framing import CHANNEL_COUNT, IncomingFrames, outgoing_frames

LIBUSB_ERROR_TIMEOUT = -7


class ShortWriteError(RuntimeError):
    def __init__(self, transferred, expected):
        self.transferred, self.expected = transferred, expected
        super().__init__(f'Partial channel frame write: {transferred}/{expected}; do not retry blindly')


class USBChannel:
    def __init__(self, transport, channel=0, *, control_timeout_ms=1000):
        if not 0 <= channel < CHANNEL_COUNT:
            raise ValueError('Channel must be between 0 and 11')
        if not 1 <= control_timeout_ms <= 60000:
            raise ValueError('Invalid control timeout')
        self.transport, self.channel = transport, channel
        self.control_timeout_ms = control_timeout_ms
        self.frames = IncomingFrames()
        self.opened = False
        self.used = False
        self.failed = False

    def __enter__(self):
        if self.used or self.failed:
            raise RuntimeError('Channel context is single-use or failed')
        self.used = True
        self.transport.control_out(0x41, value=self.channel,
                                   timeout_ms=self.control_timeout_ms)
        self.opened = True
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        try:
            self.close()
        except Exception:
            if exc_type is None:
                raise
            # Preserve the original transfer/protocol error during cleanup.

    def close(self):
        if self.opened:
            self.opened = False
            self.transport.control_out(0x42, value=self.channel,
                                       timeout_ms=self.control_timeout_ms)

    def _check_open(self):
        if not self.opened or self.failed:
            raise RuntimeError('Channel is closed or failed')

    def write(self, payload, *, timeout_ms=3000):
        """Write channel frames with one total deadline and no automatic retry."""
        self._check_open()
        if not 1 <= timeout_ms <= 60000:
            raise ValueError('Invalid write timeout')
        deadline = time.monotonic() + timeout_ms / 1000
        total = 0
        try:
            for frame in outgoing_frames(self.channel, payload):
                remaining_ms = math.ceil((deadline - time.monotonic()) * 1000)
                if remaining_ms <= 0:
                    raise TimeoutError('Channel write deadline expired')
                count = self.transport.write(frame, timeout_ms=remaining_ms)
                if count != len(frame):
                    raise ShortWriteError(count, len(frame))
                total += count - 8
        except Exception:
            self.failed = True
            raise
        return total

    def read(self, *, timeout_ms=1000):
        """Read once and return completed payloads addressed to this channel.

        Empty bytes can mean a timeout, a partial frame or data addressed
        elsewhere. Bytes received before a timeout are kept in the frame parser.
        """
        self._check_open()
        try:
            data = self.transport.read(16384, timeout_ms=timeout_ms)
        except USBError as error:
            if error.code != LIBUSB_ERROR_TIMEOUT:
                self.failed = True
                raise
            data = error.partial_data
        try:
            frames = self.frames.feed(data)
        except Exception:
            self.failed = True
            raise
        return b''.join(frame.payload for frame in frames if frame.for_channel(self.channel))
