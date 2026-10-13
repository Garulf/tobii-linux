#!/usr/bin/env python3
"""Bounded, read-only PRP client probe against the real Tobii service on Windows.

Performs the client side of the named-pipe handshake on the command and
subscription channels as the game DLL does (docs/service-transport.md), sends
connect, optional timesync rounds, optional property gets (operation 5, read
only, as the game integration layer sends them), stream start for head pose (PRP stream 2),
records every pipe message with local QueryPerformanceCounter time, then sends
stream stop and closes. It never sends calibration, settings or firmware
commands. Output is JSONL on stdout.

Requires 64-bit Windows Python with src/ on sys.path (prp.py, prp_session.py).
"""
import argparse
import ctypes as C
import ctypes.wintypes as W
import json
import os
from pathlib import Path
import queue
import struct
import subprocess
import sys
import threading
import time

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))
from tobii_linux.prp import ProtocolError, decode_envelope, decode_head_body  # noqa: E402
from tobii_linux.prp_session import (  # noqa: E402
    COMMAND_CHANNEL, COMMAND_CONNECT, COMMAND_STREAM_START, COMMAND_STREAM_STOP,
    COMMAND_TIMESYNC, SERVER_PIPE_PREFIX, SUBSCRIPTION_CHANNEL, TIMESYNC_CHANNEL,
    COMMAND_PROPERTY_SUBSCRIBE, COMMAND_PROPERTY_UNSUBSCRIBE, PROPERTY_DISPLAY_AREA,
    STRING_PROPERTIES, ClientHello, ClientSession, ConnectRequest,
    check_handshake_ack, client_pipe_name, decode_command_response, decode_display_area,
    decode_handshake_info, decode_property_response, decode_string_value, encode_client_hello,
    encode_client_pid, encode_connect_request, encode_property_request,
    encode_property_subscribe_request, encode_stream_request, encode_timesync_request,
    pipe_chunks, split_packets)

PIPE_ROOT = "\\\\.\\pipe\\"
GENERIC_WRITE = 0x40000000
OPEN_EXISTING = 3
FILE_FLAG_OVERLAPPED = 0x40000000
PIPE_ACCESS_INBOUND = 1
PIPE_TYPE_MESSAGE = 4
PIPE_READMODE_MESSAGE = 2
CLIENT_PIPE_IN_BUFFER = 0x80000
ERROR_IO_PENDING = 997
ERROR_PIPE_CONNECTED = 535
ERROR_PIPE_BUSY = 231
ERROR_MORE_DATA = 234
WAIT_OBJECT_0 = 0
INVALID_HANDLE = C.c_void_p(-1).value
READ_CHUNK = 0x1000
HEAD_POSE_STREAM = 2

kernel32 = C.WinDLL("kernel32", use_last_error=True) if sys.platform == "win32" else None


class Overlapped(C.Structure):
    _fields_ = [("Internal", C.c_size_t), ("InternalHigh", C.c_size_t),
                ("Offset", W.DWORD), ("OffsetHigh", W.DWORD), ("hEvent", W.HANDLE)]


def bind(name, argtypes, restype=W.BOOL):
    function = getattr(kernel32, name)
    function.argtypes, function.restype = argtypes, restype
    return function


if kernel32:
    CreateNamedPipeA = bind("CreateNamedPipeA", [C.c_char_p, W.DWORD, W.DWORD, W.DWORD, W.DWORD,
                                                 W.DWORD, W.DWORD, C.c_void_p], W.HANDLE)
    CreateFileA = bind("CreateFileA", [C.c_char_p, W.DWORD, W.DWORD, C.c_void_p, W.DWORD,
                                       W.DWORD, W.HANDLE], W.HANDLE)
    SetNamedPipeHandleState = bind("SetNamedPipeHandleState",
                                   [W.HANDLE, C.POINTER(W.DWORD), C.c_void_p, C.c_void_p])
    WaitNamedPipeA = bind("WaitNamedPipeA", [C.c_char_p, W.DWORD])
    ConnectNamedPipe = bind("ConnectNamedPipe", [W.HANDLE, C.POINTER(Overlapped)])
    ReadFile = bind("ReadFile", [W.HANDLE, C.c_void_p, W.DWORD, C.POINTER(W.DWORD),
                                 C.POINTER(Overlapped)])
    WriteFile = bind("WriteFile", [W.HANDLE, C.c_void_p, W.DWORD, C.POINTER(W.DWORD),
                                   C.POINTER(Overlapped)])
    GetOverlappedResult = bind("GetOverlappedResult", [W.HANDLE, C.POINTER(Overlapped),
                                                       C.POINTER(W.DWORD), W.BOOL])
    CreateEventA = bind("CreateEventA", [C.c_void_p, W.BOOL, W.BOOL, C.c_char_p], W.HANDLE)
    WaitForSingleObject = bind("WaitForSingleObject", [W.HANDLE, W.DWORD], W.DWORD)
    CancelIoEx = bind("CancelIoEx", [W.HANDLE, C.POINTER(Overlapped)])
    CloseHandle = bind("CloseHandle", [W.HANDLE])
    QueryPerformanceCounter = bind("QueryPerformanceCounter", [C.POINTER(C.c_int64)])
    QueryPerformanceFrequency = bind("QueryPerformanceFrequency", [C.POINTER(C.c_int64)])
    GetModuleFileNameW = bind("GetModuleFileNameW", [W.HMODULE, C.c_wchar_p, W.DWORD], W.DWORD)
    SetEvent = bind("SetEvent", [W.HANDLE])
    ResetEvent = bind("ResetEvent", [W.HANDLE])


class PipeError(RuntimeError):
    pass


def emit(kind, **values):
    print(json.dumps({"kind": kind, **values}), flush=True)


def qpc_us():
    """The game DLL's client clock: QPC scaled to microseconds (0x180027650, static)."""
    counter, frequency = C.c_int64(), C.c_int64()
    QueryPerformanceCounter(C.byref(counter))
    QueryPerformanceFrequency(C.byref(frequency))
    return counter.value * 1_000_000 // frequency.value


def check(ok, what):
    if not ok:
        raise PipeError(f"{what} failed with error {C.get_last_error()}")


def wait_overlapped(handle, overlapped, timeout_ms, what):
    if WaitForSingleObject(overlapped.hEvent, timeout_ms) != WAIT_OBJECT_0:
        CancelIoEx(handle, C.byref(overlapped))
        WaitForSingleObject(overlapped.hEvent, 1000)
        raise PipeError(f"{what} timed out after {timeout_ms} ms")
    done = W.DWORD()
    ok = GetOverlappedResult(handle, C.byref(overlapped), C.byref(done), False)
    return ok, C.get_last_error(), done.value


def list_prp_pipes():
    return sorted(name for name in os.listdir(PIPE_ROOT)
                  if name.upper().startswith(SERVER_PIPE_PREFIX))


def server_pipe_for(url):
    """Mirror 0x18004e8a0: tobii-prp:// prefers a listed TOBIIPRP-<name>; other schemes use <name>."""
    scheme, separator, name = url.partition("://")
    if not separator or not name:
        raise PipeError(f"unexpected URL {url!r}")
    if scheme == "tobii-prp":
        prefixed = SERVER_PIPE_PREFIX + name
        reply_base = prefixed
        listed = {n.upper() for n in os.listdir(PIPE_ROOT)}
        return (prefixed if prefixed.upper() in listed else name), reply_base
    return name, name


class Channel:
    """One client channel: outbound handle to the service pipe, inbound reply pipe."""

    counter = 0

    def __init__(self, label, url, channel, timeout_ms):
        self.label, self.url, self.channel, self.timeout_ms = label, url, channel, timeout_ms
        self.messages = queue.Queue()
        self.closed = threading.Event()
        self.reply = self.server = None
        server_name, reply_base = server_pipe_for(url)
        Channel.counter += 1
        self.reply_name = client_pipe_name(reply_base, os.getpid(), Channel.counter)
        self.server_name = server_name

    def open(self):
        self.reply = CreateNamedPipeA((PIPE_ROOT + self.reply_name).encode(),
                                      PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED,
                                      PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE, 1, 0,
                                      CLIENT_PIPE_IN_BUFFER, 5000, None)
        check(self.reply != INVALID_HANDLE, "CreateNamedPipeA")
        path = (PIPE_ROOT + self.server_name).encode()
        for attempt in range(4):
            self.server = CreateFileA(path, GENERIC_WRITE, 0, None, OPEN_EXISTING,
                                      FILE_FLAG_OVERLAPPED, None)
            if self.server != INVALID_HANDLE:
                break
            if C.get_last_error() != ERROR_PIPE_BUSY:
                raise PipeError(f"open {self.server_name}: error {C.get_last_error()}")
            WaitNamedPipeA(path, 1000)
        check(self.server != INVALID_HANDLE, "CreateFileA")
        mode = W.DWORD(PIPE_READMODE_MESSAGE)
        check(SetNamedPipeHandleState(self.server, C.byref(mode), None, None),
              "SetNamedPipeHandleState")
        self.connect_overlapped = Overlapped(hEvent=CreateEventA(None, True, False, None))
        if not ConnectNamedPipe(self.reply, C.byref(self.connect_overlapped)):
            error = C.get_last_error()
            if error == ERROR_PIPE_CONNECTED:
                SetEvent(self.connect_overlapped.hEvent)
            elif error != ERROR_IO_PENDING:
                raise PipeError(f"ConnectNamedPipe error {error}")
        emit("channel_open", label=self.label, url=self.url, server_pipe=self.server_name,
             reply_pipe=self.reply_name)

    def write(self, message):
        overlapped = Overlapped(hEvent=CreateEventA(None, True, False, None))
        written = W.DWORD()
        try:
            if not WriteFile(self.server, message, len(message), C.byref(written),
                             C.byref(overlapped)) and C.get_last_error() != ERROR_IO_PENDING:
                raise PipeError(f"WriteFile error {C.get_last_error()}")
            ok, error, _ = wait_overlapped(self.server, overlapped, self.timeout_ms, "WriteFile")
            if not ok:
                raise PipeError(f"WriteFile error {error}")
        finally:
            CloseHandle(overlapped.hEvent)
        emit("message", label=self.label, direction="client_to_service", qpc_us=qpc_us(),
             hex=message.hex())

    def read_message(self, timeout_ms):
        """Read one whole pipe message, following ERROR_MORE_DATA like 0x180090ad0."""
        parts = []
        overlapped = Overlapped(hEvent=CreateEventA(None, True, False, None))
        buffer = C.create_string_buffer(READ_CHUNK)
        try:
            while True:
                ResetEvent(overlapped.hEvent)
                got = W.DWORD()
                if not ReadFile(self.reply, buffer, READ_CHUNK, C.byref(got), C.byref(overlapped)):
                    if C.get_last_error() not in (ERROR_IO_PENDING, ERROR_MORE_DATA):
                        raise PipeError(f"ReadFile error {C.get_last_error()}")
                ok, error, got.value = wait_overlapped(self.reply, overlapped, timeout_ms,
                                                       "ReadFile")
                error = 0 if ok else error
                if error not in (0, ERROR_MORE_DATA):
                    raise PipeError(f"ReadFile error {error}")
                parts.append(buffer.raw[:got.value])
                if error == 0:
                    message = b"".join(parts)
                    emit("message", label=self.label, direction="service_to_client",
                         qpc_us=qpc_us(), hex=message.hex())
                    return message
        finally:
            CloseHandle(overlapped.hEvent)

    def handshake(self):
        """Steps 1 to 5 of the PRP-mode handshake (0x18004e8a0)."""
        self.write(encode_client_hello(ClientHello(self.channel, self.reply_name)))
        ok, error, _ = wait_overlapped(self.reply, self.connect_overlapped, self.timeout_ms,
                                       "reply pipe connect")
        if not ok and error != ERROR_PIPE_CONNECTED:
            raise PipeError(f"reply pipe connect error {error}")
        check_handshake_ack(self.read_message(self.timeout_ms))
        self.write(encode_client_pid(os.getpid()))
        info = decode_handshake_info(self.read_message(self.timeout_ms))
        emit("handshake", label=self.label, client_id=info.client_id,
             subscription_url=info.subscription_url, timesync_url=info.timesync_url)
        return info

    def start_reader(self):
        def run():
            while not self.closed.is_set():
                try:
                    message = self.read_message(250)
                except PipeError as exc:
                    if "timed out" in str(exc):
                        continue
                    if not self.closed.is_set():
                        emit("reader_end", label=self.label, reason=str(exc))
                    return
                self.messages.put((qpc_us(), message))
        self.reader = threading.Thread(target=run, daemon=True)
        self.reader.start()

    def send_packet(self, packet):
        for chunk in pipe_chunks(packet):
            self.write(chunk)

    def close(self):
        self.closed.set()
        if getattr(self, "reader", None):
            self.reader.join(2.0)
        for handle in (self.server, self.reply):
            if handle and handle != INVALID_HANDLE:
                CloseHandle(handle)
        emit("channel_closed", label=self.label)


class PacketReader:
    def __init__(self, channel):
        self.channel, self.tail = channel, b""

    def packets(self, timeout_s):
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            try:
                received_at, message = self.channel.messages.get(timeout=0.05)
            except queue.Empty:
                continue
            packets, self.tail = split_packets(self.tail + message)
            for packet in packets:
                yield received_at, packet

    def response(self, tag, timeout_s):
        for _, packet in self.packets(timeout_s):
            try:
                response = decode_command_response(packet)
            except ProtocolError as exc:
                emit("unparsed_packet", label=self.channel.label, error=str(exc), hex=packet.hex())
                continue
            if response.tag == tag:
                return response
            emit("unexpected_response", label=self.channel.label, tag=response.tag)
        raise PipeError(f"no response with tag {tag} on {self.channel.label}")

    def property_response(self, tag, timeout_s):
        for _, packet in self.packets(timeout_s):
            try:
                response = decode_property_response(packet)
            except ProtocolError as exc:
                emit("unparsed_packet", label=self.channel.label, error=str(exc), hex=packet.hex())
                continue
            if response.tag == tag:
                return packet, response
            emit("unexpected_response", label=self.channel.label, tag=response.tag)
        raise PipeError(f"no property response with tag {tag} on {self.channel.label}")


    def tagged_packet(self, tag, timeout_s):
        for _, packet in self.packets(timeout_s):
            words = struct.unpack_from("<3I", decode_envelope(packet))
            if words[1] == tag:
                return packet
            emit("unexpected_packet", label=self.channel.label, hex=packet.hex())
        raise PipeError(f"no packet with tag {tag} on {self.channel.label}")


def subscribe_properties(command, commands, session, property_ids, subscribe_command):
    """Command 5 (subscribe) or 6 (unsubscribe) per property; records the raw replies."""
    for property_id in property_ids:
        tag = session.next_command_tag()
        command.send_packet(encode_property_subscribe_request(tag, property_id,
                                                              subscribe_command))
        packet = commands.tagged_packet(tag, 5.0)
        payload = decode_envelope(packet)
        emit("property_subscription_response", command=subscribe_command,
             property_id=property_id, operation=struct.unpack_from("<I", payload, 8)[0],
             status=struct.unpack_from("<i", payload, 20)[0], hex=packet.hex())


def property_value(response):
    if response.status != 0:
        return None
    if response.property_id == PROPERTY_DISPLAY_AREA:
        return list(decode_display_area(response))
    if response.property_id in STRING_PROPERTIES:
        return decode_string_value(response)
    return None


def read_properties(command, commands, session, property_ids):
    for property_id in property_ids:
        tag = session.next_command_tag()
        command.send_packet(encode_property_request(tag, property_id))
        packet, response = commands.property_response(tag, 5.0)
        emit("property_response", property_id=property_id, status=response.status,
             returned_property=response.property_id, value=property_value(response),
             block=response.block.hex(), region=response.region.hex(), hex=packet.hex())


def notifications(reader, seconds):
    for received_at, packet in reader.packets(seconds):
        emit("subscription_packet", client_received_us=received_at, **stream_package(packet))


def stream_package(packet):
    """Parse operation 1 head-pose packages; other operations are reported raw."""
    payload = decode_envelope(packet)
    words = struct.unpack_from("<7I", payload)
    if words[0] != 12 or words[2] != 1 or words[5] != HEAD_POSE_STREAM:
        return {"operation": words[2], "raw": packet.hex()}
    body = payload[28:28 + 52]
    pose = decode_head_body(body)
    return {"message_tag": words[1], "stream": words[5], "timestamp": pose.timestamp,
            "position_valid": pose.position_valid, "position": list(pose.position),
            "rotation_valid": list(pose.rotation_valid), "rotation": list(pose.rotation),
            "trailer": payload[80:].hex()}


def image_components():
    buffer = C.create_unicode_buffer(260)
    GetModuleFileNameW(None, buffer, 260)
    path = buffer.value
    directory = os.path.splitdrive(os.path.dirname(path))[1] + "\\"
    name = os.path.splitext(os.path.basename(path))[0]
    return directory.encode("utf-16-le"), name.encode("utf-16-le")


def timesync(state, session, rounds):
    if state.get("timesync") is None:
        channel = Channel("timesync", state["timesync_url"], TIMESYNC_CHANNEL, 100)
        channel.open()
        channel.handshake()
        channel.start_reader()
        state["timesync"] = (channel, PacketReader(channel))
    channel, reader = state["timesync"]
    for _ in range(rounds):
        tag = session.next_timesync_tag()
        before = qpc_us()
        channel.send_packet(encode_timesync_request(tag))
        response = reader.response(tag, 1.0)
        after = qpc_us()
        emit("timesync", tag=tag, status=response.status, service_time=response.value,
             client_before_us=before, client_after_us=after,
             offset_us=response.value - (before + after) // 2)


def worker(args):
    if sys.platform != "win32" or C.sizeof(C.c_void_p) != 8:
        raise RuntimeError("requires 64-bit Windows Python")
    pipes = list_prp_pipes()
    emit("prp_pipes", names=pipes)
    if not pipes:
        raise RuntimeError("no TOBIIPRP- pipe listed")
    url = "tobii-prp://" + pipes[0][len(SERVER_PIPE_PREFIX):]
    session = ClientSession()
    command = Channel("command", url, COMMAND_CHANNEL, 1000)
    channels = [command]
    state = {}
    try:
        command.open()
        info = command.handshake()
        state["timesync_url"] = info.timesync_url
        subscription = Channel("subscription", info.subscription_url, SUBSCRIPTION_CHANNEL, 1000)
        channels.append(subscription)
        subscription.open()
        subscription_info = subscription.handshake()
        command.start_reader()
        subscription.start_reader()
        commands, packages = PacketReader(command), PacketReader(subscription)
        directory, name = image_components()
        tag = session.next_command_tag()
        command.send_packet(encode_connect_request(tag, ConnectRequest(
            subscription_info.client_id, process_id=os.getpid(), image_directory=directory,
            image_name=name, field_of_use=args.field_of_use)))
        connect = commands.response(tag, 5.0)
        emit("connect_response", status=connect.status, field_of_use=args.field_of_use,
             fields=connect.connect.__dict__ if connect.connect else None)
        if connect.status != 0:
            raise RuntimeError(f"connect status {connect.status}")
        if COMMAND_TIMESYNC in connect.connect.supported_commands:
            timesync(state, session, 4)
        if args.property:
            read_properties(command, commands, session, args.property)
            notifications(packages, 0.5)
        if args.subscribe_property:
            subscribe_properties(command, commands, session, args.subscribe_property,
                                 COMMAND_PROPERTY_SUBSCRIBE)
            notifications(packages, 1.0)
            subscribe_properties(command, commands, session, args.subscribe_property,
                                 COMMAND_PROPERTY_UNSUBSCRIBE)
        tag = session.next_command_tag()
        command.send_packet(encode_stream_request(tag, COMMAND_STREAM_START, HEAD_POSE_STREAM))
        start = commands.response(tag, 5.0)
        emit("stream_start_response", status=start.status, value=start.value)
        count = 0
        for received_at, packet in packages.packets(args.seconds):
            parsed = stream_package(packet)
            count += 1
            emit("stream_package", client_received_us=received_at, **parsed)
        if COMMAND_TIMESYNC in connect.connect.supported_commands:
            timesync(state, session, 4)
        tag = session.next_command_tag()
        command.send_packet(encode_stream_request(tag, COMMAND_STREAM_STOP, HEAD_POSE_STREAM))
        stop = commands.response(tag, 5.0)
        emit("stream_stop_response", status=stop.status, value=stop.value)
        late = sum(1 for _ in packages.packets(0.5))
        emit("summary", packages=count, packages_after_stop=late, seconds=args.seconds)
    finally:
        if state.get("timesync"):
            channels.append(state["timesync"][0])
        for channel in reversed(channels):
            channel.close()
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=float, default=3.0)
    parser.add_argument("--field-of-use", type=int, choices=(1, 2), default=2,
                        help="connect request field_of_use; 2 matches recorded use")
    parser.add_argument("--property", type=lambda text: int(text, 0), action="append",
                        default=[], help="property id to read with operation 5 (repeatable)")
    parser.add_argument("--subscribe-property", type=lambda text: int(text, 0), action="append",
                        default=[], help="property id to subscribe with command 5 and "
                                         "unsubscribe with command 6 afterwards (repeatable)")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 0 < args.seconds <= 30:
        parser.error("seconds must be in (0, 30]")
    if args.worker:
        try:
            return worker(args)
        except Exception as exc:
            emit("error", message=f"{type(exc).__name__}: {exc}")
            return 1
    command = [sys.executable, str(Path(__file__).resolve()), "--worker",
               "--seconds", str(args.seconds), "--field-of-use", str(args.field_of_use)]
    command += [f"--property={property_id}" for property_id in args.property]
    command += [f"--subscribe-property={property_id}" for property_id in args.subscribe_property]
    try:
        return subprocess.run(command, timeout=args.seconds + 30).returncode
    except subprocess.TimeoutExpired:
        emit("error", message="probe exceeded its deadline; child killed")
        return 124


if __name__ == "__main__":
    raise SystemExit(main())
