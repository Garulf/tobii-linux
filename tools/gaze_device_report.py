#!/usr/bin/env python3
"""Gaze validity and accuracy of a live 0x500 capture against a target log.

Reads a capture written by tools/gaze_device_capture.py (or
tools/linux_stream_capture.py) and the JSON-lines log of
tools/windows_gaze_targets.py. The WSL wall clock drifts against Windows, so
the device timestamp (column 1, microseconds) is mapped onto the target log's
QueryPerformanceCounter clock with one constant offset, chosen by grid search
to minimise the median per-target error. The error uses column 28 (relative
combined gaze point, normalised display coordinates) where column 27 says it
is valid, from --skip seconds after each target appears until it disappears.
"""
import argparse
import json
from pathlib import Path
import statistics
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux.frames import stream_columns  # noqa: E402
from tobii_linux.ttp_fields import value  # noqa: E402

GAZE_WIRE_ID = 0x500
TIMESTAMP, LEFT_VALIDITY, RIGHT_VALIDITY = 1, 7, 13
COMBINED_VALIDITY, COMBINED_POINT = 27, 28
EYE_VALID, COMBINED_VALID = 0, 1


def gaze_records(path):
    records = []
    for line in open(path):
        entry = json.loads(line)
        if entry.get('kind') != 'message' or entry.get('command') != GAZE_WIRE_ID or not entry.get('payload'):
            continue
        columns = {c.column_id: [value(f) for f in c.value] for c in stream_columns(bytes.fromhex(entry['payload']))}
        records.append({'device_us': columns[TIMESTAMP][0], 'wall': entry['time'],
                        'left': columns[LEFT_VALIDITY][0] == EYE_VALID,
                        'right': columns[RIGHT_VALIDITY][0] == EYE_VALID,
                        'combined': columns[COMBINED_VALIDITY][0] == COMBINED_VALID,
                        'point': tuple(columns[COMBINED_POINT][1:])})
    return records


def target_windows(path):
    screen, starts, ends = None, {}, {}
    for line in open(path):
        entry = json.loads(line)
        if entry['kind'] == 'screen':
            screen = (entry['width'], entry['height'])
        elif entry['kind'] == 'target_start':
            starts[entry['index']] = entry
        elif entry['kind'] == 'target_end':
            ends[entry['index']] = entry
    targets = [{'index': i, 'x': starts[i]['x'], 'y': starts[i]['y'], 'start_us': starts[i]['qpc_us'],
                'end_us': ends[i]['qpc_us'], 'start_wall': starts[i]['wall']}
               for i in sorted(starts) if i in ends]
    return screen, targets


def per_target(records, targets, screen, offset_us, skip_us):
    width, height = screen
    rows = []
    for target in targets:
        inside = [r for r in records if target['start_us'] + skip_us <= r['device_us'] + offset_us < target['end_us']]
        valid = [r for r in inside if r['combined']]
        errors = [((r['point'][0] - target['x'])*width)**2 + ((r['point'][1] - target['y'])*height)**2
                  for r in valid]
        rows.append({'index': target['index'], 'x': target['x'], 'y': target['y'], 'records': len(inside),
                     'combined_valid': len(valid), 'both_eyes_valid': sum(r['left'] and r['right'] for r in inside),
                     'median_error_px': round(statistics.median(errors)**0.5, 1) if errors else None})
    return rows


def fit_offset(records, targets, screen, skip_us, step_us=10_000, span_us=15_000_000):
    first = targets[0]
    guess = first['start_us'] - min(r['device_us'] for r in records if r['wall'] >= first['start_wall'] - 1)
    best = None
    for offset in range(guess - span_us, guess + span_us + 1, step_us):
        rows = per_target(records, targets, screen, offset, skip_us)
        errors = [row['median_error_px'] for row in rows if row['median_error_px'] is not None]
        if len(errors) < len(targets):
            continue
        score = statistics.median(errors)
        if best is None or score < best[0]:
            best = (score, offset)
    return best[1]


def report(capture, targets_path, skip_s):
    records = gaze_records(capture)
    screen, targets = target_windows(targets_path)
    skip_us = int(skip_s * 1e6)
    offset = fit_offset(records, targets, screen, skip_us)
    rows = per_target(records, targets, screen, offset, skip_us)
    errors = [row['median_error_px'] for row in rows if row['median_error_px'] is not None]
    return {'capture': str(capture), 'targets': str(targets_path), 'records': len(records),
            'combined_valid': sum(r['combined'] for r in records),
            'both_eyes_valid': sum(r['left'] and r['right'] for r in records),
            'offset_us': offset, 'screen': screen, 'per_target': rows,
            'median_error_px': statistics.median(errors) if errors else None,
            'error_range_px': [min(errors), max(errors)] if errors else None}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('pairs', nargs='+', help='capture.jsonl targets.jsonl [capture targets ...]')
    parser.add_argument('--skip', type=float, default=0.8, help='seconds ignored after each target appears')
    args = parser.parse_args()
    if len(args.pairs) % 2:
        parser.error('captures and target logs must come in pairs')
    for capture, targets in zip(args.pairs[::2], args.pairs[1::2]):
        print(json.dumps(report(Path(capture), Path(targets), args.skip)))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
