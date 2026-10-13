#!/usr/bin/env python3
"""Compare the landmark smoothing 0x14007f5a0 (0x14004c480, 0x14004bf60, 0x14004bed0) with landmark_smoothing."""
import json
from pathlib import Path
import random
import struct
import numpy as np
from vsmat_oracle import MatrixMachine
from tobii_linux.landmark_smoothing import (LandmarkFilter, LandmarkRecord, flip_y, smooth_records,
                                            table_entries, EAR_TABLE)
ROOT = Path(__file__).resolve().parents[1]
INITIALISE = 0x1400011b0
CREATE_FILTER = 0x14004bcc0
RESET_FILTER = 0x14004bed0
CREATE_RECORDS = 0x140010140
SMOOTH_RECORDS = 0x14007f5a0
SMOOTH_COORDINATES = 0x14004c480
FLIP_Y = 0x14007ac80
TLS_ARRAY = 0x64000100
TLS_BLOCK = 0x64000200
FACTORS = (1.1, 0.9, 1.1, 1.1, 1.1, 1.2, 0.8, -1.0)
D1_RECORDS = ([(2, i) for i in (1, 2, 3)] + [(3, i) for i in range(1, 15)] + [(4, i) for i in range(1, 7)]
              + [(8, i) for i in range(1, 11)] + [(9, i) for i in (3, 4, 5, 12, 15)] + [(13, 17)]
              + [(14, i) for i in range(21, 26)] + [(15, i) for i in range(1, 18)])


class Original:
    """Original filter and record set in persistent emulator memory."""

    def __init__(self, binary):
        self.m = MatrixMachine(binary)
        self.m.call(INITIALISE)
        self.m.vm.mem_write(0x64000058, struct.pack('<Q', TLS_ARRAY))
        self.m.vm.mem_write(TLS_ARRAY, struct.pack('<Q', TLS_BLOCK))
        self.filter = self.m.allocate(0x48)
        self.m.call(CREATE_FILTER, self.filter, 2000)
        self.container = self.m.allocate(0x28)
        self.m.vm.mem_write(self.container, struct.pack('<5Q', 0, self.filter, 0, 0, 0))
        self.records = self.m.allocate(0x1a8)
        self.m.call(CREATE_RECORDS, self.records)

    def _record(self, group, index):
        base = struct.unpack('<Q', self.m.vm.mem_read(self.records + 0x120 + 8*group, 8))[0]
        return base + (index - 1)*0x48

    def write(self, records):
        for (group, index), record in records.items():
            address = self._record(group, index)
            self.m.vm.mem_write(address, struct.pack('<ff', record.x, record.y))
            self.m.vm.mem_write(address + 0xc, struct.pack('<i', int(record.present)))
            self.m.vm.mem_write(address + 0x14, struct.pack('<f', record.quality))

    def read(self, group, index):
        address = self._record(group, index)
        x, y = struct.unpack('<ff', self.m.vm.mem_read(address, 8))
        quality = struct.unpack('<f', self.m.vm.mem_read(address + 0x14, 4))[0]
        return x, y, quality

    def begin_frame(self):
        frames = struct.unpack('<i', self.m.vm.mem_read(self.filter + 0x10, 4))[0]
        self.m.vm.mem_write(self.filter + 0xc, struct.pack('<ii', 0, frames + 1))

    def reset(self):
        self.m.call(RESET_FILTER, self.filter)

    def smooth(self, time, entries):
        table = self.m.allocate(16*len(entries))
        self.m.vm.mem_write(table, b''.join(struct.pack('<4f', *entry) for entry in entries))
        self.m.call(SMOOTH_RECORDS, self.container, self.records, time, table, len(entries),
                    instruction_limit=3_000_000_000, timeout_us=900_000_000)

    def smooth_coordinates(self, values, gains, time):
        count = len(values)
        data, pointers, weights = self.m.allocate(4*count), self.m.allocate(8*count), self.m.allocate(4*count)
        self.m.vm.mem_write(data, struct.pack(f'<{count}f', *values))
        self.m.vm.mem_write(pointers, struct.pack(f'<{count}Q', *(data + 4*i for i in range(count))))
        self.m.vm.mem_write(weights, struct.pack(f'<{count}f', *gains))
        self.m.call(SMOOTH_COORDINATES, self.filter, pointers, count, time, 0x136, weights,
                    instruction_limit=3_000_000_000, timeout_us=900_000_000)
        return struct.unpack(f'<{count}f', self.m.vm.mem_read(data, 4*count))


def ulps(a, b):
    a, b = np.float32(a), np.float32(b)
    if a == b:
        return 0
    if not (np.isfinite(a) and np.isfinite(b)):
        return 1 << 31
    ia = int(np.array(a).view(np.int32))
    ib = int(np.array(b).view(np.int32))
    ia = ia if ia >= 0 else -(ia & 0x7fffffff)
    ib = ib if ib >= 0 else -(ib & 0x7fffffff)
    return abs(ia - ib)


def frame_records(rng, frame, absent=()):
    records = {}
    for j, identifier in enumerate(D1_RECORDS):
        drift = 0.0015*frame + 0.02*np.sin(frame/6.0)
        x = 0.3 + 0.006*j + drift + rng.gauss(0, 0.002)
        y = 0.65 - 0.004*j + 0.5*drift + rng.gauss(0, 0.002)
        quality = min(1.0, max(-0.2, rng.gauss(0.75, 0.15)))
        records[identifier] = LandmarkRecord(float(np.float32(x)), float(np.float32(y)), identifier not in absent,
                                             float(np.float32(quality)))
    return records


def compare(original, native, entries, stats):
    identifiers = sorted({(g, i) for g, a, b, _ in entries for i in range(int(a), int(b) + 1)}
                         | {key for key, record in native.items() if record.present})
    worst = 0
    for identifier in identifiers:
        x, y, quality = original.read(*identifier)
        record = native.get(identifier, LandmarkRecord())
        for field, expected, actual in (('x', x, record.x), ('y', y, record.y), ('quality', quality, record.quality)):
            distance = ulps(expected, actual)
            stats['values'] += 1
            stats['exact'] += distance == 0
            stats['max_ulps'][field] = max(stats['max_ulps'][field], distance)
            stats['max_abs'][field] = max(stats['max_abs'][field], abs(float(np.float32(expected)) - float(np.float32(actual))))
            worst = max(worst, distance)
    return worst


def record_sequence(binary, name, frames, entries, events):
    original, native_filter = Original(binary), LandmarkFilter()
    rng = random.Random(name)
    stats = {'values': 0, 'exact': 0, 'max_ulps': {'x': 0, 'y': 0, 'quality': 0},
             'max_abs': {'x': 0.0, 'y': 0.0, 'quality': 0.0}}
    time = 0
    for frame in range(frames):
        event = events.get(frame, {})
        time += event.get('gap', 0) + 30 + (frame % 3 == 0)
        if event.get('reset'):
            original.reset()
            native_filter.reset()
        records = frame_records(rng, frame, event.get('absent', ()))
        original.write(records)
        original.begin_frame()
        native_filter.begin_frame()
        original.smooth(time, entries)
        smooth_records(native_filter, records, time, entries)
        worst = compare(original, records, entries, stats)
        print(name, 'frame', frame, 'time', time, 'max ulps', worst, flush=True)
    stats['byte_exact_fraction'] = stats['exact'] / stats['values']
    return stats


def coordinate_sequence(binary, name, frames, gains, signal):
    original, native_filter = Original(binary), LandmarkFilter()
    stats = {'values': 0, 'exact': 0, 'max_ulps': 0, 'max_abs': 0.0}
    for frame in range(frames):
        time = 33*frame
        values = [float(np.float32(signal(frame, c))) for c in range(len(gains))]
        original.begin_frame()
        native_filter.begin_frame()
        expected = original.smooth_coordinates(values, gains, time)
        actual = native_filter.smooth_coordinates(values, gains, time)
        for e, a in zip(expected, actual):
            stats['values'] += 1
            stats['exact'] += ulps(e, a) == 0
            stats['max_ulps'] = max(stats['max_ulps'], ulps(e, a))
            stats['max_abs'] = max(stats['max_abs'], abs(float(e) - float(a)))
        print(name, 'frame', frame, 'max ulps', max(ulps(e, a) for e, a in zip(expected, actual)), flush=True)
    stats['byte_exact_fraction'] = stats['exact'] / stats['values']
    return stats


def flip_case(binary):
    original = Original(binary)
    rng = random.Random('flip')
    records = frame_records(rng, 0, absent=((3, 5), (8, 2)))
    original.write(records)
    original.m.call(FLIP_Y, original.records)
    flip_y(records)
    mismatches = 0
    for identifier, record in records.items():
        x, y, _ = original.read(*identifier)
        mismatches += ulps(x, record.x) != 0 or ulps(y, record.y) != 0
    print('flip mismatches', mismatches, flush=True)
    return {'records': len(records), 'absent': 2, 'mismatches': mismatches, 'max_abs': 0.0 if not mismatches else 1.0}


def main():
    binary = (ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    tracker = table_entries(FACTORS)
    with_ears = tracker + table_entries(FACTORS, EAR_TABLE)
    report = {'functions': [hex(SMOOTH_RECORDS), hex(SMOOTH_COORDINATES), '0x14004bf60', hex(RESET_FILTER)],
              'cases': {}}
    report['cases']['flip-y'] = flip_case(binary)
    report['cases']['tracking'] = record_sequence(binary, 'tracking', 30, tracker, {})
    report['cases']['reset-gap-absent'] = record_sequence(binary, 'reset-gap-absent', 30, with_ears, {
        8: {'absent': ((3, 5), (3, 6), (9, 15))}, 9: {'absent': ((3, 5), (3, 6))},
        14: {'reset': True}, 22: {'gap': 400}, 23: {'absent': ((8, 1),)}})
    report['cases']['coordinates-degenerate'] = coordinate_sequence(
        binary, 'coordinates-degenerate', 14, [0.0, 2.5, -0.5, 1.0, 0.3],
        lambda f, c: (0.5, 0.25 + 0.01*f, 0.9, 0.1*(f % 2), 0.4 + 0.001*f*f)[c])
    tolerance = 3e-07
    passed = True
    for case in report['cases'].values():
        if 'mismatches' in case:
            passed &= case['mismatches'] == 0
        elif isinstance(case['max_abs'], dict):
            passed &= case['max_abs']['x'] <= tolerance and case['max_abs']['y'] <= tolerance
            passed &= case['max_ulps']['quality'] == 0
        else:
            passed &= case['max_abs'] <= tolerance
    report['coordinate_tolerance'] = tolerance
    report['quality_tolerance_ulps'] = 0
    report['passed'] = bool(passed)
    report['limitations'] = ('Synthetic records; allocator/free stubs, synthetic GS stack limit and a zeroed TLS '
                             'block only. Pseudo-inverse and least-squares use float64 natively against '
                             'float32 OpenCV SVD, hence the ulp tolerance.')
    (ROOT/'evidence/private/landmark-smoothing-oracle.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['cases']))
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
