#!/usr/bin/env python3
"""Compare the original TTP response parser with ttp_fields and camera_info.

Offline Unicorn emulation of hash-bound runtime functions; no service, USB or
device is involved. Every input is a synthetic vector built from the recovered
instructions, not recorded device traffic.
"""
import json
from pathlib import Path
import random
import struct
import sys

from unicorn import UC_HOOK_CODE
from unicorn.x86_const import (UC_X86_REG_R8, UC_X86_REG_RAX, UC_X86_REG_RDX, UC_X86_REG_RIP,
                              UC_X86_REG_RSP, UC_X86_REG_XMM1)
from vsmat_oracle import MatrixMachine
from tobii_linux.camera_info import (CameraRecord, HardwareInfo, hardware_info_response,
                                     read_hardware_info, camera_fields)
from tobii_linux.ttp import Decoder, Field, Message, ProtocolError
from tobii_linux import ttp_fields as tf

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = 0x1402C3C50
PARSER_INIT, PARSER_FEED, PARSER_NEXT = 0x1402CFA50, 0x1402CF8C0, 0x1402CFDE0
SCHEMA_CHECK = 0x1402C18B0
LOG, TYPE_MISMATCH = 0x1402C7360, 0x14000FFB0
PARSER_BLOCK = 0x5740
FIELD_SIZE = 0x208
WRITERS = {'int32': 0x1403B0D10, 'uint32': 0x1403B0E00, 'fixed16': 0x1403B0BD0,
           'fixed42': 0x1403B0C10, 'struct': 0x1403B0CD0, 'int64': 0x1403B0D50,
           'array_header': 0x1403B1D70, 'fixed42_array': 0x1403B0C50,
           'string': 0x1403B0D90, 'blob': 0x1403B0B40}
WIRE_OF = {'int32': 1, 'uint32': 2, 'fixed16': 3, 'fixed42': 4, 'int64': 6, 'uint64': 7,
           'string': 0x14, 'blob': 0x15, 'fixed16_array': 0x18, 'fixed42_array': 0x19,
           'string_array': 0x1A}
READERS = {'int32': 0x1403B0780, 'uint32': 0x1403B0A40, 'fixed16': 0x1403B0400,
           'fixed42': 0x1403B0580, 'int64': 0x1403B0800, 'uint64': 0x1403B0AC0,
           'string': 0x1403B0880, 'blob': 0x1403B0350, 'fixed16_array': 0x1403B0480,
           'fixed42_array': 0x1403B0600, 'string_array': 0x1403B0940}


class Machine(MatrixMachine):
    def __init__(self, binary):
        super().__init__(binary)
        self.events = []
        for address in (LOG, TYPE_MISMATCH):
            self.vm.hook_add(UC_HOOK_CODE, self.record, begin=address, end=address)

    def record(self, vm, address, size, _):
        stack = vm.reg_read(UC_X86_REG_RSP)
        if address == LOG:
            line, code = struct.unpack('<QQ', vm.mem_read(stack + 0x30, 8)
                                       + vm.mem_read(stack + 0x40, 8))
            self.events.append({'log_line': hex(line & 0xFFFFFFFF), 'code': code & 0xFFFFFFFF})
        else:
            self.events.append({'expected_wire': vm.reg_read(UC_X86_REG_RDX) & 0xFF,
                                'got_wire': vm.reg_read(UC_X86_REG_R8) & 0xFF})
        ret = struct.unpack('<Q', vm.mem_read(stack, 8))[0]
        vm.reg_write(UC_X86_REG_RAX, 0)
        vm.reg_write(UC_X86_REG_RSP, stack + 8)
        vm.reg_write(UC_X86_REG_RIP, ret)

    def result(self):
        return self.vm.reg_read(UC_X86_REG_RAX)

    def read(self, address, size):
        return bytes(self.vm.mem_read(address, size))

    def put(self, data):
        address = self.allocate(max(len(data), 1))
        self.vm.mem_write(address, data)
        return address

    def stream(self, size):
        """Writer/reader cursor as initialized by 0x1403afdf0."""
        buffer = self.allocate(size)
        cursor = self.allocate(0x20)
        self.vm.mem_write(cursor, struct.pack('<IIIIIIQ', size, size, 0, 0, 0, 0, buffer))
        return cursor, buffer


def schema_table(binary, commands):
    machine = Machine(binary)
    out = machine.allocate(0x100)
    table = {}
    for command in range(commands):
        machine.vm.mem_write(out, b'\xcc' * 0x100)
        machine.call(SCHEMA, command, out)
        raw = machine.read(out, 0x64)
        count = struct.unpack_from('<I', raw)[0]
        if count:
            table[command] = (struct.unpack_from(f'<{count}I', raw, 0x34),
                              struct.unpack_from(f'<{count}I', raw, 4))
    return table


def parse_stream(binary, chunks):
    machine = Machine(binary)
    block, holder = machine.allocate(PARSER_BLOCK), machine.allocate(8)
    machine.call(PARSER_INIT, holder, block, PARSER_BLOCK, 0, 0, 0, 0, 0)
    if machine.result() & 0xFFFFFFFF:
        raise RuntimeError('Parser initialization failed')
    results = []
    for chunk in chunks:
        data = machine.put(chunk)
        machine.call(PARSER_FEED, block, data, len(chunk))
        while True:
            message = machine.allocate(0x20)
            machine.events = []
            machine.call(PARSER_NEXT, block, message)
            status = machine.result() & 0xFFFFFFFF
            if status == 1:
                break
            request_id, kind, _, _, count, fields = struct.unpack('<IIIIi4xQ', machine.read(message, 0x20))
            machine.call(SCHEMA_CHECK, message, 0x1B, 0x0C, 2)
            results.append({'status': status, 'request_id': request_id, 'kind': kind,
                            'count': count, 'fields': fields, 'events': machine.events,
                            'hardware_schema_check': machine.result() & 0xFFFFFFFF})
            decodable = status == 0 and count == 3 and results[-1]['hardware_schema_check'] == 0
            results[-1]['decoded'] = decoded_hardware_info(machine, fields) if decodable else None
    return results


def triple(raw, offset, form='<3d'):
    return struct.unpack_from(form, raw, offset)


def decoded_hardware_info(machine, fields):
    raw = [machine.read(fields + i * FIELD_SIZE, FIELD_SIZE) for i in range(3)]
    tags = tuple(struct.unpack_from('<I', r)[0] for r in raw)
    count, cameras_at = struct.unpack_from('<i4xQ', raw[0], 8)
    cameras = []
    for index in range(count):
        record = machine.read(cameras_at + index * 0x2E8, 0x2E8)
        series = struct.unpack_from('<i', record, 0xAC)[0]
        cameras.append(CameraRecord(
            struct.unpack_from('<i', record)[0], *struct.unpack_from('<2f', record, 4),
            triple(record, 0x10), struct.unpack_from('<15d', record, 0x28),
            *struct.unpack_from('<3i', record, 0xA0),
            struct.unpack_from(f'<{series}d', record, 0xB0),
            triple(record, 0x2B0), triple(record, 0x2C8), struct.unpack_from('<d', record, 0x2E0)[0]))
    points_count, points_at = struct.unpack_from('<i4xQ', raw[1], 8)
    points = tuple(struct.unpack_from('<3f', machine.read(points_at + 12 * i, 12)) for i in range(points_count))
    scalar = struct.unpack_from('<i', raw[2], 8)[0]
    return tags, HardwareInfo(tuple(cameras), points, scalar)


def camera(seed, series_length=8):
    rng = random.Random(seed)
    value = lambda scale: rng.uniform(-scale, scale)
    return CameraRecord(
        rng.randrange(-2**31, 2**31), value(30000.0), value(30000.0),
        tuple(value(5000.0) for _ in range(3)), tuple(value(2.0e6) for _ in range(15)),
        rng.randrange(0, 4096), rng.randrange(0, 4096), rng.randrange(-5, 300),
        tuple(value(100.0) for _ in range(series_length)),
        tuple(value(1.0e6) for _ in range(3)), tuple(value(1.0) for _ in range(3)), value(1.0e5))


def points(seed, count):
    rng = random.Random(seed)
    return tuple(tuple(rng.uniform(-1000.0, 1000.0) for _ in range(3)) for _ in range(count))


def hardware_cases():
    long_series = camera(9, 70)
    return {
        'one_camera': HardwareInfo((camera(1),), points(1, 1), 7),
        'unit_test_vector': HardwareInfo((CameraRecord(
            -2, 1.5, -0.25, (1.0, -2.0, 0.5), tuple(float(n) for n in range(15)), 640, 480, 1,
            (0.125,), (0.0, 0.0, 1.0), (-1.0, 0.0, 0.0), 2.0),), ((3.0, -4.0, 0.75),), 5),
        'four_cameras_forty_points': HardwareInfo(tuple(camera(10 + i, 64) for i in range(4)),
                                                  points(2, 40), -3),
        'empty_arrays': HardwareInfo((), (), 0),
        'series_beyond_limit': HardwareInfo((long_series,), points(3, 2), 1),
        'five_cameras_parser_stage': HardwareInfo(tuple(camera(20 + i) for i in range(5)),
                                                  points(4, 41), 2**31 - 1),
        'extreme_fixed_values': HardwareInfo((CameraRecord(
            -2**31, 32767.0, -32768.0, (2.0**21 - 2**-31, -2.0**21, 2**-42), (0.0,) * 15,
            0, -1, 2**31 - 1, (), (0.0,) * 3, (-0.0,) * 3, -2.0**21),), ((1.0e-6, -1.0e-6, 0.5),), 0)}


def quantized(info):
    """Native view after a wire round trip, i.e. what the parser stores."""
    return read_hardware_info(hardware_info_response(info, 0).payload)


def malformed_payloads():
    good = hardware_info_response(hardware_cases()['one_camera'], 0).payload
    items = tf.array_fields(1) + camera_fields(camera(1))
    wrong_type = items[:]
    wrong_type[3] = tf.fixed16_field(1.0)
    wrong_header = items[:]
    wrong_header[2] = tf.struct_field(0x19, 0x332C)
    return {
        'nonzero_prefix': b'\0\1' + good[2:],
        'wrong_wire_type_in_camera': tf.encode_payload(wrong_type),
        'wrong_camera_header': tf.encode_payload(wrong_header),
        'missing_second_camera': tf.encode_payload(tf.array_fields(2) + camera_fields(camera(1))),
        'truncated_after_cameras': good[:len(tf.encode_payload(items))],
        'scalar_length_eight': tf.encode_payload(tf.array_fields(0) + tf.array_fields(0)
                                                 + [Field(1, bytes(8))])}


def native_error(payload):
    try:
        read_hardware_info(payload)
    except ProtocolError as error:
        return str(error)
    return None


def same(a, b):
    return json.dumps(a, sort_keys=True, default=repr) == json.dumps(b, sort_keys=True, default=repr)


def run_writers(binary):
    rng = random.Random(5)
    cases = []
    machine = Machine(binary)

    def original(name, *args, xmm=None, data=None):
        cursor, buffer = machine.stream(4096)
        if xmm is not None:
            machine.vm.reg_write(UC_X86_REG_XMM1, struct.unpack('<Q', struct.pack('<d', xmm))[0]
                                 if name == 'fixed42' else struct.unpack('<I', struct.pack('<f', xmm))[0])
        machine.call(WRITERS[name], cursor, *args)
        length = struct.unpack_from('<I', machine.read(cursor, 0x10), 8)[0]
        return machine.read(buffer, length)

    for number in [0, 1, -1, 2**31 - 1, -2**31] + [rng.randrange(-2**31, 2**31) for _ in range(20)]:
        cases.append(('int32', number, original('int32', number & 0xFFFFFFFF), tf.int32_field(number).encode()))
        cases.append(('uint32', number & 0xFFFFFFFF, original('uint32', number & 0xFFFFFFFF),
                      tf.uint32_field(number & 0xFFFFFFFF).encode()))
        cases.append(('int64', number * 4099, original('int64', (number * 4099) & 2**64 - 1),
                      Field(6, struct.pack('>q', number * 4099)).encode()))
    for number in [0.0, 1.0, -1.0, 1.5e-5, -1.5e-5, 32767.5, -32768.0] + [rng.uniform(-3e4, 3e4) for _ in range(20)]:
        cases.append(('fixed16', number, original('fixed16', xmm=number), tf.fixed16_field(number).encode()))
    for number in [0.0, 1.0, -1.0, 2.0**-42, -2.0**-43, 2.0**20, -2.0**21] + [rng.uniform(-2e6, 2e6) for _ in range(20)]:
        cases.append(('fixed42', number, original('fixed42', xmm=number), tf.fixed42_field(number).encode()))
    for count, identifier in ((3, 0x1F41), (0x1A, 0x332C), (0, 0), (0xFFFF, 0xFFFF)):
        cases.append(('struct', [count, identifier], original('struct', count << 16 | identifier),
                      tf.struct_field(count, identifier).encode()))
    for count, descriptor in ((0, 0), (4, 0), (40, 0x12345678), (0xFFE, 1)):
        cases.append(('array_header', [count, descriptor], original('array_header', count, descriptor),
                      b''.join(f.encode() for f in tf.array_fields(count, descriptor))))
    for numbers in ((), (1.0,), tuple(rng.uniform(-100, 100) for _ in range(64))):
        source = machine.put(struct.pack(f'<{len(numbers)}d', *numbers))
        cases.append(('fixed42_array', list(numbers), original('fixed42_array', source, len(numbers)),
                      tf.fixed42_array_field(numbers).encode()))
    for text in (b'', b'a', b'camera-0'):
        source = machine.put(text + b'\0')
        expected = Field(0x14, struct.pack('>I', len(text)) + text).encode()
        cases.append(('string', text.decode(), original('string', source), expected))
        expected = Field(0x15, struct.pack('>I', len(text)) + text).encode()
        cases.append(('blob', text.decode(), original('blob', source, len(text)), expected))
    return [{'writer': name, 'value': repr(value), 'original': got.hex(), 'native': want.hex(),
             'match': got == want} for name, value, got, want in cases]


def run_readers(binary):
    rng = random.Random(6)
    machine = Machine(binary)
    cases = []

    def original(name, encoded, form, limit=None):
        cursor, buffer = machine.stream(len(encoded) + 16)
        machine.vm.mem_write(buffer, encoded)
        machine.vm.mem_write(cursor + 8, struct.pack('<I', len(encoded)))
        out, count = machine.allocate(0x400), machine.allocate(8)
        args = (cursor, out, count) if form == 'blob' else (cursor, out) if limit is None else (cursor, out, count, limit)
        machine.call(READERS[name], *args)
        status = machine.result() & 0xFFFFFFFF
        if status:
            return 'error'
        if form == 'string':
            pointer = struct.unpack('<Q', machine.read(out, 8))[0]
            return machine.read(pointer, 64).split(b'\0')[0].decode()
        if form == 'blob':
            size = struct.unpack('<I', machine.read(count, 4))[0]
            return machine.read(struct.unpack('<Q', machine.read(out, 8))[0], size).decode()
        if form == 'strings':
            size = struct.unpack('<I', machine.read(count, 4))[0]
            pointers = struct.unpack(f'<{size}Q', machine.read(out, 8 * size))
            return [machine.read(p, 64).split(b'\0')[0].decode() for p in pointers]
        if limit is not None:
            size = struct.unpack('<I', machine.read(count, 4))[0]
            return list(struct.unpack(f'<{size}{form}', machine.read(out, size * struct.calcsize(form))))
        return struct.unpack(form, machine.read(out, struct.calcsize(form)))[0]

    def native(field, wire, limit=None):
        if field.tag != wire:
            return 'error'
        try:
            got = tf.value(field)
        except ProtocolError:
            return 'error'
        if isinstance(got, bytes):
            return got.decode()
        if isinstance(got, tuple):
            got = [n.decode() for n in got] if got and isinstance(got[0], bytes) else list(got)
            return got[:limit]
        return got

    def compare(name, field, form, limit=None):
        got, want = original(name, field.encode(), form, limit), native(field, WIRE_OF[name], limit)
        cases.append({'reader': name, 'wire': field.encode().hex()[:80], 'original': got,
                      'native': want, 'match': same(got, want)})

    for _ in range(12):
        word = rng.randrange(2**32)
        compare('int32', Field(1, struct.pack('>I', word)), '<i')
        compare('uint32', Field(2, struct.pack('>I', word)), '<I')
        compare('fixed16', Field(3, struct.pack('>I', word)), '<f')
        long_word = rng.randrange(2**64)
        compare('fixed42', Field(4, struct.pack('>Q', long_word)), '<d')
        compare('int64', Field(6, struct.pack('>Q', long_word)), '<q')
        compare('uint64', Field(7, struct.pack('>Q', long_word)), '<Q')
    for word in (0x7FFFFFFF, 0x80000000, 0xFFFFFFFF, 0x01000001):
        compare('fixed16', Field(3, struct.pack('>I', word)), '<f')
    compare('int32', Field(1, bytes(8)), '<i')
    compare('int32', Field(2, bytes(4)), '<i')
    compare('fixed42', Field(4, bytes(4)), '<d')
    for text in (b'', b'x', b'serial-1234'):
        compare('string', Field(0x14, struct.pack('>I', len(text)) + text), 'string')
        compare('blob', Field(0x15, struct.pack('>I', len(text)) + text), 'blob')
    for count in (0, 3, 10, 12):
        words = [rng.randrange(2**32) for _ in range(count)]
        compare('fixed16_array', Field(0x18, struct.pack(f'>I{count}I', count, *words)), 'f', 10)
        longs = [rng.randrange(2**64) for _ in range(count)]
        compare('fixed42_array', Field(0x19, struct.pack(f'>I{count}Q', count, *longs)), 'd', 10)
    for names in ((), (b'a',), (b'one', b'two', b'three')):
        body = struct.pack('>I', len(names)) + b''.join(struct.pack('>I', len(n)) + n for n in names)
        compare('string_array', Field(0x1A, body), 'strings', 10)
    return cases


def run_hardware(binary):
    cases = []
    for name, info in hardware_cases().items():
        message = hardware_info_response(info, 0x1234, (0, 0xABCD))
        result, = parse_stream(binary, [message.encode()])
        tags, original = result['decoded'] or ((), None)
        native = quantized(info)
        exact = repr(original) == repr(native)
        cases.append({'case': name, 'payload_bytes': len(message.payload), 'status': result['status'],
                      'kind': result['kind'], 'request_id': result['request_id'], 'tags': tags,
                      'schema_check': result['hardware_schema_check'], 'cameras': len(original.cameras_0x8),
                      'points': len(original.points_0xbb0), 'byte_exact_values': exact,
                      'match': exact and result['status'] == 0 and tags == (0x1B, 0x0C, 2)
                      and result['hardware_schema_check'] == 0})
    return cases


def run_malformed(binary):
    cases = []
    for name, payload in malformed_payloads().items():
        message = Message(0x52, 9, 0, 0x848, 0, payload)
        result, = parse_stream(binary, [message.encode()])
        error = native_error(payload)
        cases.append({'case': name, 'original_status': result['status'],
                      'original_events': result['events'], 'native_error': error,
                      'match': result['status'] != 0 and error is not None})
    return cases


def run_status(binary):
    words = sorted(tf.DEVICE_ERROR_STATUS) + [0, 0x20000401, 0x2000050A, 0x2000050F, 0x20000517, 0xFFFFFFFF]
    cases = []
    payload = hardware_info_response(hardware_cases()['one_camera'], 0).payload
    for word in words:
        message = Message(0x52, 3, 0, 0x848, word, payload)
        result, = parse_stream(binary, [message.encode()])
        native = tf.response_status(message)
        cases.append({'word4': hex(word), 'original': result['status'], 'native': native,
                      'name': tf.status_name(native), 'fields': result['count'],
                      'match': result['status'] == native and (word != 0 or result['count'] == 3)})
    for kind in (0x51, 0x52, 0x53, 0x4C, 0x43, 0x4E, 0x60):
        message = Message(kind, 4, 0, 0x1388 if kind == 0x4E else 0x848, 0x20000503)
        result, = parse_stream(binary, [message.encode()])
        cases.append({'word0': hex(kind), 'parser_kind': result['kind'],
                      'native_kind': tf.PARSER_KINDS.get(kind, 0), 'status': result['status'],
                      'match': result['kind'] == tf.PARSER_KINDS.get(kind, 0)})
    return cases


def run_stream(binary):
    infos = list(hardware_cases().values())[:3]
    data = b''.join(hardware_info_response(info, i + 1).encode() for i, info in enumerate(infos))
    rng = random.Random(8)
    cuts = sorted(rng.sample(range(1, len(data)), 25))
    chunks = [data[a:b] for a, b in zip([0] + cuts, cuts + [len(data)])]
    results = parse_stream(binary, chunks)
    native = Decoder()
    messages = [m for chunk in chunks for m in native.feed(chunk)]
    native.finish()
    matches = [r['request_id'] == m.request_id and r['status'] == 0
               and r['decoded'] and repr(r['decoded'][1]) == repr(read_hardware_info(m.payload))
               for r, m in zip(results, messages)]
    return {'bytes': len(data), 'chunks': len(chunks), 'messages_original': len(results),
            'messages_native': len(messages), 'match': len(results) == len(messages) == 3 and all(matches)}


def main():
    binary = (ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    scanned = 0x900 if '--quick' in sys.argv else 0x2000
    table = schema_table(binary, scanned)
    expected = {c: t for c, t in tf.RESPONSE_FIELD_TYPES.items() if c < scanned}
    schema = {'commands': len(table), 'scanned': f'0x0..{scanned - 1:#x}',
              'match': {c: t for c, (t, _) in table.items()} == expected,
              'table': {hex(c): {'types': [hex(t) for t in types], 'parameters': [hex(p) for p in params]}
                        for c, (types, params) in table.items()}}
    report = {'kind': 'supplemental-emulated-original-functions',
              'inputs': 'synthetic vectors derived from recovered instructions, not device traffic',
              'functions': {'schema': hex(SCHEMA), 'parser_init': hex(PARSER_INIT),
                            'feed': hex(PARSER_FEED), 'next_message': hex(PARSER_NEXT),
                            'hardware_schema_check': hex(SCHEMA_CHECK)},
              'schema': schema, 'writers': run_writers(binary), 'readers': run_readers(binary),
              'hardware_info': run_hardware(binary), 'malformed': run_malformed(binary),
              'status': run_status(binary), 'stream': run_stream(binary)}
    groups = {'schema': [schema], 'writers': report['writers'], 'readers': report['readers'],
              'hardware_info': report['hardware_info'], 'malformed': report['malformed'],
              'status': report['status'], 'stream': [report['stream']]}
    report['summary'] = {name: f"{sum(c['match'] for c in cases)}/{len(cases)}" for name, cases in groups.items()}
    report['passed'] = all(c['match'] for cases in groups.values() for c in cases)
    (ROOT/'evidence/private/ttp-reply-oracle.json').write_text(json.dumps(report, indent=2, default=repr))
    for name, text in report['summary'].items():
        print(name, text)
    print('PASS' if report['passed'] else 'FAIL')
    raise SystemExit(0 if report['passed'] else 1)


if __name__ == '__main__':
    main()
