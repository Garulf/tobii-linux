#!/usr/bin/env python3
"""Compare the calibration, persistent-file and configuration builders with gaze_device.

Runs each original request builder of the hash-bound runtime in Unicorn with
synthetic inputs and compares the bytes with the native encoder, then records
the reply schema 0x1402c3c50 assigns to each command. Offline emulation
evidence only; nothing here is sent to a device. See docs/gaze-device.md.
"""
import json
import struct
from pathlib import Path

from unicorn.x86_const import UC_X86_REG_RAX, UC_X86_REG_XMM1, UC_X86_REG_XMM2, UC_X86_REG_XMM3
from vsmat_oracle import MatrixMachine
from ttp_commands_oracle import BINARY, CAPACITY, reply_schema
from tobii_linux import gaze_device as gd
from tobii_linux.gaze_device import Eye, GazeCommand

ROOT = Path(__file__).resolve().parents[1]
REPORT = ROOT/'evidence/private/gaze-device-oracle.json'
XMM = (UC_X86_REG_XMM1, UC_X86_REG_XMM2, UC_X86_REG_XMM3)

HEADER_ONLY_BUILDERS = {
    GazeCommand.CALIBRATION_START: 0x1402c5120,
    GazeCommand.CALIBRATION_STOP: 0x1402c5200,
    GazeCommand.CALIBRATION_CLEAR: 0x1402c4dc0,
    GazeCommand.CALIBRATION_COMPUTE_AND_APPLY: 0x1402c4d80,
    GazeCommand.CALIBRATION_COMPUTE_AND_APPLY_PER_EYE: 0x1402c4da0,
    GazeCommand.CALIBRATION_RETRIEVE: 0x1402c4de0,
    GazeCommand.GET_CALIBRATION_STIMULUS_POINTS: 0x1402c5bb0,
    GazeCommand.PERSISTENT_FILE_ENUMERATE: 0x1402c6a20,
}
POINT_2D_BUILDERS = {
    GazeCommand.CALIBRATION_COLLECT_2D: (0x1402c4a20, gd.calibration_collect_2d),
    GazeCommand.CALIBRATION_COLLECT_PER_EYE_2D: (0x1402c4c70, gd.calibration_collect_per_eye_2d),
    GazeCommand.CALIBRATION_DISCARD_2D: (0x1402c4e00, gd.calibration_discard_2d),
}
POINT_3D_BUILDERS = {
    GazeCommand.CALIBRATION_COLLECT_3D: (0x1402c4b30, gd.calibration_collect_3d),
    GazeCommand.CALIBRATION_DISCARD_3D: (0x1402c4f10, gd.calibration_discard_3d),
}
UINT32_BUILDERS = {
    GazeCommand.CALIBRATION_START_EYE: (0x1402c5140, gd.calibration_start_eye),
    GazeCommand.SET_ENABLED_EYE: (0x1402c5240, gd.set_enabled_eye),
    GazeCommand.ENABLE_EXTENSION: (0x1402c57a0, gd.enable_extension),
}
NAME_BUILDERS = {
    GazeCommand.PERSISTENT_FILE_READ: (0x1402c6a40, gd.persistent_file_read),
    GazeCommand.PERSISTENT_FILE_ERASE: (0x1402c6950, gd.persistent_file_erase),
    GazeCommand.GET_CONFIGURATION_KEY: (0x1402c5300, gd.get_configuration_key),
}
POINTS = ((0.5, 0.5), (0.1, 0.9), (-0.25, 1.75), (0.333333, 0.666667), (12.5, -300.125))
POINTS_3D = ((0.0, 0.0, 600.0), (-296.62, 326.004, 111.8187), (1.5e-3, -2.25, 0.1))
NAMES = ('license.key', 'a', 'calibration/data.bin')


def run(function, arguments, floats=()):
    machine = MatrixMachine(run.binary)
    buffer = machine.allocate(CAPACITY)
    resolved = []
    for argument in arguments:
        if isinstance(argument, bytes):
            pointer = machine.allocate(max(len(argument), 1))
            machine.vm.mem_write(pointer, argument)
            argument = pointer
        elif argument == 'buffer':
            argument = buffer
        resolved.append(argument)
    for register, number in zip(XMM, floats):
        machine.vm.reg_write(register, struct.unpack('<I', struct.pack('<f', number))[0])
    machine.call(function, *resolved)
    length = machine.vm.reg_read(UC_X86_REG_RAX)
    if not 0 < length <= CAPACITY:
        raise RuntimeError(f'{function:#x} returned length {length:#x}')
    return bytes(machine.vm.mem_read(buffer, length))


def floats32(*numbers):
    return b''.join(struct.pack('<f', n) for n in numbers)


def cases():
    for command, builder in HEADER_ONLY_BUILDERS.items():
        for request_id in (1, 0x12345678, 0xffffffff):
            yield command.name, builder, {'request_id': request_id}, \
                (request_id, 'buffer', CAPACITY, 0), (), lambda r=request_id, c=command: gd.header_only(r, c)
    for command, (builder, encoder) in POINT_2D_BUILDERS.items():
        for x, y in POINTS:
            for eye in Eye:
                yield command.name, builder, {'point': [x, y], 'eye': int(eye)}, \
                    (7, 0, 0, int(eye), 'buffer', CAPACITY), (x, y), \
                    lambda a=x, b=y, e=eye, f=encoder: f(7, a, b, e)
    for command, (builder, encoder) in POINT_3D_BUILDERS.items():
        for x, y, z in POINTS_3D:
            for eye in Eye:
                yield command.name, builder, {'point': [x, y, z], 'eye': int(eye)}, \
                    (8, 0, 0, 0, int(eye), 'buffer', CAPACITY), (x, y, z), \
                    lambda a=x, b=y, c=z, e=eye, f=encoder: f(8, a, b, c, e)
    for command, (builder, encoder) in UINT32_BUILDERS.items():
        for value in (1, 2, 3) if command != GazeCommand.ENABLE_EXTENSION else (0, 9000, 0xffffffff):
            yield command.name, builder, {'value': value}, \
                (9, value, 'buffer', CAPACITY), (), lambda v=value, f=encoder: f(9, v)
    for command, (builder, encoder) in NAME_BUILDERS.items():
        for name in NAMES:
            yield command.name, builder, {'name': name}, \
                (10, name.encode() + b'\0', 'buffer', CAPACITY, 0), (), lambda n=name, f=encoder: f(10, n)
    for name in NAMES:
        for data in (b'\x01', bytes(range(200)), b'license text\n'):
            yield 'PERSISTENT_FILE_WRITE', 0x1402c6b40, {'name': name, 'size': len(data)}, \
                (11, name.encode() + b'\0', data, len(data), 'buffer', CAPACITY, 0), (), \
                lambda n=name, d=data: gd.persistent_file_write(11, n, d)
    for data in (b'\x00', bytes(range(256)) * 3, b'calibration'):
        yield 'CALIBRATION_APPLY', 0x1402c5050, {'size': len(data)}, \
            (12, data, len(data), 'buffer', CAPACITY), (), lambda d=data: gd.calibration_apply(12, d)
    areas = (((-296.62, 326.004, 111.8187), (300.38, 326.004, 111.8187), (-296.62, 10.2673, -3.10002)),
             ((0, 0, 0), (1, 2, 3), (-4.5, 5.25, -6.125)))
    for top_left, top_right, bottom_left in areas:
        yield 'SET_DISPLAY_AREA', 0x1402c6f40, {'points': [top_left, top_right, bottom_left]}, \
            (13, floats32(*top_left), floats32(*top_right), floats32(*bottom_left), 0, 0, 'buffer', CAPACITY), (), \
            lambda a=top_left, b=top_right, c=bottom_left: gd.set_display_area(13, a, b, c)


def main():
    run.binary = BINARY.read_bytes()
    results = []
    for name, builder, inputs, arguments, floats, native in cases():
        original = run(builder, arguments, floats)
        encoded = native().encode()
        results.append({'command': name, 'builder': hex(builder), 'inputs': inputs,
                        'original': original.hex(), 'native': encoded.hex(), 'match': original == encoded})
    schemas = {}
    for command in GazeCommand:
        try:
            schemas[command.name] = reply_schema(run.binary, command)
        except RuntimeError as error:
            schemas[command.name] = {'error': str(error)}
    failures = [case for case in results if not case['match']]
    REPORT.write_text(json.dumps({
        'binary_sha256': '0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469',
        'evidence': 'offline Unicorn emulation of original builders with synthetic inputs',
        'passed': not failures, 'request_cases': results, 'reply_schemas': schemas}, indent=2))
    for case in failures:
        print('MISMATCH', case['command'], case['inputs'], case['original'], case['native'])
    print(f'{len(results)} request cases, {len(failures)} mismatches, {len(schemas)} reply schemas')
    for name, schema in schemas.items():
        print(' ', name, schema)
    print('PASS' if not failures else 'FAIL')
    raise SystemExit(0 if not failures else 1)


if __name__ == '__main__':
    main()
