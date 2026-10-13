#!/usr/bin/env python3
"""Compare the order-3 Euler matrix builder and its inverse with native code."""
import json
from pathlib import Path
import struct
import numpy as np
from vsmat_oracle import MatrixMachine
from tobii_linux.pose_output import euler_matrix, matrix_euler
ROOT = Path(__file__).resolve().parents[1]
BUILD = 0x1402b1c00
DECOMPOSE = 0x1402b1020
CASES = [
    (0.0, 0.0, 0.0),
    (0.3, 0.0, 0.0),
    (0.0, 0.3, 0.0),
    (0.0, 0.0, 0.3),
    (0.15045709, -0.15250771, 0.00541361),
    (-0.4, 0.7, -0.2),
    (0.2, -2.9, 1.1),
    (0.1, 3.0, -0.05),
    (1.4, 0.5, 0.3),
]


def original(binary, angles):
    m = MatrixMachine(binary)
    source = m.allocate(16)
    matrix = m.allocate(36)
    result = m.allocate(16)
    m.vm.mem_write(source, struct.pack('<fffI', *angles, 3))
    m.call(BUILD, matrix, source)
    built = m.floats(matrix, 9)
    m.call(DECOMPOSE, result, matrix)
    back = m.floats(result, 3)
    order = struct.unpack('<I', m.vm.mem_read(result + 12, 4))[0]
    return built, back, order


def main():
    binary = (ROOT/"evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe").read_bytes()
    cases = []
    for angles in CASES:
        angles = tuple(float(np.float32(a)) for a in angles)
        built, back, order = original(binary, angles)
        native_built = euler_matrix(angles).flatten()
        native_back = matrix_euler(np.asarray(built, dtype=np.float32).reshape(3, 3))
        cases.append({
            'angles': angles, 'original_matrix': built, 'original_angles': back,
            'order': order,
            'matrix_error': float(np.max(np.abs(np.asarray(built) - native_built))),
            'angles_error': float(np.max(np.abs(np.asarray(back) - native_back))),
        })
    passed = all(c['order'] == 3 and c['matrix_error'] <= 1e-6 and c['angles_error'] <= 1e-6 for c in cases)
    report = {'functions': [hex(BUILD), hex(DECOMPOSE)], 'passed': passed, 'cases': cases}
    (ROOT/'evidence/private/pose-euler-oracle.json').write_text(json.dumps(report, indent=2))
    for c in cases:
        print(c['angles'], 'matrix', f"{c['matrix_error']:.3g}", 'angles', f"{c['angles_error']:.3g}", c['original_angles'])
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
