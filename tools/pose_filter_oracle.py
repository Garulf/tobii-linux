#!/usr/bin/env python3
"""Compare the original head-pose filter stage 0x140295790 with HeadPoseFilter."""
import json
import math
from pathlib import Path
import struct
import numpy as np
from vsmat_oracle import MatrixMachine
from tobii_linux.pose_output import HeadPoseFilter, POSITION_FILTER, ROTATION_FILTER
ROOT = Path(__file__).resolve().parents[1]
STAGE = 0x140295790


def sequences():
    rng = np.random.default_rng(7)
    steady = [((0.15, -0.15, 0.01), (-55.0, -16.0, 530.0), 1_000_000 + 8333*i) for i in range(12)]
    moving = [((0.1*math.sin(i/3), 0.3*math.cos(i/4), 0.02*i), (-50 + 4*i, -15 + i, 520 + 3*i),
               5_000_000 + 8333*i) for i in range(20)]
    noisy = []
    t = 9_000_000
    for i in range(25):
        t += int(rng.integers(4000, 20000)) if i % 7 else 0
        noisy.append((tuple(rng.normal(0, 0.2, 3)), tuple(rng.normal((0, 0, 500), 20, 3)), t))
    reversed_time = [((0.1, 0.1, 0.1), (0, 0, 500), 2_000_000), ((0.2, 0.0, 0.1), (5, 0, 505), 1_990_000),
                     ((0.3, -0.1, 0.1), (9, 1, 510), 2_010_000)]
    return {'steady': steady, 'moving': moving, 'noisy_irregular': noisy, 'reversed_time': reversed_time}


def original(binary, samples):
    m = MatrixMachine(binary)
    stage = m.allocate(0x28)
    filters = []
    for _ in range(3):
        filters.append(m.allocate(0x30))
        m.vm.mem_write(filters[-1], struct.pack('<4f', *ROTATION_FILTER) + bytes(0x20))
    position = m.allocate(0x50)
    m.vm.mem_write(position, struct.pack('<4f', *POSITION_FILTER) + bytes(0x40))
    m.vm.mem_write(stage, struct.pack('<5Q', 0, *filters, position))
    source, output = m.allocate(0x20), m.allocate(0x20)
    results = []
    for rotation, translation, timestamp in samples:
        m.vm.mem_write(source, struct.pack('<4f3f', *rotation, 0, *translation))
        m.vm.mem_write(output, bytes(0x20))
        m.call(STAGE, stage, output, source, timestamp)
        values = m.floats(output, 7)
        results.append((values[:3], values[4:]))
    return results


def main():
    binary = (ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    report, passed = {'function': hex(STAGE), 'cases': {}}, True
    for name, samples in sequences().items():
        expected = original(binary, samples)
        native = HeadPoseFilter()
        rotation_error = position_error = 0.0
        exact = True
        for (rotation, translation, timestamp), (want_rotation, want_position) in zip(samples, expected):
            got_rotation, got_position = native(rotation, translation, timestamp)
            rotation_error = max(rotation_error, float(np.max(np.abs(got_rotation - want_rotation))))
            position_error = max(position_error, float(np.max(np.abs(got_position - want_position))))
            exact &= (got_rotation.tobytes() == np.asarray(want_rotation, np.float32).tobytes()
                      and got_position.tobytes() == np.asarray(want_position, np.float32).tobytes())
        ok = rotation_error <= 1e-6 and position_error <= 1e-4
        passed &= ok
        report['cases'][name] = {'samples': len(samples), 'byte_exact': exact,
                                 'rotation_error': rotation_error, 'position_error': position_error, 'passed': ok}
        print(name, len(samples), 'exact' if exact else '', f'rotation {rotation_error:.3g}',
              f'position {position_error:.3g}', 'ok' if ok else 'FAIL')
    report['passed'] = passed
    (ROOT/'evidence/private/pose-filter-oracle.json').write_text(json.dumps(report, indent=2))
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
