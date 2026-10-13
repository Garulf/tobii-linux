#!/usr/bin/env python3
"""Compare the original face finder 0x14007be00 with face_detect.detect_faces.

The network call is replaced by a hook returning synthetic outputs; the hook
also captures the letterboxed image the original passes to the network.
"""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn import UC_HOOK_CODE
from unicorn.x86_const import (UC_X86_REG_RAX, UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8,
                               UC_X86_REG_R9, UC_X86_REG_RIP, UC_X86_REG_RSP, UC_X86_REG_XMM3)
from ttp_encoder_oracle import STACK, STOP
from vsmat_oracle import MatrixMachine
from tobii_linux.face_detect import anchors, detect_faces
ROOT = Path(__file__).resolve().parents[1]
DETECT = 0x14007be00
CREATE_HEADER = 0x1400482e0
ALLOCATE_DATA = 0x1400464e0
HOOK = 0x67000000


def f32_bits(value):
    return struct.unpack('<I', struct.pack('<f', value))[0]


def synthetic_output(seed, faces):
    rng = np.random.default_rng(seed)
    count = len(anchors())
    offsets = rng.normal(0, 0.3, (count, 4)).astype(np.float32)
    scores = np.zeros((count, 2), dtype=np.float32)
    scores[:, 1] = rng.uniform(0, 0.2, count)
    scores[:, 0] = 1 - scores[:, 1]
    extra = rng.uniform(0, 1, (count, 3)).astype(np.float32)
    for index in rng.choice(np.arange(2304, count), faces, replace=False):
        scores[index, 1] = rng.uniform(0.3, 0.99)
        for neighbour in (index + 1, index + 2):
            if neighbour < count and rng.uniform() < 0.5:
                scores[neighbour, 1] = rng.uniform(0.26, 0.9)
    return np.concatenate((offsets.ravel(), scores.ravel(), extra.ravel())).astype(np.float32)


def original(binary, frame, output, region=None):
    m = MatrixMachine(binary)
    m.call(0x1400011b0)
    height, width = frame.shape
    m.call(CREATE_HEADER, width | height << 32, 8, 1)
    header = m.vm.reg_read(UC_X86_REG_RAX)
    m.call(ALLOCATE_DATA, header)
    data, step = struct.unpack('<Q', m.vm.mem_read(header + 0x58, 8))[0], struct.unpack('<i', m.vm.mem_read(header + 0x60, 4))[0]
    for row in range(height):
        m.vm.mem_write(data + row*step, frame[row].tobytes())
    network_output = m.allocate(output.nbytes)
    m.vm.mem_write(network_output, output.tobytes())
    m.vm.mem_map(HOOK, 0x1000)
    runner, table = m.allocate(16), m.allocate(16)
    m.vm.mem_write(runner, struct.pack('<Q', table))
    m.vm.mem_write(table, struct.pack('<QQ', HOOK, HOOK))
    captured = {}

    def network(vm, address, size, _):
        request = vm.reg_read(UC_X86_REG_RDX)
        pixels, dims = struct.unpack('<QQ', vm.mem_read(request, 16))
        columns, rows = dims & 0xffffffff, dims >> 32
        captured['image'] = np.frombuffer(bytes(vm.mem_read(pixels, columns*rows)), np.uint8).reshape(rows, columns)
        stack = vm.reg_read(UC_X86_REG_RSP)
        vm.reg_write(UC_X86_REG_RAX, network_output)
        vm.reg_write(UC_X86_REG_RIP, struct.unpack('<Q', vm.mem_read(stack, 8))[0])
        vm.reg_write(UC_X86_REG_RSP, stack + 8)
    m.vm.hook_add(UC_HOOK_CODE, network, begin=HOOK, end=HOOK)
    detector, boxes = m.allocate(0x40), m.allocate(0x18)
    m.vm.mem_write(detector, bytes(0x40))
    m.vm.mem_write(detector + 0x10, struct.pack('<Q', runner))
    m.vm.mem_write(boxes, bytes(0x18))
    region_pointer = 0
    if region is not None:
        region_pointer = m.allocate(16)
        m.vm.mem_write(region_pointer, struct.pack('<4i', *region))
    stack = STACK + 0xfef8
    m.vm.mem_write(stack, struct.pack('<Q', STOP))
    m.vm.mem_write(stack + 0x28, struct.pack('<QQQ', f32_bits(0.15000000596046448), f32_bits(1.0), region_pointer))
    for register, value in ((UC_X86_REG_RSP, stack), (UC_X86_REG_RCX, detector), (UC_X86_REG_RDX, header),
                            (UC_X86_REG_R8, boxes), (UC_X86_REG_XMM3, f32_bits(0.75))):
        m.vm.reg_write(register, value)
    m.vm.emu_start(DETECT, STOP, timeout=600_000_000, count=3_000_000_000)
    if m.vm.reg_read(UC_X86_REG_RIP) != STOP:
        raise RuntimeError('Detector did not return')
    begin, end = struct.unpack('<QQ', m.vm.mem_read(boxes, 16))
    found = [struct.unpack('<8f', m.vm.mem_read(address, 32)) for address in range(begin, end, 32)]
    return found, captured.get('image')


def main():
    binary = (ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    rng = np.random.default_rng(11)
    cases = [('landscape-4:3', 640, 480, 1, 6, None), ('wide-16:9', 320, 180, 2, 8, None),
             ('portrait', 120, 160, 3, 5, None), ('region', 640, 480, 4, 8, (100, 50, 400, 380))]
    report, passed = {'function': hex(DETECT), 'cases': []}, True
    for name, width, height, seed, faces, region in cases:
        frame = rng.integers(0, 256, (height, width), dtype=np.uint8)
        output = synthetic_output(seed, faces)
        expected, image = original(binary, frame, output, region)
        captured_image = {}

        def network(letterboxed):
            captured_image['image'] = letterboxed
            return output
        native = detect_faces(frame, network, region=region)
        got = [(d.x1, d.y1, d.x2, d.y2, d.score) + d.angles for d in native]
        image_exact = image is not None and np.array_equal(image, captured_image['image'])
        boxes_exact = len(got) == len(expected) and all(
            np.asarray(a, np.float32).tobytes() == np.asarray(b, np.float32).tobytes() for a, b in zip(got, expected))
        passed &= image_exact and boxes_exact
        report['cases'].append({'case': name, 'frame': [width, height], 'detections': len(expected),
                                'letterbox_byte_exact': bool(image_exact), 'boxes_byte_exact': bool(boxes_exact),
                                'original': [list(map(float, b)) for b in expected]})
        print(name, 'detections', len(expected), 'native', len(got), 'image', image_exact, 'boxes', boxes_exact, flush=True)
    report['passed'] = bool(passed)
    report['limitations'] = ('Synthetic network outputs through a hooked runner; the network itself is '
                             'not executed here. Allocator/free stubs and synthetic GS stack limit only.')
    (ROOT/'evidence/private/face-detect-oracle.json').write_text(json.dumps(report, indent=2) + '\n')
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
