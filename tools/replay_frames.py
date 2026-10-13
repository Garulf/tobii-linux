#!/usr/bin/env python3
"""Replay frames saved by linux_session_probe.py through the head-pose pipeline.

Reads the probe's JSON-lines log for the camera calibration and frame
timestamps, loads each saved grayscale frame, and prints one JSON line per
frame followed by a summary of the valid samples. No device is needed.
"""
import argparse
import json
from pathlib import Path
import statistics
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux.cli import DEFAULT_MODELS  # noqa: E402

ALBEDO_DISPLAY = (-296.62, 326.0, 111.82, 300.38, 326.0, 111.82, -296.62, 10.27, -3.1)


def recorded_session(log_path, frames_dir):
    calibration, frames = None, []
    for line in Path(log_path).read_text().splitlines():
        row = json.loads(line)
        if row.get('kind') == 'step' and row.get('name') == 'calibration':
            calibration = (tuple(row['translation']), tuple(row['angles']))
        elif row.get('kind') == 'frame':
            name = Path(row['path'].replace('\\', '/')).name
            frames.append((row['timestamp'], row['width'], row['height'], Path(frames_dir)/name))
    if calibration is None:
        raise ValueError(f'{log_path} has no calibration step')
    return calibration, frames


def summary(samples):
    valid = [s for s in samples if s.valid]
    if not valid:
        return {'kind': 'summary', 'frames': len(samples), 'valid': 0}
    mean = lambda values: [statistics.fmean(axis) for axis in zip(*values)]  # noqa: E731
    return {'kind': 'summary', 'frames': len(samples), 'valid': len(valid),
            'mean_position_mm': mean([s.position_mm for s in valid]),
            'mean_rotation_rad': mean([s.rotation_rad for s in valid])}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', help='linux_session_probe.py JSON-lines output')
    parser.add_argument('frames_dir')
    parser.add_argument('--display', type=float, nargs=9, default=ALBEDO_DISPLAY,
                        help='Display area corners in millimetres: top-left, top-right, bottom-left')
    parser.add_argument('--models', type=Path, default=DEFAULT_MODELS)
    args = parser.parse_args()
    import numpy as np
    from tobii_linux.face_tracking import FaceTracker
    from tobii_linux.head_pose_pipeline import DisplayArea, HeadPoseOutput
    (translation, angles), frames = recorded_session(args.log, args.frames_dir)
    area = DisplayArea(*(tuple(args.display[i:i + 3]) for i in (0, 3, 6)))
    output = HeadPoseOutput(translation, angles, area)
    tracker = FaceTracker(args.models)
    samples = []
    for timestamp, width, height, path in frames:
        pixels = np.frombuffer(path.read_bytes(), np.uint8).reshape(height, width)
        result = tracker.process(pixels, timestamp // 1000)
        fit = result.fit['rigid_state'] if result.fit is not None else None
        sample = output(fit, timestamp)
        samples.append(sample)
        print(json.dumps({'kind': 'sample', 'timestamp_us': timestamp, 'state': result.state,
                          'accepted': result.accepted, 'valid': sample.valid,
                          'position_mm': sample.position_mm, 'rotation_rad': sample.rotation_rad}))
    print(json.dumps(summary(samples)))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
