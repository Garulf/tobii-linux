#!/usr/bin/env python3
"""Compare the calibration parsers and the apply request with the original code in emulation.

Three groups, all offline Unicorn runs of hash-bound original functions:

- tobii_calibration_parse (Stream Engine 0x18007e9b0) against
  gaze_device.calibration_points, on synthetic blobs and, when the private
  copy exists, the calibration.setpm blob read from albedo.
- The runtime's response parser plus the 1120 type check 0x1402c1900, whose
  parsed records 0x1402b7830 copies, against gaze_device.stimulus_points on
  replies built by gaze_device.stimulus_points_payload (synthetic records and
  the values the device returned live).
- The 1110 builder 0x1402c5050 with the full albedo blob against
  gaze_device.calibration_apply.

Nothing is sent to a device. See docs/gaze-calibration.md.
"""
import hashlib
import json
from pathlib import Path
import struct

from unicorn import UC_ARCH_X86, UC_HOOK_CODE, UC_MODE_64, Uc
from unicorn.x86_const import (UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RAX, UC_X86_REG_RCX,
                              UC_X86_REG_RDX, UC_X86_REG_RIP, UC_X86_REG_RSP)
from ttp_reply_oracle import Machine, PARSER_BLOCK, PARSER_FEED, PARSER_INIT, PARSER_NEXT
from vsmat_oracle import MatrixMachine
from tobii_linux import gaze_device as gd
from tobii_linux.ttp import Message, ProtocolError, fields
from tobii_linux.ttp_fields import encode_payload, value

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe'
STREAM_ENGINE = ROOT/'evidence/private/binaries/tobii_stream_engine.dll'
STREAM_ENGINE_SHA256 = 'f60c90c41a6c29718f0255ab22db160b038f9305fe92d184c9cf3e8e938ec5a3'
ALBEDO_SETTING = ROOT/'evidence/private/gaze-device/calibration.setpm'
ALBEDO_QUERY = ROOT/'evidence/private/gaze-device/device-calibration-query.jsonl'
REPORT = ROOT/'evidence/private/gaze-calibration-oracle.json'

CALIBRATION_PARSE = 0x18007E9B0
STREAM_ENGINE_LOG = 0x180092170
STIMULUS_TYPE_CHECK = 0x1402C1900
STIMULUS_PARSED_TYPE = 0x1C
APPLY_BUILDER = 0x1402C5050
STACK, STOP, CALLBACK, HEAP = 0x60000000, 0x62000000, 0x63000000, 0x70000000
HEAP_SIZE = 0x2000000
POINT_DATA = struct.Struct('<2fi2fi2f')


class StreamEngine:
    """The Stream Engine DLL mapped at its preferred base with a callback trap and a bump heap."""

    def __init__(self, binary):
        if hashlib.sha256(binary).hexdigest() != STREAM_ENGINE_SHA256:
            raise ValueError('This oracle only supports the researched tobii_stream_engine.dll')
        pe = struct.unpack_from('<I', binary, 0x3C)[0]
        sections = struct.unpack_from('<H', binary, pe + 6)[0]
        optional = pe + 24
        optional_size = struct.unpack_from('<H', binary, pe + 20)[0]
        base = struct.unpack_from('<Q', binary, optional + 24)[0]
        size = struct.unpack_from('<I', binary, optional + 56)[0]
        self.vm = Uc(UC_ARCH_X86, UC_MODE_64)
        self.vm.mem_map(base, (size + 0xFFF) & ~0xFFF)
        for index in range(sections):
            entry = optional + optional_size + index * 40
            _, rva, raw_size, raw_offset = struct.unpack_from('<IIII', binary, entry + 8)
            if raw_size:
                self.vm.mem_write(base + rva, binary[raw_offset:raw_offset + raw_size])
        for address, length in ((STACK, 0x20000), (STOP, 0x1000), (CALLBACK, 0x1000), (HEAP, HEAP_SIZE)):
            self.vm.mem_map(address, length)
        self.vm.mem_write(CALLBACK, b'\xc3')
        self.vm.hook_add(UC_HOOK_CODE, self._callback, begin=CALLBACK, end=CALLBACK)
        self.vm.hook_add(UC_HOOK_CODE, self._log, begin=STREAM_ENGINE_LOG, end=STREAM_ENGINE_LOG)
        self.cursor = HEAP
        self.records = []
        self.logged = 0

    def put(self, data):
        address = self.cursor
        self.cursor += (len(data) + 0x3F) & ~0x3F
        if self.cursor > HEAP + HEAP_SIZE:
            raise RuntimeError('Stream Engine heap exhausted')
        self.vm.mem_write(address, data)
        return address

    def _callback(self, vm, address, size, _):
        pointer = vm.reg_read(UC_X86_REG_RCX)
        self.records.append(POINT_DATA.unpack(bytes(vm.mem_read(pointer, POINT_DATA.size))))

    def _log(self, vm, address, size, _):
        self.logged += 1
        stack = vm.reg_read(UC_X86_REG_RSP)
        vm.reg_write(UC_X86_REG_RIP, struct.unpack('<Q', vm.mem_read(stack, 8))[0])
        vm.reg_write(UC_X86_REG_RSP, stack + 8)

    def calibration_parse(self, blob):
        self.records, self.logged = [], 0
        data = self.put(blob or b'\0')
        stack = STACK + 0x1F000
        self.vm.mem_write(stack, struct.pack('<Q', STOP))
        self.vm.mem_write(stack + 0x28, struct.pack('<Q', 0x1234))
        self.vm.reg_write(UC_X86_REG_RSP, stack)
        api = self.put(b'\0' * 0x100)
        for register, number in zip((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9),
                                    (api, data, len(blob), CALLBACK)):
            self.vm.reg_write(register, number)
        self.vm.emu_start(CALIBRATION_PARSE, STOP, timeout=20_000_000, count=50_000_000)
        return self.vm.reg_read(UC_X86_REG_RAX) & 0xFFFFFFFF, list(self.records)


def blob_with(records, header=b'', count=None, trailer_word=0x5A5A5A5A):
    """A blob whose word 0 points at a trailer of the given 40-byte records."""
    body = struct.pack('<I', 0) + header
    trailer = struct.pack('<Ii', trailer_word, len(records) if count is None else count)
    table = b''.join(gd.CALIBRATION_RECORD.pack(*r) for r in records)
    return struct.pack('<I', len(body)) + body[4:] + trailer + table


def synthetic_blobs():
    records = [(0.5, 0.5, 0.49, 0.51, 1, 0.48, 0.5, 1),
               (0.1, 0.9, 0.15, 0.87, 0, 0.09, 0.95, -1),
               (0.9, 0.1, -0.01, 1.2, -1, 0.97, -0.08, 0),
               (0.25, 0.75, 0.3, 0.7, 2, 0.2, 0.8, 7),
               (1e-3, -2.5, 3.0e5, -0.0, 0x7FFFFFFF, 0.0, 0.0, -2)]
    return {'one_record': blob_with(records[:1], b'\x11' * 60),
            'statuses': blob_with(records, b'human\0' + b'\0' * 58),
            'empty_table': blob_with([], b'\x22' * 16),
            'negative_count': blob_with(records[:2], count=-1),
            'header_only': struct.pack('<II', 0, 0),
            'too_short': b'\x08\0\0',
            'odd_trailer_word': blob_with(records[1:3], trailer_word=0xFD559C5A)}


def native_points(blob):
    try:
        return [(p.point[0], p.point[1], int(p.left_status), p.left_mapping[0], p.left_mapping[1],
                 int(p.right_status), p.right_mapping[0], p.right_mapping[1])
                for p in gd.calibration_points(blob)]
    except ProtocolError:
        return None


def run_parse(engine):
    blobs = synthetic_blobs()
    if ALBEDO_SETTING.exists():
        blobs['albedo_calibration'] = gd.parse_calibration_setting(ALBEDO_SETTING.read_bytes()).blob
    cases = []
    for name, blob in blobs.items():
        status, original = engine.calibration_parse(blob)
        native = native_points(blob)
        expected = None if status else [tuple(r) for r in original]
        cases.append({'case': name, 'bytes': len(blob), 'original_status': status,
                      'original_records': len(original), 'native_records': None if native is None else len(native),
                      'match': (native is None and status != 0) or native == expected})
    return cases


def parse_reply(binary, message):
    machine = Machine(binary)
    block, holder = machine.allocate(PARSER_BLOCK), machine.allocate(8)
    machine.call(PARSER_INIT, holder, block, PARSER_BLOCK, 0, 0, 0, 0, 0)
    encoded = message.encode()
    machine.call(PARSER_FEED, block, machine.put(encoded), len(encoded))
    parsed = machine.allocate(0x20)
    machine.call(PARSER_NEXT, block, parsed)
    status = machine.result() & 0xFFFFFFFF
    if status:
        return status, None
    machine.call(STIMULUS_TYPE_CHECK, parsed, STIMULUS_PARSED_TYPE)
    check = machine.result() & 0xFFFFFFFF
    if check:
        return 0x100 | check, None
    item = struct.unpack_from('<Q', machine.read(parsed, 0x20), 0x18)[0]
    count, records = struct.unpack('<i4xQ', machine.read(item + 8, 16))
    return 0, [struct.unpack('<3fi2fi2f', machine.read(records + 36 * i, 36)) for i in range(count)]


def stimulus_cases():
    point = gd.StimulusPoint
    quality = gd.EyeQuality
    cases = {'single': [point((0.5, 0.5, 0.0), quality(2, 0.0127, 0.0491), quality(2, 0.0135, 0.0548))],
             'statuses': [point((0.1, 0.9, 0.0), quality(0, 0.0, 0.0), quality(1, 0.25, 0.5)),
                          point((0.9, 0.1, 0.0), quality(2, 1.5, -0.125), quality(0, 3.0, 2.0)),
                          point((0.5, 0.1, 0.25), quality(1, 0.0001, 0.9999), quality(2, 0.75, 0.0))],
             'empty': []}
    if ALBEDO_QUERY.exists():
        for line in ALBEDO_QUERY.read_text().splitlines():
            record = json.loads(line)
            if record.get('command') == 'GET_CALIBRATION_STIMULUS_POINTS':
                values = [v for v in record['values'] if not isinstance(v, str)]
                cases['albedo_live_values'] = [
                    point(tuple(values[1 + 9 * i:4 + 9 * i]), quality(*values[4 + 9 * i:7 + 9 * i]),
                          quality(*values[7 + 9 * i:10 + 9 * i])) for i in range((len(values) - 1) // 9)]
    return cases


def run_stimulus(binary):
    cases = []
    for name, points in stimulus_cases().items():
        message = Message(0x52, 7, 0, int(gd.GazeCommand.GET_CALIBRATION_STIMULUS_POINTS), 0,
                          encode_payload(gd.stimulus_points_payload(points)))
        status, original = parse_reply(binary, message)
        native = [(*p.point, p.left.status, p.left.bias, p.left.precision,
                   p.right.status, p.right.bias, p.right.precision)
                  for p in gd.stimulus_points([value(f) for f in fields(message.payload)])]
        cases.append({'case': name, 'records': len(points), 'original_status': status,
                      'match': status == 0 and original == native})
    return cases


def run_apply(binary):
    if not ALBEDO_SETTING.exists():
        return []
    blob = gd.parse_calibration_setting(ALBEDO_SETTING.read_bytes()).blob
    machine = MatrixMachine(binary)
    capacity = len(blob) + 0x1000
    buffer, data = machine.allocate(capacity), machine.allocate(len(blob))
    machine.vm.mem_write(data, blob)
    machine.call(APPLY_BUILDER, 12, data, len(blob), buffer, capacity, instruction_limit=50_000_000)
    length = machine.vm.reg_read(UC_X86_REG_RAX)
    original = bytes(machine.vm.mem_read(buffer, length)) if 0 < length <= capacity else b''
    native = gd.calibration_apply(12, blob).encode()
    return [{'case': 'albedo_calibration_apply', 'blob_bytes': len(blob), 'request_bytes': len(native),
             'original_sha256': hashlib.sha256(original).hexdigest(),
             'native_sha256': hashlib.sha256(native).hexdigest(), 'match': original == native}]


def main():
    runtime = RUNTIME.read_bytes()
    report = {'kind': 'offline Unicorn emulation of original functions',
              'functions': {'tobii_calibration_parse': hex(CALIBRATION_PARSE),
                            'stimulus_type_check': hex(STIMULUS_TYPE_CHECK), 'apply_builder': hex(APPLY_BUILDER)},
              'calibration_parse': run_parse(StreamEngine(STREAM_ENGINE.read_bytes())),
              'stimulus_points': run_stimulus(runtime), 'calibration_apply': run_apply(runtime)}
    groups = ('calibration_parse', 'stimulus_points', 'calibration_apply')
    report['summary'] = {g: f"{sum(c['match'] for c in report[g])}/{len(report[g])}" for g in groups}
    report['passed'] = all(c['match'] for g in groups for c in report[g])
    REPORT.write_text(json.dumps(report, indent=2))
    for group in groups:
        for case in report[group]:
            print(group, case['case'], 'ok' if case['match'] else f'MISMATCH {case}')
    print(report['summary'])
    print('PASS' if report['passed'] else 'FAIL')
    raise SystemExit(0 if report['passed'] else 1)


if __name__ == '__main__':
    main()
