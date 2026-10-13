#!/usr/bin/env python3
"""Offline differential oracle for the game DLL's PRP session messages.

Requires optional Unicorn. Executes only original serializer, decoder and
handshake-parser machine code from the hash-bound DLL in an emulator. No Windows
loader, imports, pipes, device or running game are involved, and no imported
function is stubbed. Native errors fail the check.
"""
import argparse
import json
from pathlib import Path
import struct
import sys

from unicorn.x86_const import (UC_X86_REG_RAX, UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8,
                               UC_X86_REG_R9, UC_X86_REG_RSP, UC_X86_REG_RIP)

sys.path.insert(0, str(Path(__file__).resolve().parent))
from prp_encoder_oracle import SHA256, STACK, STOP, machine  # noqa: E402
from tobii_linux.prp_session import (  # noqa: E402
    COMMAND_CONNECT, COMMAND_STREAM_START, COMMAND_STREAM_STOP, COMMAND_TIMESYNC,
    CommandResponse, ConnectRequest, ConnectResponse, HandshakeInfo, decode_command_request,
    decode_command_response, encode_command_response, encode_connect_request,
    encode_handshake_ack, encode_handshake_info, encode_stream_request, encode_timesync_request)

SERIALIZE = 0x18004F730
DECODE = 0x180053D10
HANDSHAKE_ACK_PARSER = 0x18004F300
HANDSHAKE_INFO_PARSER = 0x18004F360
COMMAND_CHANNEL_CALLBACK = 0x180020D80
SUBSCRIPTION_CHANNEL_CALLBACK = 0x1800217E0
CLIENT_VERSION = 0x7FFFFFFF
HEAP, HEAP_SIZE = 0x63000000, 0x80000
LOGGER, MESSAGE, OUTPUT, LENGTH, DATA = (HEAP, HEAP + 0x1000, HEAP + 0x10000,
                                          HEAP + 0x20000, HEAP + 0x30000)
CONNECT_DATA_SIZE = 0x130C


def vm_with_heap(binary):
    vm = machine(binary)
    vm.mem_map(HEAP, HEAP_SIZE)
    # Required nonnull logger context; successful paths never call it.
    vm.mem_write(LOGGER, struct.pack("<Q", 1))
    return vm


def call(vm, entry, rcx=0, rdx=0, r8=0, r9=0, stack=()):
    rsp = STACK + 0xFE00
    vm.mem_write(rsp, struct.pack("<Q", STOP))
    for index, value in enumerate(stack):
        vm.mem_write(rsp + 0x28 + 8 * index, struct.pack("<Q", value))
    for register, value in ((UC_X86_REG_RCX, rcx), (UC_X86_REG_RDX, rdx), (UC_X86_REG_R8, r8),
                            (UC_X86_REG_R9, r9), (UC_X86_REG_RSP, rsp)):
        vm.reg_write(register, value)
    vm.emu_start(entry, STOP, timeout=5_000_000, count=2_000_000)
    if vm.reg_read(UC_X86_REG_RIP) != STOP:
        raise RuntimeError("original function did not return")
    return vm.reg_read(UC_X86_REG_RAX)


class Arena:
    """Bump allocator for native buffers referenced by request pointers."""

    def __init__(self, vm):
        self.vm, self.next = vm, DATA

    def put(self, raw):
        if not raw:
            return 0
        address = self.next
        self.vm.mem_write(address, raw)
        self.next += len(raw) + 16 & ~15
        return address


def original_serialize(binary, native, version=CLIENT_VERSION, arena_fill=None):
    vm = vm_with_heap(binary)
    message = native(Arena(vm)) if arena_fill is None else arena_fill(vm)
    vm.mem_write(MESSAGE, message)
    status = call(vm, SERIALIZE, LOGGER, version, MESSAGE, 0x10000, (OUTPUT, LENGTH))
    if status & 0xFFFFFFFF:
        raise RuntimeError("original serializer failed with %d" % status)
    size = struct.unpack("<Q", vm.mem_read(LENGTH, 8))[0]
    return bytes(vm.mem_read(OUTPUT, size))


def original_decode(binary, packet, version=CLIENT_VERSION):
    vm = vm_with_heap(binary)
    vm.mem_write(DATA, packet)
    status = call(vm, DECODE, LOGGER, version, DATA, len(packet), (OUTPUT, LENGTH))
    if status & 0xFFFFFFFF:
        raise RuntimeError("original decoder rejected packet with %d" % status)
    if struct.unpack("<Q", vm.mem_read(LENGTH, 8))[0] != len(packet):
        raise RuntimeError("original decoder consumed an unexpected length")
    return vm, bytes(vm.mem_read(OUTPUT, 0x1400))


def deref(vm, pointer, length):
    return bytes(vm.mem_read(pointer, length)) if pointer and length else b""


# Native client requests --------------------------------------------------------------


def native_connect(tag, request):
    def build(arena):
        body = bytearray(0x1060)
        struct.pack_into("<i", body, 0, request.subscription_client_id)
        for index, license in enumerate(request.licenses):
            struct.pack_into("<QQ", body, 8 + 16 * index, arena.put(license), len(license))
        struct.pack_into("<I", body, 0x1008, len(request.licenses))
        struct.pack_into("<QQ", body, 0x1010, arena.put(request.signer_public_key),
                         len(request.signer_public_key))
        struct.pack_into("<4i", body, 0x1020, *request.client_version)
        struct.pack_into("<I", body, 0x1030, request.process_id)
        struct.pack_into("<QQ", body, 0x1038, arena.put(request.image_directory),
                         len(request.image_directory))
        struct.pack_into("<QQ", body, 0x1048, arena.put(request.image_name),
                         len(request.image_name))
        struct.pack_into("<i", body, 0x1058, request.field_of_use)
        return struct.pack("<IIII", tag, 9, COMMAND_CONNECT, 0) + bytes(body)
    return build


def native_simple(tag, operation, command, body):
    return lambda arena: struct.pack("<IIII", tag, operation, command, 0) + body


def check_connect_decode(binary, packet, request, version):
    vm, out = original_decode(binary, packet, version)
    tag, operation, command = struct.unpack_from("<III", out)
    body = out[0x10:]
    count = struct.unpack_from("<I", body, 0x1008)[0]
    licenses = tuple(deref(vm, *struct.unpack_from("<QQ", body, 8 + 16 * i)) for i in range(count))
    decoded = ConnectRequest(
        struct.unpack_from("<i", body, 0)[0], licenses,
        deref(vm, *struct.unpack_from("<QQ", body, 0x1010)),
        struct.unpack_from("<4i", body, 0x1020), struct.unpack_from("<I", body, 0x1030)[0],
        deref(vm, *struct.unpack_from("<QQ", body, 0x1038)),
        deref(vm, *struct.unpack_from("<QQ", body, 0x1048)),
        struct.unpack_from("<i", body, 0x1058)[0] if version > 0 else 0)
    expected = request if version > 0 else ConnectRequest(**{**request.__dict__, "field_of_use": 0})
    # The original decoder yields a null pointer for empty buffers, so compare contents.
    if (operation, command) != (9, COMMAND_CONNECT) or decoded != expected:
        raise RuntimeError("original decoder disagrees with connect request fields")
    return tag


# Native server responses -------------------------------------------------------------


def _text(raw, size):
    encoded = raw.encode("ascii")
    return encoded + bytes(size - len(encoded))


def native_connect_data(response):
    data = bytearray(CONNECT_DATA_SIZE)
    data[0:0x100] = _text(response.build_version, 0x100)
    data[0x100] = int(response.firmware_upgrade_in_progress)
    for offset, count_at, values in ((0x104, 0x168, response.list_a), (0x16C, 0x1D0, response.list_b),
                                     (0x1D4, 0x238, response.list_c), (0x23C, 0x2A0, response.list_d),
                                     (0x2A4, 0x324, response.streams),
                                     (0x328, 0x3AC, response.supported_commands),
                                     (0xE80, 0x1280, response.license_results),
                                     (0x1288, 0x1308, response.list_g)):
        struct.pack_into("<%di" % len(values), data, offset, *values)
        struct.pack_into("<i", data, count_at, len(values))
    for index, (kind, values) in enumerate(response.groups):
        base = 0x3B0 + 0x88 * index
        struct.pack_into("<i", data, base, kind)
        struct.pack_into("<%di" % len(values), data, base + 4, *values)
        struct.pack_into("<i", data, base + 0x84, len(values))
    struct.pack_into("<i", data, 0x658, len(response.groups))
    for offset, size, text in zip((0x65C, 0x75C, 0x85C, 0x95C, 0xA5C, 0xD5C, 0xDDC),
                                  (0x100,) * 5 + (0x80,) * 2, response.strings):
        data[offset:offset + size] = _text(text, size)
    for offset, text in zip((0xB5C, 0xBDC, 0xC5C, 0xCDC), response.extended_strings):
        data[offset:offset + 0x80] = _text(text, 0x80)
    struct.pack_into("<9i", data, 0xE5C, *response.words)
    struct.pack_into("<i", data, 0x1284, response.mode)
    return bytes(data)


def native_response(response, extended):
    if response.command == COMMAND_CONNECT:
        connect = response.connect
        if not extended:
            connect = ConnectResponse(**{**connect.__dict__, "extended_strings": ("",) * 4})
        data = native_connect_data(connect)
    elif response.command == COMMAND_TIMESYNC:
        data = struct.pack("<Q", response.value)
    else:
        data = struct.pack("<i", response.value)
    return struct.pack("<IIiI", response.tag, 10, response.status, response.command) + data


def check_response_decode(binary, packet, response, extended):
    version = CLIENT_VERSION if extended else 0
    _, out = original_decode(binary, packet, version)
    expected = native_response(response, extended)
    # Synthetic strings fit their client buffers, so the stored record must match
    # the native input byte for byte, including the whole connect record.
    actual = out[:len(expected)]
    if actual != expected:
        raise RuntimeError("original client decoder disagrees with %s response" % response.command)


# Handshake parsers -------------------------------------------------------------------


def original_handshake_info(binary, message, callback):
    vm = vm_with_heap(binary)
    context, user = HEAP + 0x2000, HEAP + 0x3000
    vm.mem_write(context, bytes(0x48))
    vm.mem_write(context + 0x38, struct.pack("<QQ", callback, user))
    vm.mem_write(user, struct.pack("<i", -1) + bytes(0x100))
    vm.mem_write(DATA, message)
    accepted = call(vm, HANDSHAKE_INFO_PARSER, DATA, len(message), context) & 0xFF
    entries = struct.unpack_from("<i", vm.mem_read(context + 0x30, 4))[0]
    return accepted, entries, bytes(vm.mem_read(user, 0x100))


def original_handshake_ack(binary, message):
    vm = vm_with_heap(binary)
    context, target = HEAP + 0x2000, HEAP + 0x3000
    vm.mem_write(context, struct.pack("<IIQQ", 1, 0, target, 4))
    vm.mem_write(target, bytes(8))
    vm.mem_write(DATA, message)
    accepted = call(vm, HANDSHAKE_ACK_PARSER, DATA, len(message), context) & 0xFF
    status = struct.unpack("<I", vm.mem_read(context, 4))[0]
    return accepted, status, struct.unpack("<I", vm.mem_read(target, 4))[0]


def cstring(raw, offset=0):
    return raw[offset:raw.index(b"\0", offset)].decode("ascii")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.read_bytes()
    cases = []

    requests = [
        (1, ConnectRequest(3, (b"license-key-one", b"", b"k2"), bytes(range(1, 71)), (4, 17, 0, 6),
                           0x1234, "C:\\Games\\StarCitizen\\".encode("utf-16-le"),
                           "StarCitizen".encode("utf-16-le"), 1)),
        (0x7FFFFFFF, ConnectRequest(-7, (), b"", (4, 17, 0, 6), 1, b"", b"", 2)),
    ]
    for tag, request in requests:
        for version in (CLIENT_VERSION, 0):
            extended = version > 0
            original = original_serialize(binary, native_connect(tag, request), version)
            ours = encode_connect_request(tag, request, extended=extended)
            if original != ours:
                raise RuntimeError("connect request bytes differ from original serializer")
            check_connect_decode(binary, ours, request, version)
            if decode_command_request(ours, extended=extended).connect != (
                    request if extended else ConnectRequest(**{**request.__dict__, "field_of_use": 0})):
                raise RuntimeError("Python decoder disagrees with connect request")
            cases.append(dict(kind="command-request", command=COMMAND_CONNECT, tag=tag,
                              version=version, bytes=len(ours), hex=ours.hex(), matches=True,
                              original_decoder_fields_match=True))

    for command, stream in ((COMMAND_STREAM_START, 2), (COMMAND_STREAM_STOP, 2),
                            (COMMAND_STREAM_START, 29)):
        original = original_serialize(binary, native_simple(9, 9, command, struct.pack("<i", stream)))
        ours = encode_stream_request(9, command, stream)
        if original != ours:
            raise RuntimeError("stream request bytes differ from original serializer")
        _, out = original_decode(binary, ours)
        if struct.unpack_from("<IIIIi", out) != (9, 9, command, 0, stream):
            raise RuntimeError("original decoder disagrees with stream request")
        cases.append(dict(kind="command-request", command=command, stream=stream, tag=9,
                          bytes=len(ours), hex=ours.hex(), matches=True,
                          original_decoder_fields_match=True))

    original = original_serialize(binary, native_simple(5, 9, COMMAND_TIMESYNC, b""))
    ours = encode_timesync_request(5)
    if original != ours:
        raise RuntimeError("timesync request bytes differ from original serializer")
    original_decode(binary, ours)
    cases.append(dict(kind="command-request", command=COMMAND_TIMESYNC, tag=5, bytes=len(ours),
                      hex=ours.hex(), matches=True, original_decoder_accepts=True))

    full = ConnectResponse(
        "1.2.3.4-test", False, (1, 2), (3,), (), (-4, 5, 6), (2, 3, 29), (0, 0x20, 7),
        ((0, (1, 2, 3)), (4, ())), ("serial", "model", "generation", "firmware", "integration",
                                    "x", "y"),
        tuple(range(9)), (0, 1, 10), 3, (8, 9), ("e0", "e1", "", "e3"))
    responses = [
        CommandResponse(1, COMMAND_CONNECT, 0, connect=full),
        CommandResponse(2, COMMAND_CONNECT, 0, connect=ConnectResponse("minimal", streams=(2,))),
        CommandResponse(3, COMMAND_CONNECT, 17, connect=ConnectResponse()),
        CommandResponse(4, COMMAND_STREAM_START, 0, 2),
        CommandResponse(5, COMMAND_STREAM_STOP, 5, 2),
        CommandResponse(6, COMMAND_TIMESYNC, 0, 0x0001020304050607),
    ]
    for response in responses:
        for extended in (True, False):
            version = CLIENT_VERSION if extended else 0
            original = original_serialize(binary, lambda arena: native_response(response, extended),
                                          version)
            ours = encode_command_response(response, extended=extended)
            if original != ours:
                raise RuntimeError("response bytes differ from original serializer")
            check_response_decode(binary, ours, response, extended)
            ours_decoded = decode_command_response(ours, extended=extended)
            expected = response
            if response.command == COMMAND_CONNECT and not extended:
                expected = CommandResponse(**{**response.__dict__, "connect": ConnectResponse(
                    **{**response.connect.__dict__, "extended_strings": ("",) * 4})})
            if ours_decoded != expected:
                raise RuntimeError("Python decoder disagrees with response")
            cases.append(dict(kind="command-response", command=response.command,
                              status=response.status, tag=response.tag, version=version,
                              bytes=len(ours), hex=ours.hex(), matches=True,
                              original_client_decoder_fields_match=True))

    accepted, status, value = original_handshake_ack(binary, encode_handshake_ack())
    if (accepted, status, value) != (1, 0, 1):
        raise RuntimeError("client rejects handshake acknowledgement")
    rejected = original_handshake_ack(binary, encode_handshake_ack() + b"\0")
    if rejected[0] != 0:
        raise RuntimeError("client accepted an oversized acknowledgement")
    cases.append(dict(kind="handshake-ack", hex=encode_handshake_ack().hex(), accepted=True,
                      five_byte_variant_accepted=False))

    info = HandshakeInfo(7, "tobii-prp://linux-bridge_subscription", "tobii-prp://linux-bridge_timesync")
    message = encode_handshake_info(info)
    accepted, entries, urls = original_handshake_info(binary, message, COMMAND_CHANNEL_CALLBACK)
    if (accepted, entries) != (1, 3) or (cstring(urls), cstring(urls, 0x80)) != (
            info.subscription_url, info.timesync_url):
        raise RuntimeError("command-channel callback did not receive both URLs")
    accepted, entries, identity = original_handshake_info(binary, message,
                                                          SUBSCRIPTION_CHANNEL_CALLBACK)
    if (accepted, entries) != (1, 3) or struct.unpack_from("<i", identity)[0] != info.client_id:
        raise RuntimeError("subscription-channel callback did not receive the client id")
    short = original_handshake_info(binary, message[:-len(info.timesync_url) - 1],
                                    SUBSCRIPTION_CHANNEL_CALLBACK)
    if short[0] != 0 or struct.unpack_from("<i", short[2])[0] != -1:
        raise RuntimeError("client accepted handshake information without a second string")
    cases.append(dict(kind="handshake-info", hex=message.hex(), accepted=True, entries=3,
                      command_callback_urls=[info.subscription_url, info.timesync_url],
                      subscription_callback_client_id=info.client_id,
                      missing_second_string_accepted=False))

    print(json.dumps(dict(
        kind="supplemental-emulated-original-function", sha256=SHA256,
        serializer_entry=hex(SERIALIZE), decoder_entry=hex(DECODE),
        handshake_entries=[hex(HANDSHAKE_ACK_PARSER), hex(HANDSHAKE_INFO_PARSER),
                           hex(COMMAND_CHANNEL_CALLBACK), hex(SUBSCRIPTION_CHANNEL_CALLBACK)],
        client_version_argument=hex(CLIENT_VERSION), cases=cases,
        limitations="Synthetic inputs only. No pipe I/O, Windows loader, service, device, "
                    "game or Wine behavior is exercised; server-side version choice is assumed."),
        indent=2))


if __name__ == "__main__":
    main()
