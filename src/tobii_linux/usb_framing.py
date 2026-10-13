"""Direction-specific channel framing recovered from the ET5 Windows runtime.

Supplemental static/emulated evidence is in docs/usb-transport-static.md.
No hardware capture yet. Validation limits are our design, not vendor limits.
"""
from dataclasses import dataclass
import struct

HEADER = struct.Struct('<II')
CHANNEL_COUNT = 12
MAX_FRAME = 1024 * 1024  # Local memory bound; not a claimed firmware limit.
MAX_OUT_PAYLOAD = 4087


def outgoing_frames(channel, payload):
    """Yield individual bulk OUT records, preserving original chunk choices."""
    if not 0 <= channel < CHANNEL_COUNT:
        raise ValueError('Channel must be between 0 and 11')
    if len(payload) > MAX_FRAME:
        raise ValueError('Payload exceeds local frame limit')
    offset = 0
    while offset < len(payload):
        count = min(len(payload) - offset, MAX_OUT_PAYLOAD)
        if count % 512 == 0:
            count -= 1
        yield HEADER.pack(channel, count) + bytes(payload[offset:offset + count])
        offset += count


@dataclass(frozen=True)
class IncomingFrame:
    recipients: int
    payload: bytes

    def for_channel(self, channel):
        if not 0 <= channel < CHANNEL_COUNT:
            raise ValueError('Channel must be between 0 and 11')
        return bool(self.recipients & (1 << channel))


class IncomingFrames:
    """Incremental bulk IN parser: [recipient bitmask, total size incl header].

    Fragmented headers/payloads and multiple frames per USB read are supported.
    Invalid framing poisons the parser until the owner creates a new instance.
    We preserve recipient bits; the original dispatches only channels 0..11.
    """
    def __init__(self, max_frame=MAX_FRAME):
        if not 8 <= max_frame <= MAX_FRAME:
            raise ValueError('Invalid frame limit')
        self.max_frame = max_frame
        self.buffer = bytearray()
        self.failed = False

    def feed(self, data):
        if self.failed:
            raise ValueError('Parser unusable after invalid framing')
        # Bound each input chunk independently, including many small frames.
        if len(data) > self.max_frame:
            self.failed = True
            self.buffer.clear()
            raise ValueError('Input exceeds local frame limit')
        frames = []
        offset = 0
        while offset < len(data) or len(self.buffer) >= 8:
            target = 8
            if len(self.buffer) >= 8:
                recipients, target = HEADER.unpack_from(self.buffer)
                if not 8 <= target <= self.max_frame:
                    self.failed = True
                    self.buffer.clear()
                    raise ValueError('Invalid incoming total frame length')
            needed = target - len(self.buffer)
            if needed:
                take = min(needed, len(data) - offset)
                self.buffer.extend(data[offset:offset + take])
                offset += take
                if len(self.buffer) < target:
                    break
                if target == 8:
                    # Parse the newly completed header before emitting a frame.
                    recipients, target = HEADER.unpack_from(self.buffer)
                    if not 8 <= target <= self.max_frame:
                        self.failed = True
                        self.buffer.clear()
                        raise ValueError('Invalid incoming total frame length')
                    if target > 8:
                        continue
            recipients, target = HEADER.unpack_from(self.buffer)
            frames.append(IncomingFrame(recipients, bytes(self.buffer[8:target])))
            self.buffer.clear()
        return frames
