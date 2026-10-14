#!/usr/bin/env python3
"""Per-target accuracy of the published gaze point, Linux captures against Windows.

Linux: recorded 0x500 messages go through tobii_linux.gaze (the runtime's host
path) and the published gaze point (PRP stream 3) is scored. The capture's
device timestamps are mapped onto the target log's QueryPerformanceCounter clock
with one constant offset found by grid search, because the WSL wall clock drifts.

Windows: Stream Engine gaze points from tools/windows_stream_probe.py are already
on the QueryPerformanceCounter clock and are scored directly.

The error per target is the median pixel distance of valid points from --skip
seconds after the target appears until it disappears.
"""
import argparse
import json
from pathlib import Path
import statistics
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import gaze  # noqa: E402


def targets(path):
    screen, starts, ends = None, {}, {}
    for line in open(path):
        row = json.loads(line)
        if row['kind'] == 'screen':
            screen = (row['width'], row['height'])
        elif row['kind'] == 'target_start':
            starts[row['index']] = row
        elif row['kind'] == 'target_end':
            ends[row['index']] = row
    return screen, [(starts[i]['x'], starts[i]['y'], starts[i]['qpc_us'], ends[i]['qpc_us'])
                    for i in sorted(starts) if i in ends]


def linux_points(path):
    return [(frame.timestamp_us, bool(frame.gaze_point.valid), frame.gaze_point.point)
            for _, frame in gaze.convert_messages(gaze.recorded_gaze_messages(path))]


def windows_points(path):
    points = []
    for line in open(path):
        row = json.loads(line)
        if row.get('kind') == 'gaze':
            points.append((row['timestamp_us'], bool(row['validity']), tuple(row['position'])))
    return points


def per_target(points, screen, target_list, offset_us, skip_us):
    width, height = screen
    rows = []
    for x, y, start, end in target_list:
        inside = [p for p in points if start + skip_us <= p[0] + offset_us < end]
        errors = [((px - x) * width) ** 2 + ((py - y) * height) ** 2
                  for _, valid, (px, py) in inside if valid]
        rows.append({'target': [x, y], 'samples': len(inside), 'valid': len(errors),
                     'median_error_px': round(statistics.median(errors) ** 0.5, 1) if errors else None})
    return rows


def summary(rows):
    errors = [row['median_error_px'] for row in rows if row['median_error_px'] is not None]
    return {'median_error_px': statistics.median(errors) if errors else None,
            'error_range_px': [min(errors), max(errors)] if errors else None}


def fit_offset(points, screen, target_list, skip_us, step_us=10_000, span_us=20_000_000):
    first_start = target_list[0][2]
    guess = first_start - points[0][0]
    best = None
    for offset in range(guess - span_us, guess + span_us + 1, step_us):
        rows = per_target(points, screen, target_list, offset, skip_us)
        errors = [row['median_error_px'] for row in rows if row['median_error_px'] is not None]
        if len(errors) == len(target_list):
            score = statistics.median(errors)
            if best is None or score < best[0]:
                best = (score, offset)
    if best is None:
        raise RuntimeError('No offset puts valid gaze inside every target window')
    return best[1]


def linux_report(capture, target_path, skip_us, offset_us=None):
    screen, target_list = targets(target_path)
    points = linux_points(capture)
    offset = fit_offset(points, screen, target_list, skip_us) if offset_us is None else offset_us
    rows = per_target(points, screen, target_list, offset, skip_us)
    return {'source': str(capture), 'targets': str(target_path), 'published_points': len(points),
            'valid': sum(valid for _, valid, _ in points), 'offset_us': offset,
            **summary(rows), 'per_target': rows}


def windows_report(capture, target_path, skip_us):
    screen, target_list = targets(target_path)
    points = windows_points(capture)
    rows = per_target(points, screen, target_list, 0, skip_us)
    return {'source': str(capture), 'targets': str(target_path), 'points': len(points),
            'valid': sum(valid for _, valid, _ in points), **summary(rows), 'per_target': rows}


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--linux', nargs=2, action='append', default=[],
                        metavar=('CAPTURE', 'TARGETS'))
    parser.add_argument('--windows', nargs=2, action='append', default=[],
                        metavar=('GAZE', 'TARGETS'))
    parser.add_argument('--skip', type=float, default=0.8,
                        help='seconds ignored after each target appears')
    parser.add_argument('--offset-us', type=int,
                        help='fixed QPC minus device time instead of the grid search, for example '
                             'the offset Windows timesync measured')
    args = parser.parse_args()
    skip_us = int(args.skip * 1e6)
    for capture, target_path in args.windows:
        print(json.dumps({'side': 'windows', **windows_report(capture, target_path, skip_us)}))
    for capture, target_path in args.linux:
        print(json.dumps({'side': 'linux', **linux_report(capture, target_path, skip_us,
                                                            args.offset_us)}))


if __name__ == '__main__':
    main()
