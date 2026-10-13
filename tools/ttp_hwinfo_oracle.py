#!/usr/bin/env python3
"""Compare the original hardware-info request builder with the native encoder."""
import json
from pathlib import Path
from unicorn.x86_const import UC_X86_REG_RAX
from vsmat_oracle import MatrixMachine
from tobii_linux.ttp import hardware_info_request
ROOT = Path(__file__).resolve().parents[1]
BUILDER = 0x1402c5c10
REQUEST_IDS = (0, 1, 0x7f, 0x12345678, 0xffffffff)


def original(binary, request_id):
    machine = MatrixMachine(binary)
    buffer = machine.allocate(4096)
    machine.call(BUILDER, request_id, buffer, 4096, 0)
    length = machine.vm.reg_read(UC_X86_REG_RAX)
    return bytes(machine.vm.mem_read(buffer, length))


def main():
    binary = (ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    cases = []
    for request_id in REQUEST_IDS:
        expected = original(binary, request_id)
        actual = hardware_info_request(request_id).encode()
        cases.append({'request_id': request_id, 'original': expected.hex(),
                      'native': actual.hex(), 'match': expected == actual})
    passed = all(case['match'] for case in cases)
    report = {'function': hex(BUILDER), 'passed': passed, 'cases': cases}
    (ROOT/'evidence/private/ttp-hwinfo-oracle.json').write_text(json.dumps(report, indent=2))
    for case in cases:
        print(hex(case['request_id']), case['original'], 'match' if case['match'] else 'MISMATCH')
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
