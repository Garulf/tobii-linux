#!/usr/bin/env python3
"""Compare the original stream-startup request builders with ttp_commands.

Runs each original builder of the hash-bound runtime in Unicorn and compares
the bytes with the native encoder. Also emulates the realm digest function
0x1402c0ef0 against HMAC-MD5 and records the expected-reply schema that
0x1402c3c50 assigns to each command. Synthetic inputs only; this is offline
emulation evidence, not a device observation.
"""
import json
import struct
from pathlib import Path

from unicorn.x86_const import UC_X86_REG_RAX, UC_X86_REG_XMM1
from vsmat_oracle import MatrixMachine
from tobii_linux import ttp_commands as tc
from tobii_linux.ttp_commands import Command, Stream

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe'
REPORT = ROOT/'evidence/private/ttp-commands-oracle.json'
CAPACITY = 4096
REQUEST_IDS = (1, 0x7f, 0x12345678, 0xffffffff)
REALM_DIGEST_FUNCTION = 0x1402c0ef0
REPLY_SCHEMA_FUNCTION = 0x1402c3c50

HEADER_ONLY_BUILDERS = {
    Command.ENUMERATE_STREAM_TYPES: 0x1402c7190,
    Command.GET_DEVICE_PROPERTIES: 0x1402c53e0,
    Command.GET_CAPABILITIES: 0x1402c5bd0,
    Command.GET_CAPABILITIES_ALL: 0x1402c5bf0,
    Command.GET_TRACK_BOX: 0x1402c61f0,
    Command.GET_DEVICE_INFO: 0x1402c6250,
    Command.GET_DISPLAY_AREA: 0x1402c6f20,
    Command.GET_DEVICE_STATUS: 0x1402c6230,
    Command.UNLOCK_PPU: 0x1402c6210,
    Command.GET_OUTPUT_FREQUENCY: 0x1402c5990,
    Command.ENUMERATE_OUTPUT_FREQUENCIES: 0x1402c5970,
    Command.GET_FREQUENCIES: 0x1402c5aa0,
    Command.ENUMERATE_FREQUENCIES: 0x1402c5a80,
    Command.GET_DEVICE_NAME: 0x1402c6870,
    Command.ENUMERATE_EXTENSIONS: 0x1402c5860,
    Command.ENUMERATE_ENABLED_EXTENSIONS: 0x1402c5880,
    Command.GET_LOW_BLINK_MODE: 0x1402c6790,
    Command.GET_ILLUMINATION_MODE: 0x1402c56c0,
    Command.ENUMERATE_ILLUMINATION_MODES: 0x1402c56a0,
    Command.GET_GEOMETRY_MOUNTING: 0x1402c5b90,
    Command.HARDWARE_CONFIGURATION_GET: 0x1402c5c10,
    Command.POWER_SAVE_ACTIVATE: 0x1402c6c40,
    Command.POWER_SAVE_DEACTIVATE: 0x1402c6c60,
    Command.GET_ENABLED_EYE: 0x1402c5220,
}


class Builders:
    def __init__(self, binary):
        self.binary = binary

    def run(self, function, *arguments, xmm1=None):
        machine = MatrixMachine(self.binary)
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
        if xmm1 is not None:
            machine.vm.reg_write(UC_X86_REG_XMM1, struct.unpack('<I', struct.pack('<f', xmm1))[0])
        machine.call(function, *resolved)
        length = machine.vm.reg_read(UC_X86_REG_RAX)
        if not 0 < length <= CAPACITY:
            raise RuntimeError(f'{function:#x} returned length {length:#x}')
        return bytes(machine.vm.mem_read(buffer, length))


def uint32_words(values):
    return b''.join(struct.pack('<I', value) for value in values)


def request_cases():
    for command, builder in HEADER_ONLY_BUILDERS.items():
        for request_id in REQUEST_IDS:
            yield (command.name, builder, request_id,
                   (request_id, 'buffer', CAPACITY, 0), {},
                   lambda r=request_id, c=command: tc.header_only_request(r, c))
    for stream in tc.WIRE_STREAM_IDS:
        for request_id in (1, 0xffffffff):
            yield ('SUBSCRIBE_STREAM', 0x1402c71b0, request_id,
                   (request_id, stream, 0, 0, 'buffer', CAPACITY), {'stream': stream},
                   lambda r=request_id, s=stream: tc.subscribe_stream(r, s))
            yield ('UNSUBSCRIBE_STREAM', 0x1402c72a0, request_id,
                   (request_id, stream, 'buffer', CAPACITY), {'stream': stream},
                   lambda r=request_id, s=stream: tc.unsubscribe_stream(r, s))
            yield ('ENUMERATE_STREAM_TYPE_COLUMNS', 0x1402c70b0, request_id,
                   (request_id, stream, 'buffer', CAPACITY, 0), {'stream': stream},
                   lambda r=request_id, s=stream: tc.enumerate_stream_type_columns(r, s))
    columns = (1, 2, 5, 0x10203040)
    yield ('SUBSCRIBE_STREAM', 0x1402c71b0, 9, (9, Stream.IMAGE, uint32_words(columns), len(columns),
           'buffer', CAPACITY), {'stream': 2, 'columns': list(columns)},
           lambda: tc.subscribe_stream(9, Stream.IMAGE, columns))
    for realm in (0x2712, 0x3e9, 0):
        for request_id in (1, 0xffffffff):
            yield ('OPEN_REALM_SALT', 0x1402c4600, request_id,
                   (request_id, realm, uint32_words((0,)), 1, 'buffer', CAPACITY, 0), {'realm': realm},
                   lambda r=request_id, m=realm: tc.open_realm_salt(r, m))
            yield ('CLOSE_REALM', 0x1402c4730, request_id,
                   (request_id, realm, 'buffer', CAPACITY), {'realm': realm},
                   lambda r=request_id, m=realm: tc.close_realm(r, m))
    digest = bytes(range(0x10, 0x20))
    for first, second in ((0, 0), (7, 0x12345678), (0xffffffff, 1)):
        yield ('OPEN_REALM_RESPONSE', 0x1402c47f0, 5,
               (5, first, second, digest, 16, 'buffer', CAPACITY), {'challenge': [first, second]},
               lambda a=first, b=second: tc.open_realm_response(5, (a, b), digest))
        for extra in (b'\x01\x02\x03\x04', b'\xaa' * 9):
            yield ('OPEN_REALM_RESPONSE_WITH_DATA', 0x1402c48f0, 6,
                   (6, first, second, digest, 16, extra, len(extra), 'buffer', CAPACITY),
                   {'challenge': [first, second], 'extra': extra.hex()},
                   lambda a=first, b=second, e=extra: tc.open_realm_response(6, (a, b), digest, e))
    for request_id in (1, 0xffffffff):
        yield ('PAUSE_DEVICE', 0x1402c6d80, request_id, (request_id, 1, 'buffer', CAPACITY),
               {'value': 1}, lambda r=request_id: tc.pause_device(r))
        yield ('PAUSE_DEVICE', 0x1402c6d80, request_id, (request_id, 0, 'buffer', CAPACITY),
               {'value': 0}, lambda r=request_id: tc.resume_device(r))
    for mode in ('', 'default', 'mode with spaces'):
        yield ('SET_ILLUMINATION_MODE', 0x1402c56e0, 3,
               (3, mode.encode() + b'\0', 'buffer', CAPACITY), {'mode': mode},
               lambda m=mode: tc.set_illumination_mode(3, m))
    for frequency in (0.0, 30.0, 33.333, 90.0, -1.5, 40000.0):
        yield ('SET_OUTPUT_FREQUENCY', 0x1402c59b0, 4, (4, 0, 'buffer', CAPACITY),
               {'frequency': frequency, 'xmm1': frequency},
               lambda f=frequency: tc.set_output_frequency(4, f))
    for first, second in ((0, 0), (1, 2), (0xffffffff, 0x80000000)):
        yield ('SET_FREQUENCIES', 0x1402c5ac0, 8, (8, second, first, 'buffer', CAPACITY),
               {'first': first, 'second': second},
               lambda a=first, b=second: tc.set_frequencies(8, a, b))


def realm_digest_cases(binary):
    cases = []
    inputs = ((bytes(range(16)), bytes(range(100, 116))),
              (b'k' * 16, b''),
              (b'short', bytes(range(40))),
              (bytes(range(80)), b'salt bytes beyond one block' * 4))
    for key, salt in inputs:
        machine = MatrixMachine(binary)
        key_pointer = machine.allocate(max(len(key), 1))
        salt_pointer = machine.allocate(max(len(salt), 1))
        output = machine.allocate(16)
        machine.vm.mem_write(key_pointer, key)
        machine.vm.mem_write(salt_pointer, salt)
        machine.call(REALM_DIGEST_FUNCTION, salt_pointer, len(salt), key_pointer, len(key), output)
        original = bytes(machine.vm.mem_read(output, 16))
        native = tc.realm_digest(key, salt)
        cases.append({'key_size': len(key), 'salt_size': len(salt), 'original': original.hex(),
                      'native': native.hex(), 'match': original == native})
    return cases


def reply_schema(binary, command):
    machine = MatrixMachine(binary)
    schema = machine.allocate(0x80)
    machine.vm.mem_write(schema, b'\xcc' * 0x80)
    machine.call(REPLY_SCHEMA_FUNCTION, command, schema)
    words = struct.unpack('<32I', machine.vm.mem_read(schema, 0x80))
    count = words[0]
    if count > 12:
        raise RuntimeError(f'Implausible reply schema count {count} for {command}')
    return {'count': count, 'columns': list(words[1:1 + count]), 'types': list(words[13:13 + count])}


def main():
    binary = BINARY.read_bytes()
    builders = Builders(binary)
    cases = []
    for name, builder, request_id, arguments, inputs, native in request_cases():
        xmm1 = inputs.pop('xmm1', None)
        expected = builders.run(builder, *arguments, xmm1=xmm1)
        actual = native().encode()
        cases.append({'command': name, 'builder': hex(builder), 'request_id': request_id,
                      'inputs': inputs, 'original': expected.hex(), 'native': actual.hex(),
                      'match': expected == actual})
    digests = realm_digest_cases(binary)
    schemas = {command.name: reply_schema(binary, command) for command in Command}
    passed = all(case['match'] for case in cases + digests)
    report = {'binary_sha256': '0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469',
              'evidence': 'offline Unicorn emulation of original functions with synthetic inputs',
              'passed': passed, 'request_cases': cases, 'realm_digest_cases': digests,
              'reply_schemas': schemas}
    REPORT.write_text(json.dumps(report, indent=2))
    failures = [case for case in cases + digests if not case['match']]
    for case in failures:
        print('MISMATCH', case)
    print(f'{len(cases)} request cases, {len(digests)} realm digest cases, '
          f'{len(failures)} mismatches, {len(schemas)} reply schemas')
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
