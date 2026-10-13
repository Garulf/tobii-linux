"""Recovered inner TTP framing, not the USB transport envelope.

Derived from the installed Tobii platform runtime; see docs/usb-protocol.md.
Unknown header words and TLV tags are preserved. Nothing here writes to hardware.
"""
from dataclasses import dataclass
import struct

HEADER = struct.Struct(">6I")
MAX_PAYLOAD = 4 * 1024 * 1024  # Local resource limit, not a claimed device limit.


class ProtocolError(ValueError):
    pass


def uint32(value: int) -> int:
    if type(value) is not int or not 0 <= value <= 0xFFFFFFFF:
        raise ProtocolError("Expected an unsigned 32-bit integer")
    return value


@dataclass(frozen=True)
class Message:
    kind: int
    request_id: int
    word2: int
    command: int
    word4: int
    payload: bytes = b""

    def encode(self, max_payload: int = MAX_PAYLOAD) -> bytes:
        if not isinstance(self.payload, bytes):
            raise TypeError("payload must be bytes")
        if len(self.payload) > max_payload:
            raise ProtocolError("Payload exceeds local size limit")
        return HEADER.pack(*(uint32(n) for n in (
            self.kind, self.request_id, self.word2, self.command, self.word4,
            len(self.payload)))) + self.payload


class Decoder:
    """Incremental decoder, preserving USB/pipe chunk independence.

    On a framing error, discard this decoder and reconnect. No resynchronization
    marker has been established, so scanning for one would silently invent data.
    """
    def __init__(self, max_payload: int = MAX_PAYLOAD):
        if type(max_payload) is not int or not 0 <= max_payload <= 0xFFFFFFFF:
            raise ValueError("Invalid payload limit")
        self.max_payload = max_payload
        self.pending = bytearray()
        self.expected = HEADER.size
        self.header = None
        self.failed = False

    def feed(self, data: bytes) -> list[Message]:
        if self.failed:
            raise ProtocolError("Decoder failed; a new stream requires a new decoder")
        result = []
        source = memoryview(data).cast("B")
        offset = 0
        while offset < len(source):
            count = min(self.expected - len(self.pending), len(source) - offset)
            self.pending.extend(source[offset:offset + count])
            offset += count
            if len(self.pending) != self.expected:
                continue
            if self.header is None:
                self.header = HEADER.unpack(self.pending)
                length = self.header[-1]
                if length > self.max_payload:
                    self.failed = True
                    self.pending.clear()
                    raise ProtocolError("Declared payload exceeds local size limit")
                self.pending.clear()
                self.expected = length
                if length:
                    continue
            result.append(Message(*self.header[:5], bytes(self.pending)))
            self.pending.clear()
            self.header = None
            self.expected = HEADER.size
        return result

    def finish(self) -> None:
        if self.failed or self.pending or self.header is not None:
            raise ProtocolError("Stream ended with an invalid or incomplete message")


@dataclass(frozen=True)
class Field:
    tag: int
    data: bytes

    def encode(self) -> bytes:
        if type(self.tag) is not int or not 0 <= self.tag <= 255:
            raise ProtocolError("TLV tag must fit one byte")
        if not isinstance(self.data, bytes):
            raise TypeError("TLV data must be bytes")
        return bytes([self.tag]) + struct.pack(">I", uint32(len(self.data))) + self.data


def fields(payload: bytes) -> list[Field]:
    """Decode the zero-prefixed typed payload observed in the runtime.

    The meaning of the two prefix bytes is not yet established. The runtime's
    parser checks them for zero. They are not assumed to be a success code.
    """
    if not payload:
        return []
    if len(payload) < 2 or payload[:2] != b"\0\0":
        raise ProtocolError("Unsupported payload prefix")
    result = []
    offset = 2
    while offset < len(payload):
        if len(payload) - offset < 5:
            raise ProtocolError("Truncated TLV header")
        tag = payload[offset]
        length = struct.unpack_from(">I", payload, offset + 1)[0]
        offset += 5
        if length > len(payload) - offset:
            raise ProtocolError("Truncated TLV data")
        result.append(Field(tag, payload[offset:offset + length]))
        offset += length
    return result


def uint32_array(values) -> Field:
    items = tuple(uint32(n) for n in values)
    data = struct.pack(">I", uint32(len(items)))
    data += b"".join(struct.pack(">I", n) for n in items)
    return Field(0x17, data)


def read_uint32_array(field: Field) -> tuple[int, ...]:
    if field.tag != 0x17 or len(field.data) < 4:
        raise ProtocolError("Not a uint32 array field")
    count = struct.unpack_from(">I", field.data)[0]
    if count != (len(field.data) - 4) // 4 or len(field.data) != 4 + count * 4:
        raise ProtocolError("Array count does not match byte length")
    return tuple(n[0] for n in struct.iter_unpack(">I", field.data[4:]))


def protocol_upgrade(request_id: int) -> Message:
    """Reconstruct the observed inner upgrade request; not a USB-ready packet."""
    versions = range(0x10000, 0x10009)
    payload = b"\0\0" + uint32_array(versions).encode()
    return Message(0x51, uint32(request_id), 0, 1000, 0, payload)
