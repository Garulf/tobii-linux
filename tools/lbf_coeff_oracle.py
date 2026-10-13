#!/usr/bin/env python3
"""Check version-4 coefficient unpacking against the hash-bound native reader.

Emulates 0x1400ddbb0 float and int8 branches. Allocator,
fread, memset, free and pow are explicit host stubs. Original float truncation
code executes inside the emulator. Does not execute Windows/device code.
"""
import hashlib
import json
from pathlib import Path
import struct
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import *
from ttp_encoder_oracle import machine, DATA, STACK, STOP, SHA256
from tobii_linux.lbf import read_lbf, read_lv, quantize_matrix

ROOT = Path(__file__).resolve().parents[1]


def decode(binary, packed, inputs, outputs, has_bias=True, quantized=False):
    vm = machine(binary)
    heap = 0x63000000
    vm.mem_map(heap, 0x1000000)
    cursor = 0
    allocated = heap
    obj, matrix = DATA, DATA + 0x100
    bias, weights = heap, heap + 0x10000
    allocated = heap + 0x800000
    scales = heap + 0x700000
    vm.mem_write(obj + 0x20, struct.pack('<I', int(quantized)))
    vm.mem_write(matrix, struct.pack('<iiQQQQ', outputs, inputs, bias, 0 if quantized else weights, weights if quantized else 0, scales))
    rsp = STACK + 0xfeF8
    vm.mem_write(rsp, struct.pack('<Q', STOP))
    vm.mem_write(rsp + 0x28, struct.pack('<Q', int(has_bias)))
    for reg, val in ((UC_X86_REG_RCX, obj), (UC_X86_REG_RDX, 1),
                     (UC_X86_REG_R8, matrix), (UC_X86_REG_R9, 4), (UC_X86_REG_RSP, rsp)):
        vm.reg_write(reg, val)

    def hook(vm, address, size, _):
        nonlocal allocated, cursor
        rcx, rdx, r8 = [vm.reg_read(r) for r in (UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8)]
        result = 0
        if address == 0x1403b6808:
            result = allocated
            allocated += (rcx + 15) & ~15
            if allocated >= heap + 0x1000000:
                raise RuntimeError('Allocator bound exceeded')
        elif address == 0x1403e5adc:
            count = rdx * r8
            chunk = packed[cursor:cursor + count]
            if len(chunk) != count:
                raise RuntimeError('Original reader exceeded fixture')
            vm.mem_write(rcx, bytes(chunk))
            cursor += count
            result = r8
        elif address == 0x1403da6c0:
            vm.mem_write(rcx, bytes([rdx & 255]) * r8)
            result = rcx
        elif address == 0x140401540:
            a = struct.unpack('<d', vm.reg_read(UC_X86_REG_XMM0).to_bytes(16, 'little')[:8])[0]
            b = struct.unpack('<d', vm.reg_read(UC_X86_REG_XMM1).to_bytes(16, 'little')[:8])[0]
            vm.reg_write(UC_X86_REG_XMM0, int.from_bytes(struct.pack('<d', a ** b), 'little'))
        elif address != 0x1403b59c4:
            raise RuntimeError('Unexpected stub')
        stack = vm.reg_read(UC_X86_REG_RSP)
        ret = struct.unpack('<Q', vm.mem_read(stack, 8))[0]
        vm.reg_write(UC_X86_REG_RAX, result)
        vm.reg_write(UC_X86_REG_RSP, stack + 8)
        vm.reg_write(UC_X86_REG_RIP, ret)

    for address in (0x1403b6808, 0x1403e5adc, 0x1403da6c0, 0x140401540, 0x1403b59c4):
        vm.hook_add(UC_HOOK_CODE, hook, begin=address, end=address)
    vm.emu_start(0x1400ddbb0, STOP, timeout=10_000_000, count=10_000_000)
    if vm.reg_read(UC_X86_REG_RIP) != STOP or cursor != len(packed):
        raise RuntimeError('Original reader did not finish consuming fixture')
    result = (bytes(vm.mem_read(bias, outputs * 4)), bytes(vm.mem_read(weights, inputs * outputs * (1 if quantized else 4))))
    return (*result, bytes(vm.mem_read(scales, outputs * 4))) if quantized else result


def main():
    binary = (ROOT / 'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    cases = []
    for name in ('fa', 'fc'):
        path = ROOT / f'models/tobii-et5/support/bdtsdata/NN/{name}.lbf'
        model = read_lbf(path)
        matrix = model['stages'][0][0]
        inputs, outputs = matrix['shape']
        start = model['forest_end'] + 8
        # Original first column, with one output selected to keep emulation bounded.
        data = path.read_bytes()[start:start + 64 + (inputs + 2) // 2]
        bias, weights = decode(binary, data, inputs, 1)
        expected = b''.join(matrix['weights'][i * outputs * 4:i * outputs * 4 + 4] for i in range(inputs))
        assert bias == matrix['biases'][:4] and weights == expected
        cases.append({'source': name, 'inputs': inputs, 'outputs': 1, 'matches': True})
        quantized = decode(binary, data, inputs, 1, quantized=True)
        qweights, scales = quantize_matrix(matrix)
        expected_q = bytes(qweights[i * outputs] for i in range(inputs))
        assert quantized == (matrix['biases'][:4], expected_q, scales[:4])
        cases.append({'source': name, 'branch': 'int8', 'inputs': inputs, 'outputs': 1, 'matches': True})
    lv_path = ROOT / 'models/tobii-et5/support/bdtsdata/LBF/lv'
    lv = read_lv(lv_path)
    for record in lv['records']:
        inputs, outputs = record['matrix']['shape']
        start = record['matrix_offset'] + 8
        packed = lv_path.read_bytes()[start:start + outputs * (64 + (inputs + 1) // 2)]
        actual = decode(binary, packed, inputs, outputs, has_bias=False)
        assert actual == (record['matrix']['biases'], record['matrix']['weights'])
        cases.append({'source': 'lv', 'landmark': record['id'], 'matches': True})
    for inputs in (2, 7, 8, 9):
        outputs = 3
        packed = bytearray()
        bias_expected = bytearray()
        weights_expected = bytearray(inputs * outputs * 4)
        for col in range(outputs):
            table = [struct.pack('<f', (j - 8) * (col + 1) / 16) for j in range(16)]
            codes = [(j * 3 + col) % 16 for j in range(inputs + 1)]
            packed.extend(b''.join(table))
            packed.extend(bytes(codes[j] | ((codes[j + 1] if j + 1 < len(codes) else 0) << 4) for j in range(0, len(codes), 2)))
            bias_expected.extend(table[codes[0]])
            for j in range(inputs):
                off = (j * outputs + col) * 4
                weights_expected[off:off + 4] = table[codes[j + 1]]
        assert decode(binary, packed, inputs, outputs) == (bias_expected, weights_expected)
        cases.append({'source': 'synthetic', 'inputs': inputs, 'outputs': outputs, 'matches': True})
        qweights, scales = quantize_matrix({'shape': (inputs, outputs), 'weights': weights_expected,
                                            'codebook_max_abs': [0.5, 1.0, 1.5]})
        assert decode(binary, packed, inputs, outputs, quantized=True) == (bias_expected, qweights, scales)
        cases.append({'source': 'synthetic', 'branch': 'int8', 'inputs': inputs, 'outputs': outputs, 'matches': True})
    report = {'kind': 'supplemental-emulated-original-function', 'runtime_sha256': SHA256,
              'entry': '0x1400ddbb0', 'cases': cases,
              'limitations': 'Version-4 float unpacking and bounded int8 comparisons; explicit libc/pow stubs; no complete prediction parity.'}
    (ROOT / 'evidence/private/lbf-coeff-oracle.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
