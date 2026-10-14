#!/usr/bin/env python3
"""Check the display-area model and the 1440 request against original code in emulation.

Two groups, offline Unicorn runs of hash-bound original functions:

- Stream Engine's tobii_calculate_display_area_basic (0x18007da30), the
  native form of the model the configuration app's .NET
  DisplayAreaUtils.ComposeDisplayArea uses, against
  display_setup.compose_display_area on albedo's monitor and mounting and on
  synthetic sizes, offsets and mountings. The native code works in float32
  and the app in double, so corners are compared within 0.001 mm and exact
  float32 agreement is reported separately.
- The runtime's 1440 builder 0x1402c6f40 with the corners albedo's device
  returned for 1430 against gaze_device.set_display_area.

Nothing is sent to a device. See docs/display-setup.md.
"""
import json
from pathlib import Path
import struct

from unicorn.x86_const import (UC_X86_REG_RAX, UC_X86_REG_RCX, UC_X86_REG_RSP, UC_X86_REG_XMM1,
                              UC_X86_REG_XMM2, UC_X86_REG_XMM3)
from gaze_calibration_oracle import STACK, STOP, STREAM_ENGINE, StreamEngine
from gaze_device_oracle import floats32, run
from ttp_commands_oracle import BINARY, CAPACITY
from tobii_linux import display_setup as ds
from tobii_linux import gaze_device as gd

ROOT = Path(__file__).resolve().parents[1]
REPORT = ROOT/'evidence/private/display-setup-oracle.json'
CALCULATE_DISPLAY_AREA_BASIC = 0x18007DA30
SET_DISPLAY_AREA_BUILDER = 0x1402C6F40
MOUNTING = struct.Struct('<i2f3f3f')
TOLERANCE_MM = 1e-3
ALBEDO_1430 = ds.DisplayArea((-296.6200866699219, 326.0040588378906, 111.8187484741211),
                             (300.3799133300781, 326.0040588378906, 111.8187484741211),
                             (-296.6200866699219, 10.267330169677734, -3.100020170211792))


def calculate_display_area_basic(engine, size_mm, offset_mm, mounting):
    vm = engine.vm
    packed = MOUNTING.pack(mounting.guides, mounting.width_mm, mounting.angle_deg,
                           *mounting.external_offset_mm, *mounting.internal_offset_mm)
    geometry, output = engine.put(packed), engine.put(b'\0' * 36)
    stack = STACK + 0x1F000
    vm.mem_write(stack, struct.pack('<Q', STOP))
    vm.mem_write(stack + 0x28, struct.pack('<QQ', geometry, output))
    vm.reg_write(UC_X86_REG_RSP, stack)
    vm.reg_write(UC_X86_REG_RCX, engine.put(b'\0' * 0x100))
    for register, number in zip((UC_X86_REG_XMM1, UC_X86_REG_XMM2, UC_X86_REG_XMM3), (*size_mm, offset_mm)):
        vm.reg_write(register, struct.unpack('<I', struct.pack('<f', number))[0])
    vm.emu_start(CALCULATE_DISPLAY_AREA_BASIC, STOP, timeout=20_000_000, count=5_000_000)
    status = vm.reg_read(UC_X86_REG_RAX) & 0xFFFFFFFF
    return status, ds.DisplayArea.from_floats(struct.unpack('<9f', bytes(vm.mem_read(output, 36))))


def model_cases():
    albedo = ds.decompose_display_area(ALBEDO_1430)
    yield 'albedo_edid_and_device_offset', (597.0, 336.0), albedo.tracker_offset_mm, ds.ET5_MOUNTING
    yield 'albedo_offset_rounded', (597.0, 336.0), -1.88, ds.ET5_MOUNTING
    yield 'centred_24_inch', ds.size_from_diagonal(24.0), 0.0, ds.ET5_MOUNTING
    yield 'off_centre_34_inch_ultrawide', ds.size_from_diagonal(34.0, (21, 9)), 37.5, ds.ET5_MOUNTING
    yield 'portrait', (336.0, 597.0), -12.25, ds.ET5_MOUNTING
    yield 'other_mounting', (520.0, 290.0), 4.0, ds.MountingGeometry(1, 252.0, 35.0, (1.5, -2.0, 7.25),
                                                                     (-0.5, 3.0, -4.0))
    yield 'no_tilt', (300.0, 200.0), 0.0, ds.MountingGeometry(0, 184.0, 0.0, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0))


def run_model(engine):
    cases = []
    for name, size, offset, mounting in model_cases():
        status, original = calculate_display_area_basic(engine, size, offset, mounting.as_float32())
        native = ds.compose_display_area(size, mounting, offset)
        largest = max(abs(a - b) for a, b in zip(original.floats, native.floats))
        cases.append({'case': name, 'size_mm': list(size), 'tracker_offset_mm': offset, 'status': status,
                      'original': list(original.floats), 'native': list(native.floats),
                      'max_difference_mm': largest, 'float32_equal': original == native,
                      'match': status == 0 and largest < TOLERANCE_MM})
    return cases


def run_builder():
    original = run(SET_DISPLAY_AREA_BUILDER,
                   (14, floats32(*ALBEDO_1430.top_left), floats32(*ALBEDO_1430.top_right),
                    floats32(*ALBEDO_1430.bottom_left), 0, 0, 'buffer', CAPACITY), ())
    native = gd.set_display_area(14, *ALBEDO_1430.corners).encode()
    return {'case': 'albedo_1430_corners', 'original': original.hex(), 'native': native.hex(),
            'match': original == native}


def main():
    run.binary = BINARY.read_bytes()
    engine = StreamEngine(STREAM_ENGINE.read_bytes())
    model, builder = run_model(engine), run_builder()
    failures = [case['case'] for case in model + [builder] if not case['match']]
    REPORT.write_text(json.dumps({'evidence': 'offline Unicorn emulation of original Stream Engine and runtime code',
                                  'passed': not failures, 'model_cases': model, 'builder_case': builder}, indent=2))
    for case in model:
        print(f"{case['case']}: max {case['max_difference_mm']:.2e} mm, float32 equal {case['float32_equal']}, "
              f"match {case['match']}")
    print(f"{builder['case']}: builder match {builder['match']}")
    print('PASS' if not failures else f'FAIL {failures}')
    raise SystemExit(0 if not failures else 1)


if __name__ == '__main__':
    main()
