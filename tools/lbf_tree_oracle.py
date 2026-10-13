#!/usr/bin/env python3
"""Offline original-tree traversal comparison; no import stubs or OS execution."""
import json
from pathlib import Path
import struct
from unicorn.x86_const import *
from ttp_encoder_oracle import machine, DATA, STACK, STOP, SHA256
from tobii_linux.lbf import read_lbf, read_lv
from tobii_linux.lbf_features import forest_leaves

ROOT = Path(__file__).resolve().parents[1]


def original(binary, forest, pixels, rows, columns, stride, center0, center1, scale, sign):
    vm = machine(binary)
    heap = 0x63000000
    vm.mem_map(heap, 0x400000)
    obj, output, image = DATA, DATA + 0x1000, heap + 0x300000
    vm.mem_write(image, pixels)
    trees = forest[2]
    vm.mem_write(obj, struct.pack('<iiQQ', forest[1], forest[0], len(trees), heap))
    cursor = heap + 0x10000
    for i, (_, word, depth, payload) in enumerate(trees):
        threshold = cursor + len(payload)
        vm.mem_write(heap + i * 32, struct.pack('<iiQQQ', word, depth, cursor, threshold, 0))
        vm.mem_write(cursor, payload)
        cursor = (threshold + (1 << depth) + 15) & ~15
    rsp = STACK + 0xfef8
    vm.mem_write(rsp, struct.pack('<Q', STOP))
    vm.mem_write(rsp + 0x28, struct.pack('<f', scale))
    for offset, value in [(0x30, image), (0x38, rows), (0x40, columns), (0x48, stride), (0x50, sign & 0xffffffff)]:
        vm.mem_write(rsp + offset, struct.pack('<Q', value))
    for reg, value in [(UC_X86_REG_RCX, output), (UC_X86_REG_RDX, obj), (UC_X86_REG_RSP, rsp),
                       (UC_X86_REG_XMM2, int.from_bytes(struct.pack('<f', center0), 'little')),
                       (UC_X86_REG_XMM3, int.from_bytes(struct.pack('<f', center1), 'little'))]:
        vm.reg_write(reg, value)
    vm.emu_start(0x1400e1970, STOP, timeout=5_000_000, count=1000000)
    if vm.reg_read(UC_X86_REG_RIP) != STOP:
        raise RuntimeError('Tree traversal did not return')
    return struct.unpack('<' + 'i' * len(trees), vm.mem_read(output, len(trees) * 4))


def main():
    binary = (ROOT / 'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    support = ROOT / 'models/tobii-et5/support/bdtsdata'
    forests = {}
    for name in ('fa', 'fc'):
        model = read_lbf(support / f'NN/{name}.lbf')
        forests[name] = next(p['submodels'][0] for p in model['points'] if p['submodels'])
    forests['lv'] = read_lv(support / 'LBF/lv')['records'][0]['forest']
    rows, columns, stride = 96, 128, 132
    pixels = bytes((i * 73 + (i // stride) * 39 + i * i) & 255 for i in range(rows * stride))
    cases = []
    for name, forest in forests.items():
        for c0, c1, scale, sign in [(48.9, 64.2, 48.8, 1), (48, 64, 48, -1),
                                    (0, 0, 64, 1), (95, 127, 96, -1),
                                    (-3.7, 130.2, 80, 1), (48, 64, 0, 1)]:
            expected = forest_leaves(forest, pixels, rows, columns, stride, c0, c1, scale, sign)
            actual = original(binary, forest, pixels, rows, columns, stride, c0, c1, scale, sign)
            assert actual == expected, (name, c0, c1, actual[:5], expected[:5])
            cases.append({'model': name, 'center': [c0, c1], 'scale': scale, 'axis1_sign': sign,
                          'trees': len(actual), 'matches': True})
    report = {'runtime_sha256': SHA256, 'entry': '0x1400e1970', 'cases': cases,
              'limitations': 'Synthetic grayscale buffer; selected installed forests; zero thresholds as loaded; no crop, predictor or pose parity.'}
    (ROOT / 'evidence/private/lbf-tree-oracle.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
