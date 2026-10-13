#!/usr/bin/env python3
"""Compare tracker working-view preparation 0x140079ce0 with face_tracking.working_view."""
import json
from pathlib import Path
import struct
import numpy as np
from unicorn.x86_const import UC_X86_REG_RAX
from vsmat_oracle import MatrixMachine
from tobii_linux.face_tracking import expanded_region, working_view
ROOT = Path(__file__).resolve().parents[1]
PREPARE = 0x140079ce0
CREATE_HEADER = 0x1400482e0
ALLOCATE_DATA = 0x1400464e0
CREATE_RECORDS = 0x140010140


def image(m, width, height, pixels=None):
    m.call(CREATE_HEADER, width | height << 32, 8, 1)
    header = m.vm.reg_read(UC_X86_REG_RAX)
    m.call(ALLOCATE_DATA, header)
    if pixels is not None:
        data, step = struct.unpack('<Q', m.vm.mem_read(header + 0x58, 8))[0], struct.unpack('<i', m.vm.mem_read(header + 0x60, 4))[0]
        for row in range(height):
            m.vm.mem_write(data + row*step, pixels[row].tobytes())
    return header


def read_image(m, header):
    width, height = struct.unpack('<ii', m.vm.mem_read(header + 0x28, 8))
    data, step = struct.unpack('<Q', m.vm.mem_read(header + 0x58, 8))[0], struct.unpack('<i', m.vm.mem_read(header + 0x60, 4))[0]
    rows = [np.frombuffer(bytes(m.vm.mem_read(data + r*step, width)), np.uint8) for r in range(height)]
    return np.stack(rows)


def original(binary, frame, face):
    m = MatrixMachine(binary)
    m.call(0x1400011b0)
    height, width = frame.shape
    source = image(m, width, height, frame)
    working = image(m, 240, 240)
    records = m.allocate(0x1a8)
    m.call(CREATE_RECORDS, records)
    source_records = m.allocate(0x1a8)
    m.call(CREATE_RECORDS, source_records)
    state = m.allocate(0x58)
    m.vm.mem_write(state, struct.pack('<QQQ', 0, working, records) + b'\xff' * 0x20
                   + struct.pack('<if', 96, 2.5) + bytes(0x18))
    rectangle = m.allocate(16)
    m.vm.mem_write(rectangle, struct.pack('<4i', *face))
    m.call(PREPARE, state, source, source_records, rectangle,
           instruction_limit=3_000_000_000, timeout_us=900_000_000)
    fields = struct.unpack('<8i', m.vm.mem_read(state + 0x18, 32))
    adjustments = struct.unpack('<QQ', m.vm.mem_read(state + 0x40, 16))
    crop = struct.unpack('<Q', m.vm.mem_read(adjustments[0], 8))[0]
    region = struct.unpack('<4i', m.vm.mem_read(crop + 0x20, 16))
    return read_image(m, working), fields[:4], region


def main():
    binary = (ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    rng = np.random.default_rng(5)
    frame = rng.integers(0, 256, (240, 320), dtype=np.uint8)
    faces = [(120, 80, 60, 60), (10, 5, 70, 70), (250, 170, 64, 64), (130, 90, 97, 97), (100, 60, 41, 41)]
    report, passed = {'function': hex(PREPARE), 'cases': []}, True
    for face in faces:
        expected_image, expected_face, expected_region = original(binary, frame, face)
        view = working_view(frame, face)
        region = expanded_region(face, frame.shape[1], frame.shape[0])
        ok = (np.array_equal(view.image, expected_image) and tuple(view.face) == tuple(expected_face)
              and tuple(region) == tuple(expected_region))
        passed &= ok
        report['cases'].append({'face': face, 'working_size': list(expected_image.shape[::-1]),
                                'face_in_working': list(expected_face), 'region': list(expected_region),
                                'byte_exact': bool(ok)})
        print(face, 'working', expected_image.shape[::-1], 'face', expected_face, 'region', expected_region,
              'ok' if ok else 'MISMATCH', view.face, region, view.image.shape[::-1], flush=True)
    report['passed'] = bool(passed)
    report['limitations'] = 'Synthetic frame; allocator/free stubs and synthetic GS stack limit only.'
    (ROOT/'evidence/private/face-tracking-oracle.json').write_text(json.dumps(report, indent=2) + '\n')
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
