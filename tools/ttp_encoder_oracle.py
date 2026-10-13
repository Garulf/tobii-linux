#!/usr/bin/env python3
"""Offline differential check against one original x64 encoder function.

Requires optional Unicorn 2.1.4. The original executable is hash-bound and mapped
in an emulator. No Windows service, entry point, DLL loader or device is executed.
No imports are stubbed: an unimplemented call fails the check. This is supplemental
emulation evidence, not a REA result or hardware validation.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct

from unicorn import Uc, UC_ARCH_X86, UC_MODE_64, UC_HOOK_MEM_INVALID
from unicorn.x86_const import (UC_X86_REG_RAX, UC_X86_REG_RCX, UC_X86_REG_RDX,
                              UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RSP,
                              UC_X86_REG_RIP)
from tobii_linux.ttp import Message, uint32_array

SHA256 = "0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469"
ENTRY = 0x1402C6E40
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


def encode_original(binary, request_id, values):
    vm = machine(binary)
    source, output = DATA, DATA + 0x1000
    vm.mem_write(source, b"".join(struct.pack("<I", x) for x in values) or b"\0")
    rsp = STACK + 0xFFF8
    # Leave room for Windows x64 shadow space and two stack arguments.
    rsp -= 0x100
    vm.mem_write(rsp, struct.pack("<Q", STOP))
    vm.mem_write(rsp + 0x28, struct.pack("<Q", 4096))
    vm.mem_write(rsp + 0x30, struct.pack("<Q", 0))
    for register, value in ((UC_X86_REG_RCX, request_id), (UC_X86_REG_RDX, source),
                            (UC_X86_REG_R8, len(values)), (UC_X86_REG_R9, output),
                            (UC_X86_REG_RSP, rsp)):
        vm.reg_write(register, value)
    faults = []

    def invalid(vm, access, address, size, value, _):
        faults.append({"rip": hex(vm.reg_read(UC_X86_REG_RIP)), "address": hex(address),
                       "access": access, "size": size})
        return False

    vm.hook_add(UC_HOOK_MEM_INVALID, invalid)
    try:
        vm.emu_start(ENTRY, STOP, timeout=2_000_000, count=200_000)
    except Exception as error:
        raise RuntimeError(f"Original encoder emulation failed: {error}; {faults}") from error
    if vm.reg_read(UC_X86_REG_RIP) != STOP:
        raise RuntimeError("Encoder did not return within emulation budget")
    count = vm.reg_read(UC_X86_REG_RAX)
    if not 24 <= count <= 4096:
        raise RuntimeError(f"Invalid encoder result length {count}")
    return bytes(vm.mem_read(output, count))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.read_bytes()
    cases = [(1, tuple(range(0x10000, 0x10009))), (0, ()),
             (0xFFFFFFFF, (0, 0x01020304, 0xFFFFFFFF))]
    results = []
    for request_id, values in cases:
        actual = encode_original(binary, request_id, values)
        expected = Message(0x51, request_id, 0, 1000, 0,
                           b"\0\0" + uint32_array(values).encode()).encode()
        if actual != expected:
            raise AssertionError(f"Encoder mismatch: {actual.hex()} != {expected.hex()}")
        results.append({"request_id": request_id, "values": list(values),
                        "bytes": len(actual), "hex": actual.hex(), "matches": True})
    print(json.dumps({"kind": "supplemental-emulated-original-function",
                      "sha256": SHA256, "entry": hex(ENTRY),
                      "limitations": "No OS, USB or whole-program behavior validated",
                      "cases": results}, indent=2))


if __name__ == "__main__":
    main()
