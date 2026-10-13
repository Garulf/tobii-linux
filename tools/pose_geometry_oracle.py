#!/usr/bin/env python3
"""Compare the head-pose geometry stage and its inputs with native code."""
import json
import math
from pathlib import Path
import struct
import numpy as np
from vsmat_oracle import MatrixMachine
from tobii_linux import pose_geometry as geometry
ROOT = Path(__file__).resolve().parents[1]
SET_DISPLAY_AREA = 0x140293880
DISPLAY_MATRIX = 0x140294230
CAMERA_MODEL = 0x1402b00f0
GEOMETRY_CONFIG = 0x14028fc30
STAGE_CONSTRUCTOR = 0x1402945f0
STAGE_PROCESS = 0x140294670
SETTINGS_SIZE = 0x160
DISPLAY_FLAG = 0xe8
DISPLAY_OBJECT = 0xf0
CAMERA_MATRIX = 0x50
ALBEDO = 'albedo (Windows probe)'


def albedo_display():
    for line in (ROOT/'evidence/private/windows-geometry-probe.jsonl').read_text().splitlines():
        record = json.loads(line)
        if record.get('kind') == 'display_area':
            return record['top_left_mm'], record['bottom_left_mm'], record['top_right_mm']
    raise RuntimeError('No display area in the Windows probe')


def rectangle(center, width, height, tilt, yaw, roll):
    """Corners of a rectangle; the native setter rejects edges whose dot product exceeds 1 mm^2."""
    a, b, c = tilt, yaw, roll
    rx = np.array([[1, 0, 0], [0, math.cos(a), -math.sin(a)], [0, math.sin(a), math.cos(a)]])
    ry = np.array([[math.cos(b), 0, math.sin(b)], [0, 1, 0], [-math.sin(b), 0, math.cos(b)]])
    rz = np.array([[math.cos(c), -math.sin(c), 0], [math.sin(c), math.cos(c), 0], [0, 0, 1]])
    rotation = ry @ rx @ rz
    right, up = rotation[:, 0] * width / 2, rotation[:, 1] * height / 2
    center = np.asarray(center)
    corners = (center - right + up, center - right - up, center + right + up)
    return tuple(tuple(float(np.float32(v)) for v in corner) for corner in corners)


def displays():
    return {
        'upright': ((-250.0, 300.0, 0.0), (-250.0, 10.0, 0.0), (250.0, 300.0, 0.0)),
        'tilted_offset': rectangle((12.0, 170.0, 35.0), 520.0, 300.0, -0.35, 0.0, 0.0),
        'yawed_rolled': rectangle((-40.0, 210.0, 60.0), 610.0, 345.0, -0.2, 0.25, 0.04),
        ALBEDO: albedo_display(),
    }


CAMERAS = {
    'zero': ((0.0, 0.0, 0.0), (0.0, 0.0, 0.0)),
    'synthetic_a': ((10.0, -20.0, 5.0), (0.3, 0.1, -0.2)),
    'synthetic_b': ((-3.5, 12.25, 1.5), (-0.35, 0.02, 0.08)),
}
POSES = [
    ((0.0, 0.0, 0.0), (0.0, 0.0, 600.0)),
    ((0.15, -0.2 + math.pi, 0.05), (-40.0, 30.0, 600.0)),
    ((-0.3, 2.9, -0.4), (55.0, -10.0, 480.0)),
    ((0.12, -3.0, 0.2), (-120.0, 80.0, 900.0)),
]


def setting_display(m, settings, corners):
    module = m.allocate(0x60)
    m.vm.mem_write(module + 0x58, struct.pack('<Q', settings))
    points = m.allocate(48)
    m.vm.mem_write(points, struct.pack('<10f', *(v for c in corners for v in c), 0))
    area = m.allocate(16)
    m.vm.mem_write(area, struct.pack('<QQ', points, 1))
    m.call(SET_DISPLAY_AREA, module, area)
    return m.vm.mem_read(settings + DISPLAY_FLAG, 1)[0] == 1


def camera_record(translation, angles):
    record = bytearray(0x140)
    struct.pack_into('<HH', record, 0x10, 1280, 960)
    struct.pack_into('<5f', record, 0x14, 0.003, 0.003, 4.0, 0.05, -0.04)
    struct.pack_into('<6f', record, 0x3c, *translation, *angles)
    return bytes(record)


def original_matrices(binary, corners, camera):
    m = MatrixMachine(binary)
    settings = m.allocate(SETTINGS_SIZE)
    record = m.allocate(0x140)
    m.vm.mem_write(record, camera_record(*camera))
    m.call(CAMERA_MODEL, settings, record)
    if not setting_display(m, settings, corners):
        raise RuntimeError(f'Native setter rejected display corners {corners}')
    display = m.allocate(64)
    m.call(DISPLAY_MATRIX, display, settings + DISPLAY_OBJECT)
    return m, settings, display


def original_stage(m, settings, display, poses):
    config = m.allocate(0x68)
    m.call(GEOMETRY_CONFIG, 0, config)
    stage = m.allocate(0x70)
    m.call(STAGE_CONSTRUCTOR, stage, config)
    pose, output = m.allocate(0x20), m.allocate(0x20)
    results = []
    for angles, position in poses:
        m.vm.mem_write(pose, struct.pack('<3fI3f', *angles, 3, *position))
        m.vm.mem_write(output, bytes(0x20))
        m.call(STAGE_PROCESS, stage, output, pose, settings + CAMERA_MATRIX, display)
        values = m.floats(output, 7)
        results.append((values[:3], values[4:]))
    config3 = np.asarray(m.floats(config, 9), dtype=np.float32).reshape(3, 3)
    config4 = np.asarray(m.floats(config + 0x24, 16), dtype=np.float32).reshape(4, 4)
    return results, (config3, config4)


def ulp_distance(a, b):
    a = np.asarray(a, dtype=np.float32).view(np.int32).astype(np.int64)
    b = np.asarray(b, dtype=np.float32).view(np.int32).astype(np.int64)
    a = np.where(a < 0, -(a & 0x7fffffff), a)
    b = np.where(b < 0, -(b & 0x7fffffff), b)
    return int(np.max(np.abs(a - b)))


def compare(expected, actual):
    expected, actual = np.asarray(expected, dtype=np.float32), np.asarray(actual, dtype=np.float32)
    return {'max_error': float(np.max(np.abs(expected - actual))), 'max_ulps': ulp_distance(expected, actual),
            'exact': bool(np.array_equal(expected.view(np.int32), actual.view(np.int32)))}


def reordered_rejection(binary, corners):
    top_left, bottom_left, top_right = corners
    m = MatrixMachine(binary)
    settings = m.allocate(SETTINGS_SIZE)
    return not setting_display(m, settings, (top_left, top_right, bottom_left))


def main():
    binary = (ROOT/'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    report = {'functions': {name: hex(address) for name, address in (
        ('set_display_area', SET_DISPLAY_AREA), ('display_matrix', DISPLAY_MATRIX),
        ('camera_model', CAMERA_MODEL), ('geometry_config', GEOMETRY_CONFIG),
        ('stage_constructor', STAGE_CONSTRUCTOR), ('stage_process', STAGE_PROCESS))},
        'corner_order': 'top_left, bottom_left, top_right', 'cases': []}
    passed = True
    config_checked = False
    for display_name, corners in displays().items():
        rejected = reordered_rejection(binary, corners)
        for camera_name, camera in CAMERAS.items():
            m, settings, display = original_matrices(binary, corners, camera)
            native_display = geometry.display_matrix(*corners)
            native_camera = geometry.camera_extrinsic(*camera)
            original_display = np.asarray(m.floats(display, 16), dtype=np.float32).reshape(4, 4)
            original_camera = np.asarray(m.floats(settings + CAMERA_MATRIX, 16), dtype=np.float32).reshape(4, 4)
            results, config = original_stage(m, settings, display, POSES)
            if not config_checked:
                native_config = geometry.geometry_config()
                report['geometry_config'] = {'3x3': compare(config[0], native_config[0]),
                                             '4x4': compare(config[1], native_config[1])}
                passed &= all(c['exact'] for c in report['geometry_config'].values())
                config_checked = True
            poses = []
            for (angles, position), (want_rotation, want_position) in zip(POSES, results):
                got_rotation, got_position = geometry.geometry_stage(angles, position, native_camera, native_display)
                rotation, translation = compare(want_rotation, got_rotation), compare(want_position, got_position)
                poses.append({'angles': angles, 'position': position,
                              'original_rotation': want_rotation, 'original_position': want_position,
                              'rotation': rotation, 'position_match': translation})
                passed &= rotation['max_error'] <= 1e-6 and translation['exact']
            case = {'display': display_name, 'corners': corners, 'camera': camera_name,
                    'camera_inputs': camera, 'reordered_corners_rejected': rejected,
                    'original_display_matrix': original_display.tolist(),
                    'display_matrix': compare(original_display, native_display),
                    'camera_matrix': compare(original_camera, native_camera), 'poses': poses}
            passed &= rejected and case['display_matrix']['exact'] and case['camera_matrix']['exact']
            report['cases'].append(case)
    report['passed'] = bool(passed)
    (ROOT/'evidence/private/pose-geometry-oracle.json').write_text(json.dumps(report, indent=2))
    for case in report['cases']:
        rotation = max(p['rotation']['max_error'] for p in case['poses'])
        position = max(p['position_match']['max_ulps'] for p in case['poses'])
        print(f"{case['display']:24} {case['camera']:12} "
              f"reordered_rejected={case['reordered_corners_rejected']} "
              f"display_exact={case['display_matrix']['exact']} camera_exact={case['camera_matrix']['exact']} "
              f"rotation_error={rotation:.3g} position_ulps={position}")
    print('config', report['geometry_config'])
    print('PASS' if passed else 'FAIL')
    raise SystemExit(0 if passed else 1)


if __name__ == '__main__':
    main()
