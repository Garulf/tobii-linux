#!/usr/bin/env python3
"""Offline differential oracle for the hash-bound game DLL PRP stream serializer.

Requires optional Unicorn. Executes only original serialization machine code in
an emulator, with no Windows loader, imports, device or running game involved.
No imported functions are stubbed. Native errors fail the check.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
from unicorn import Uc, UC_ARCH_X86, UC_MODE_64
from unicorn.x86_const import UC_X86_REG_RAX, UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RSP, UC_X86_REG_RIP
from tobii_linux.prp import GazePoint, HeadPose, encode_gaze_body, encode_head_body, encode_stream_packet
SHA256 = "35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020"
ENTRY = 0x180052E60
STACK, DATA, STOP = 0x60000000, 0x61000000, 0x62000000

def machine(binary):
    if hashlib.sha256(binary).hexdigest() != SHA256:
        raise ValueError("This oracle only supports the exact researched runtime")
    pe = struct.unpack_from("<I", binary, 0x3C)[0]
    if binary[pe:pe + 4] != b"PE\0\0":
        raise ValueError("Invalid PE")
    coff = pe + 4
    sections = struct.unpack_from("<H", binary, coff + 2)[0]
    optional_size = struct.unpack_from("<H", binary, coff + 16)[0]
    optional = coff + 20
    if struct.unpack_from("<H", binary, optional)[0] != 0x20B:
        raise ValueError("Expected PE32+")
    base = struct.unpack_from("<Q", binary, optional + 24)[0]
    size = struct.unpack_from("<I", binary, optional + 56)[0]
    vm = Uc(UC_ARCH_X86, UC_MODE_64)
    vm.mem_map(base, (size + 4095) & ~4095)
    for index in range(sections):
        off = optional + optional_size + index * 40
        _, rva, raw_size, raw_offset = struct.unpack_from("<IIII", binary, off + 8)
        if raw_size:
            vm.mem_write(base + rva, binary[raw_offset:raw_offset + raw_size])
    vm.mem_map(STACK, 0x10000)
    vm.mem_map(DATA, 0x10000)
    vm.mem_map(STOP, 0x1000)
    return vm


def original_body(binary, stream, native_input):
    vm = machine(binary)
    source, output, state = DATA, DATA + 0x1000, DATA + 0x2000
    vm.mem_write(source, native_input)
    vm.mem_write(state, struct.pack("<QIIII", output, 4096, 0, 0, 0))
    rsp = STACK + 0xFEF8
    vm.mem_write(rsp, struct.pack("<Q", STOP))
    vm.mem_write(rsp + 0x28, struct.pack("<Q", state))
    for register, value in ((UC_X86_REG_RCX, DATA + 0x3000),
                            (UC_X86_REG_RDX, 0), (UC_X86_REG_R8, stream),
                            (UC_X86_REG_R9, source), (UC_X86_REG_RSP, rsp)):
        vm.reg_write(register, value)
    vm.emu_start(ENTRY, STOP, timeout=2_000_000, count=200_000)
    if vm.reg_read(UC_X86_REG_RIP) != STOP or vm.reg_read(UC_X86_REG_RAX) != 0:
        raise RuntimeError("Original stream serializer failed")
    used = struct.unpack("<I", vm.mem_read(state + 12, 4))[0]
    size = struct.unpack("<I", vm.mem_read(output, 4))[0]
    if used != size + 4 or size not in (24, 52):
        raise RuntimeError("Unexpected body/extension length")
    return bytes(vm.mem_read(output, size))


def original_packet(binary, stream, native_input, message_tag):
    vm = machine(binary)
    source, output, length, logger = DATA, DATA + 0x1000, DATA + 0x2000, DATA + 0x3000
    vm.mem_write(source, struct.pack("<IIII", message_tag, 1, stream, 0) + native_input)
    # Required nonnull logger context; the successful serialization path never calls it.
    vm.mem_write(logger, struct.pack("<Q", 1))
    rsp = STACK + 0xFEF8
    vm.mem_write(rsp, struct.pack("<Q", STOP))
    vm.mem_write(rsp + 0x28, struct.pack("<QQ", output, length))
    for register, value in ((UC_X86_REG_RCX, logger), (UC_X86_REG_RDX, 0),
                            (UC_X86_REG_R8, source), (UC_X86_REG_R9, 4096),
                            (UC_X86_REG_RSP, rsp)):
        vm.reg_write(register, value)
    vm.emu_start(0x18004F730, STOP, timeout=2_000_000, count=200_000)
    if vm.reg_read(UC_X86_REG_RIP) != STOP or vm.reg_read(UC_X86_REG_RAX) != 0:
        raise RuntimeError("Original packet serializer failed")
    size = struct.unpack("<Q", vm.mem_read(length, 8))[0]
    if size not in (72, 100):
        raise RuntimeError("Unexpected complete packet length")
    return bytes(vm.mem_read(output, size))


def verify_original_decode(binary, packet, stream, native_input, message_tag):
    vm = machine(binary)
    output, consumed, logger = DATA + 0x4000, DATA + 0x2000, DATA + 0x3000
    vm.mem_write(DATA, packet)
    vm.mem_write(logger, struct.pack("<Q", 1))
    rsp = STACK + 0xFEF8
    vm.mem_write(rsp, struct.pack("<Q", STOP))
    vm.mem_write(rsp + 0x28, struct.pack("<QQ", output, consumed))
    for register, value in ((UC_X86_REG_RCX, logger), (UC_X86_REG_RDX, 0),
                            (UC_X86_REG_R8, DATA), (UC_X86_REG_R9, len(packet)),
                            (UC_X86_REG_RSP, rsp)):
        vm.reg_write(register, value)
    vm.emu_start(0x180053D10, STOP, timeout=2_000_000, count=200_000)
    if vm.reg_read(UC_X86_REG_RIP) != STOP or vm.reg_read(UC_X86_REG_RAX) != 0:
        raise RuntimeError("Original packet decoder rejected our packet")
    if struct.unpack("<Q", vm.mem_read(consumed, 8))[0] != len(packet):
        raise RuntimeError("Original decoder consumed an unexpected length")
    expected = struct.pack("<IIII", message_tag, 1, stream, 0) + native_input
    if bytes(vm.mem_read(output, len(expected))) != expected:
        raise RuntimeError("Original decoder produced different stream data")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.read_bytes()
    cases = []
    for stream in (3, 29):
        for sample in (GazePoint(0x1122334455667788, True, .5, .25),
                       GazePoint(-2, False, -1, 0)):
            source = struct.pack("<qB3xff", sample.timestamp, sample.valid, sample.x, sample.y)
            actual = original_body(binary, stream, source)
            assert actual == encode_gaze_body(sample), "Gaze serialization mismatch"
            packet = original_packet(binary, stream, source, 42)
            assert packet == encode_stream_packet(sample, message_tag=42, responsive=stream == 29)
            verify_original_decode(binary, packet, stream, source, 42)
            cases.append(dict(stream=stream, bytes=len(actual), hex=actual.hex(), matches=True,
                              packet_hex=packet.hex(), packet_matches=True, message_tag=42,
                              original_decoder_accepts=True))
    for sample in (HeadPose(-2, True, (1, 2, 3), (True, False, True), (-.25, .5, -1)),
                   HeadPose(0x1122334455667788, False, (0, -1, 9), (False, True, False), (0, 0, 0))):
        source = struct.pack("<qB3x3f3Bx3f", sample.timestamp, sample.position_valid,
                             *sample.position, *sample.rotation_valid, *sample.rotation)
        actual = original_body(binary, 2, source)
        assert actual == encode_head_body(sample), "Head serialization mismatch"
        packet = original_packet(binary, 2, source, 0xFFFFFFFF)
        assert packet == encode_stream_packet(sample, message_tag=0xFFFFFFFF)
        verify_original_decode(binary, packet, 2, source, 0xFFFFFFFF)
        cases.append(dict(stream=2, bytes=len(actual), hex=actual.hex(), matches=True,
                          packet_hex=packet.hex(), packet_matches=True, message_tag=0xFFFFFFFF,
                          original_decoder_accepts=True))
    print(json.dumps(dict(kind="supplemental-emulated-original-function", sha256=SHA256,
                         entry=hex(ENTRY), packet_entry="0x18004f730", decoder_entry="0x180053d10", cases=cases,
                         limitations="No whole-program, Windows transport, device or game behavior validated"), indent=2))


if __name__ == "__main__":
    main()
