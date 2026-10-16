#!/usr/bin/env python3
"""Supplemental offline emulation of original USB framing, not hardware evidence.

Only the named routines run. USB write is replaced by an argument-recording
stub; callback, mutex and transfer-result APIs are explicitly simulated. memcpy
is replaced with equivalent emulator memory copying. No service or device runs.
"""
import argparse
import json
from pathlib import Path
import struct

from unicorn import UC_HOOK_CODE
from unicorn.x86_const import (UC_X86_REG_RAX, UC_X86_REG_RCX, UC_X86_REG_RDX,
                              UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RSP,
                              UC_X86_REG_RIP)
from ttp_encoder_oracle import machine, DATA, STACK, STOP, SHA256
from tobii_linux.usb_framing import IncomingFrames, outgoing_frames

CALLBACK = STOP + 0x10
SOURCE = DATA + 0x20000


def execute(binary, entry, args, stack_args, handlers, initialize):
    vm = machine(binary)
    vm.mem_map(DATA + 0x10000, 0x30000)
    initialize(vm)
    rsp = STACK + 0xFEF8
    vm.mem_write(rsp, struct.pack('<Q', STOP))
    for index, value in enumerate(stack_args):
        vm.mem_write(rsp + 0x28 + index * 8, struct.pack('<Q', value))
    for register, value in zip((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8,
                                UC_X86_REG_R9), args):
        vm.reg_write(register, value)
    vm.reg_write(UC_X86_REG_RSP, rsp)

    def hook(vm, address, size, user):
        if address in handlers:
            result = handlers[address](vm)
            rsp = vm.reg_read(UC_X86_REG_RSP)
            ret = struct.unpack('<Q', vm.mem_read(rsp, 8))[0]
            vm.reg_write(UC_X86_REG_RAX, result or 0)
            vm.reg_write(UC_X86_REG_RSP, rsp + 8)
            vm.reg_write(UC_X86_REG_RIP, ret)
        elif address == 0x1403DB8A0:  # memcpy
            dest, src, count = [vm.reg_read(r) for r in
                (UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8)]
            vm.mem_write(dest, bytes(vm.mem_read(src, count)))
            rsp = vm.reg_read(UC_X86_REG_RSP)
            ret = struct.unpack('<Q', vm.mem_read(rsp, 8))[0]
            vm.reg_write(UC_X86_REG_RAX, dest)
            vm.reg_write(UC_X86_REG_RSP, rsp + 8)
            vm.reg_write(UC_X86_REG_RIP, ret)
        elif not (entry <= address < entry + 0x300 or address == STOP):
            raise RuntimeError(f'Unexpected execution address {address:#x}')

    vm.hook_add(UC_HOOK_CODE, hook)
    vm.emu_start(entry, STOP, timeout=2_000_000, count=300_000)
    if vm.reg_read(UC_X86_REG_RIP) != STOP:
        raise RuntimeError('Emulation budget exceeded')


def transmit(binary, channel, payload):
    writes = []
    def initialize(vm):
        slot = DATA + 0x70 + channel * 0x20
        vm.mem_write(slot + 0x10, b'\x01')
        vm.mem_write(slot + 0x14, struct.pack('<IH', 100, channel))
        vm.mem_write(SOURCE, payload or b'\0')
    def write(vm):
        ep, buf, size = [vm.reg_read(r) for r in
            (UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9)]
        writes.append((ep & 0xff, bytes(vm.mem_read(buf, size))))
        return 0
    execute(binary, 0x1402D3490, [DATA, 100, SOURCE, len(payload)], [0, CALLBACK],
            {0x1403B44D0: write, CALLBACK: lambda vm: 0}, initialize)
    return writes


def receive(binary, wire):
    callbacks = []
    def initialize(vm):
        for channel in range(12):
            slot = DATA + 0x70 + channel * 0x20
            vm.mem_write(slot, struct.pack('<QQBI', CALLBACK, channel, 1, 0))
            vm.mem_write(slot + 0x14, struct.pack('<I', channel + 100))
        vm.mem_write(SOURCE, wire)
    def transfer_result(vm):
        target = vm.reg_read(UC_X86_REG_RDX)
        vm.mem_write(target, struct.pack('<QII', SOURCE, len(wire), 0))
        return 0
    def callback(vm):
        event = vm.reg_read(UC_X86_REG_RCX)
        channel, data, length = struct.unpack('<QQI', vm.mem_read(event + 16, 20))
        callbacks.append((channel, bytes(vm.mem_read(data, length))))
    execute(binary, 0x1402D3740, [DATA, 0, 0, 0], [],
        {0x1403B5220: transfer_result, 0x1403B3C90: lambda vm: 0,
         0x1403B3CA0: lambda vm: 0, CALLBACK: callback}, initialize)
    return callbacks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    binary = parser.parse_args().binary.read_bytes()
    results = []
    for channel, size in ((0, 1), (7, 512), (11, 4087), (2, 4088), (3, 8192), (0, 0)):
        payload = bytes(i % 251 for i in range(size))
        writes = transmit(binary, channel, payload)
        expected = [(5, frame) for frame in outgoing_frames(channel, payload)]
        assert writes == expected
        results.append({'direction': 'out', 'channel': channel, 'payload_bytes': size,
                        'transfer_lengths': [len(w) for _, w in writes],
                        'headers': [w[:8].hex() for _, w in writes], 'matches': True})
    for mask, payload in ((1, b'abc'), (0x805, b'hello'), (0, b'discard'), (0xfff, b'x')):
        wire = struct.pack('<II', mask, len(payload) + 8) + payload
        actual = receive(binary, wire)
        frames = IncomingFrames().feed(wire)
        expected = [(channel, frame.payload) for frame in frames for channel in range(12) if frame.for_channel(channel)]
        assert actual == expected, (actual, expected)
        results.append({'direction': 'in', 'mask': mask, 'payload_hex': payload.hex(),
                        'channels': [channel for channel, _ in actual], 'matches': True})
    print(json.dumps({'kind': 'supplemental-emulated-original-function', 'sha256': SHA256,
        'entries': ['0x1402d3490', '0x1402d3740'],
        'limitations': 'Simulated USB I/O, callbacks, mutexes, memcpy and transfer results; no hardware validation',
        'cases': results}, indent=2))


if __name__ == '__main__':
    main()
