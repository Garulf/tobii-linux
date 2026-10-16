"""Offline codecs for the bundled game DLL's PRP client session.

Covers the named-pipe connection handshake, transport framing, and the command
request/response messages the client sends to obtain head pose: connect (1),
stream start (3), stream stop (4) and timesync (22), and the property messages of
the game integration layer: get (operation 5, answered by operation 6), subscribe
(command 5) and unsubscribe (command 6). These are pure codecs; they do not open
pipes or sockets and do not implement a service. Implemented in
crates/tobii-service/src/prp_session.rs; the dataclasses here carry its values.

Recovered from DLL SHA256 35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020.
See docs/service-transport.md for locations, evidence and limits.
"""

from dataclasses import dataclass
import struct

from tobii_linux._native import prp_session as _session
from tobii_linux.prp import MAGIC, ProtocolError, encode_envelope, decode_envelope  # noqa: F401

URL_SCHEME = "tobii-prp://"
SERVER_PIPE_PREFIX = "TOBIIPRP-"
PIPE_ROOT = "\\\\.\\pipe\\"
CLIENT_PIPE_FORMAT = "client_{server}_{pid:08x}_{counter:016x}"

COMMAND_CHANNEL = 0x4E21
SUBSCRIPTION_CHANNEL = 0x4E22
TIMESYNC_CHANNEL = 0x4E23
CHANNELS = (COMMAND_CHANNEL, SUBSCRIPTION_CHANNEL, TIMESYNC_CHANNEL)

HANDSHAKE_VERSION = 1
HANDSHAKE_ACK = 1
# Server-side buffer for the handshake information message (0x18008f470).
MAX_HANDSHAKE_INFO = 0x100
# Client WriteFile and ReadFile chunk size (0x18004f250, 0x180090ad0).
PIPE_CHUNK = 0x1000

OPERATION_STREAM_PACKAGE = 1
OPERATION_PROPERTY_NOTIFICATION = 2
OPERATION_PROPERTY_GET = 5
OPERATION_PROPERTY_GET_RESPONSE = 6
OPERATION_COMMAND_REQUEST = 9
OPERATION_COMMAND_RESPONSE = 10

COMMAND_CONNECT = 1
COMMAND_STREAM_START = 3
COMMAND_STREAM_STOP = 4
COMMAND_TIMESYNC = 0x16
COMMAND_PROPERTY_SUBSCRIBE = 5
COMMAND_PROPERTY_UNSUBSCRIBE = 6
WORD_RESPONSE_COMMANDS = (COMMAND_STREAM_START, COMMAND_STREAM_STOP,
                          COMMAND_PROPERTY_UNSUBSCRIBE)

STATUS_NAMES = _session.STATUS_NAMES
STATUS_OK = 0
STATUS_NOT_SUPPORTED = 2

CONNECT_REQUEST_BLOCK = 0x3C
CONNECT_REQUEST_BLOCK_EXTENDED = 0x40
CONNECT_RESPONSE_BLOCK = 0x98
CONNECT_RESPONSE_BLOCK_EXTENDED = 0xA8

# Decoder capacity limits for connect response lists (0x180055780, operation 10).
LIMITS = dict(list_a=0x19, list_b=0x19, list_c=0x19, list_d=0x19, streams=0x20,
              supported_commands=0x21, groups=5, group_values=0x20, license_results=0x100,
              list_g=0x20)
STRING_LIMITS = _session.STRING_LIMITS
EXTENDED_STRING_LIMIT = 0x80
BUILD_VERSION_LIMIT = 0x100


# Pipe naming and handshake ------------------------------------------------------------


def server_pipe_names(url: str) -> tuple[str, str]:
    """Pipe names the client tries for a tobii-prp:// URL, in order.

    The client looks for TOBIIPRP-<name> in a \\\\.\\pipe\\* listing; if it is not
    listed it opens <name> unchanged (0x18004e8a0, 0x18008fb20).
    """
    return _session.server_pipe_names(url)


def client_pipe_name(server_pipe: str, pid: int, counter: int) -> str:
    """Name of the inbound pipe the client creates for one channel."""
    return _session.client_pipe_name(server_pipe, pid, counter)


@dataclass(frozen=True)
class ClientHello:
    channel: int
    reply_pipe: str
    version: int = HANDSHAKE_VERSION


def encode_client_hello(hello: ClientHello) -> bytes:
    return _session.encode_client_hello(hello)


def decode_client_hello(message: bytes) -> ClientHello:
    """Parse the first client message as the PRP-mode server does (0x18008f470)."""
    return ClientHello(*_session.decode_client_hello(message))


def encode_handshake_ack() -> bytes:
    return _session.encode_handshake_ack()


def check_handshake_ack(message: bytes) -> None:
    """Client accepts exactly one four-byte value 1 (0x18004f300, 0x18004f00b)."""
    _session.check_handshake_ack(message)


def encode_client_pid(pid: int) -> bytes:
    return _session.encode_client_pid(pid)


def decode_client_pid(message: bytes) -> int:
    return _session.decode_client_pid(message)


@dataclass(frozen=True)
class HandshakeInfo:
    client_id: int
    subscription_url: str
    timesync_url: str


def encode_handshake_info(info: HandshakeInfo) -> bytes:
    return _session.encode_handshake_info(info)


def decode_handshake_info(message: bytes) -> HandshakeInfo:
    """Parse with the client's rule: id plus two NUL-terminated strings (0x18004f360)."""
    return HandshakeInfo(*_session.decode_handshake_info(message))


# Transport framing --------------------------------------------------------------------


def pipe_chunks(packet: bytes) -> list[bytes]:
    """Split one packet into the client's WriteFile chunks (one pipe message each)."""
    return _session.pipe_chunks(packet)


def split_packets(stream: bytes, *,
                  max_payload: int = 16 * 1024 * 1024) -> tuple[list[bytes], bytes]:
    """Return complete TPRP packets from reassembled bytes and the incomplete tail."""
    return _session.split_packets(stream, max_payload)


# Command requests ---------------------------------------------------------------------


@dataclass(frozen=True)
class ConnectRequest:
    """Command 1 body serialized by 0x18004fdd0 from the native request at 0x18001fed0."""
    subscription_client_id: int
    licenses: tuple[bytes, ...] = ()
    signer_public_key: bytes = b""
    client_version: tuple[int, int, int, int] = (4, 17, 0, 6)
    process_id: int = 0
    image_directory: bytes = b""
    image_name: bytes = b""
    field_of_use: int = 1


@dataclass(frozen=True)
class CommandRequest:
    tag: int
    command: int
    stream: int | None = None
    connect: ConnectRequest | None = None


def encode_connect_request(tag: int, request: ConnectRequest, *, extended: bool = True) -> bytes:
    return _session.encode_connect_request(tag, request, extended)


def encode_stream_request(tag: int, command: int, stream: int) -> bytes:
    """Stream start (3) or stop (4) carrying one PRP stream id."""
    return _session.encode_stream_request(tag, command, stream)


def encode_timesync_request(tag: int) -> bytes:
    return _session.encode_timesync_request(tag)


def decode_command_request(packet: bytes, *, extended: bool = True) -> CommandRequest:
    """Server-side view of the commands this client sends during head-pose sessions."""
    tag, command, stream, connect = _session.decode_command_request(packet, extended)
    return CommandRequest(tag, command, stream, None if connect is None else ConnectRequest(*connect))


# Command responses --------------------------------------------------------------------


@dataclass(frozen=True)
class ConnectResponse:
    """Command 1 response body, serialized by 0x180050370 and stored by the device.

    Field names follow observed consumers; lists without a recovered consumer keep
    neutral names. streams holds PRP stream ids checked by stream support
    (0x18002c6d0); license_results must have one entry per requested license
    (0x18002a6a0); firmware_upgrade_in_progress must be false for device creation.
    supported_commands is compared with PRP command ids: 0x16 (timesync) makes
    device creation run up to four timesync rounds (0x180030630), and capability
    queries search it (0x18002cd30). Treating it as a command list is an inference.
    """
    build_version: str = ""
    firmware_upgrade_in_progress: bool = False
    list_a: tuple[int, ...] = ()
    list_b: tuple[int, ...] = ()
    list_c: tuple[int, ...] = ()
    list_d: tuple[int, ...] = ()
    streams: tuple[int, ...] = ()
    supported_commands: tuple[int, ...] = ()
    groups: tuple[tuple[int, tuple[int, ...]], ...] = ()
    strings: tuple[str, ...] = ("",) * 7
    words: tuple[int, ...] = (0,) * 9
    license_results: tuple[int, ...] = ()
    mode: int = 0
    list_g: tuple[int, ...] = ()
    extended_strings: tuple[str, ...] = ("",) * 4


@dataclass(frozen=True)
class CommandResponse:
    tag: int
    command: int
    status: int
    value: int | None = None
    connect: ConnectResponse | None = None


def encode_command_response(response: CommandResponse, *, extended: bool = True) -> bytes:
    """Serialize operation 10 for connect, stream start/stop, timesync and unsubscribe."""
    return _session.encode_command_response(response, extended)


def decode_command_response(packet: bytes, *, extended: bool = True) -> CommandResponse:
    """Client-side view of operation 10 for the head-pose session commands."""
    tag, command, status, value, connect = _session.decode_command_response(packet, extended)
    return CommandResponse(tag, command, status, value,
                           None if connect is None else ConnectResponse(*connect))


# Property messages ---------------------------------------------------------------------

PROPERTY_DISPLAY_AREA = 1
PROPERTY_DISPLAY_ID = 0x12
DISPLAY_AREA_FLOATS = 9
STRING_PROPERTIES = (8, PROPERTY_DISPLAY_ID)
STRING_PROPERTY_LIMIT = 0x100


def encode_property_request(tag: int, property_id: int) -> bytes:
    """Operation 5 as the client sends it (0x180019f60): one block holding the property id."""
    return _session.encode_property_request(tag, property_id)


def encode_property_subscribe_request(tag: int, property_id: int,
                                      command: int = COMMAND_PROPERTY_SUBSCRIBE) -> bytes:
    """Command 5 (subscribe, 0x180019830) or 6 (unsubscribe) with the property id."""
    return _session.encode_property_subscribe_request(tag, property_id, command)


def decode_property_subscribe_request(packet: bytes) -> tuple[int, int, int]:
    """Tag, command (5 or 6) and property id of a property subscription request."""
    return _session.decode_property_subscribe_request(packet)


def decode_property_request(packet: bytes) -> tuple[int, int]:
    """Tag and property id of an operation 5 request."""
    return _session.decode_property_request(packet)


@dataclass(frozen=True)
class PropertyResponse:
    """A property value with its status, as the raw value block and region.

    Sent as operation 6 for a get (operation 5) and inside the command 5 response for
    a subscribe; the value block layout depends on the property id.
    """
    tag: int
    status: int
    property_id: int
    block: bytes = struct.pack("<I", 4)
    region: bytes = struct.pack("<I", 4)


def encode_property_response(response: PropertyResponse) -> bytes:
    return _session.encode_property_response(response)


def encode_property_subscribe_response(response: PropertyResponse) -> bytes:
    """Command 5 response: status, property id, then its current value (decoder 0x180055780)."""
    return _session.encode_property_subscribe_response(response)


def encode_property_notification(tag: int, property_id: int, block: bytes, region: bytes) -> bytes:
    """Operation 2 on the subscription channel: {8, property} then the value as in operation 6.

    Layout from the game DLL's serializer 0x18004f730 and decoder 0x180055780 (emulated).
    """
    return _session.encode_property_notification(tag, property_id, block, region)


def decode_property_response(packet: bytes) -> PropertyResponse:
    return PropertyResponse(*_session.decode_property_response(packet))


def decode_property_subscribe_response(packet: bytes) -> PropertyResponse:
    return PropertyResponse(*_session.decode_property_subscribe_response(packet))


def display_area_value(top_left, top_right, bottom_left) -> tuple[bytes, bytes]:
    """Value block for property 1: nine floats in millimetres (decoder 0x180055780)."""
    return _session.display_area_value((*top_left, *top_right, *bottom_left))


def string_value(text: str) -> tuple[bytes, bytes]:
    """Value block for string properties: a region offset, then the NUL-terminated text."""
    return _session.string_value(text)


def decode_display_area(response: PropertyResponse) -> tuple[float, ...]:
    return tuple(_session.decode_display_area(response))


def decode_string_value(response: PropertyResponse) -> str:
    return _session.decode_string_value(response)


def minimal_connect_response(build_version: str, license_count: int,
                             streams: tuple[int, ...] = (2, 3, 29)) -> ConnectResponse:
    """Connect response satisfying the device_create checks recovered statically.

    Only the build version string, the firmware-upgrade flag, the stream list and the
    license-result count are known to be checked. supported_commands is left empty,
    so device creation skips its timesync rounds. All other fields are zero or empty;
    their effect on later API calls is not established.
    """
    return ConnectResponse(*_session.minimal_connect_response(build_version, license_count, streams))


@dataclass
class ClientSession:
    """Client tag counters: command tags (client+0x68cc) and timesync tags (client+0x5638).

    Both are incremented before use, so the first tag on each channel is 1.
    """
    command_tag: int = 0
    timesync_tag: int = 0

    def next_command_tag(self) -> int:
        self.command_tag = (self.command_tag + 1) & 0xFFFFFFFF
        return self.command_tag

    def next_timesync_tag(self) -> int:
        self.timesync_tag = (self.timesync_tag + 1) & 0xFFFFFFFF
        return self.timesync_tag
