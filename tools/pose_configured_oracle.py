#!/usr/bin/env python3
"""Compare all 69 installed fitting states against original machine code."""
import json
from pathlib import Path
import struct
import numpy as np
from pose_projection_oracle import make_model, SHA256
from tobii_linux.pose_model import load_pose_model
from tobii_linux.pose_linalg import inverse_float32
from tobii_linux.pose_projection import model_jacobian
from tobii_linux.pose_fit import prepare_observations
ROOT = Path(__file__).resolve().parents[1]


def main():
    model = load_pose_model(ROOT / 'models/tobii-et5/support')
    binary = (ROOT / 'evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe').read_bytes()
    start = model.initial_state([0, np.pi, 0, 0, 0, .65])
    m, obj, state, _, _, _ = make_model(binary, model.vertices, start[:3], start[3:6], model.focal_factor, 1,
        (model.shape_basis, model.animation_basis, start[6:46], start[46:], model.shape_ranges, model.animation_ranges))
    def matrix_data(offset):
        header = struct.unpack('<Q', m.vm.mem_read(obj + offset, 8))[0]
        return struct.unpack('<Q', m.vm.mem_read(header + 0x18, 8))[0]
    for offset, mask in ((0x70, model.shape_mask), (0x48, model.animation_mask)):
        m.vm.mem_write(matrix_data(offset), np.asarray(mask, dtype='<i4').tobytes())
    m.vm.mem_write(matrix_data(0x90), model.state_sensitivity.tobytes())
    _, weights = prepare_observations(np.full((57, 2), .5), model.landmark_ids, 640, 480)
    m.vm.mem_write(matrix_data(0x98), weights.tobytes())
    # Encode physical zero coefficients through the original log-ratio routine.
    rotation = m.allocate(12); translation = m.allocate(12)
    m.vm.mem_write(rotation, start[:3].tobytes()); m.vm.mem_write(translation, start[3:6].tobytes())
    shape, _ = m.matrix(40, 1, [0] * 40); animation, _ = m.matrix(23, 1, [0] * 23)
    m.call(0x1400dfbf0, obj, rotation, translation, shape, animation, state)
    encoded_data = struct.unpack('<Q', m.vm.mem_read(state + 0x18, 8))[0]
    encoded = np.asarray(m.floats(encoded_data, 69), dtype=np.float32)
    encode_error = float(np.max(np.abs(encoded-start)))
    np.testing.assert_allclose(encoded, start, rtol=1e-6, atol=1e-7)
    print('encode', encode_error, flush=True)
    target = start.copy(); target[:6] += np.array([.025, -.03, .02, .008, -.005, -.02], dtype=np.float32)
    _, observed = model_jacobian(model.vertices, target[:3], target[3:6], model.focal_factor, 1,
        model.shape_basis, target[6:46], model.shape_ranges, model.animation_basis, target[46:], model.animation_ranges,
        model.shape_mask, model.animation_mask)
    fitter = m.allocate(0x98); m.call(0x1400df430, fitter, obj)
    smat = struct.unpack('<Q', m.vm.mem_read(fitter+0x10, 8))[0]
    data = struct.unpack('<Q', m.vm.mem_read(smat+0x18, 8))[0]
    m.vm.mem_write(data, encoded.tobytes()); m.vm.mem_write(fitter+9, b'\1')
    obs, _ = m.matrix(114, 1, observed.flatten())
    cases = []
    for iteration in range(3):
        before = np.asarray(m.floats(data, 69), dtype=np.float32)
        expected = model.step(observed, before, weights)
        m.call(0x1400df6b0, fitter, obs, instruction_limit=30_000_000)
        actual = np.asarray(m.floats(data, 69), dtype=np.float32)
        error = float(np.max(np.abs(actual-expected)))
        print('step', iteration, error, flush=True)
        if error > 1e-5:
            jac, pred = model_jacobian(model.vertices, before[:3], before[3:6], model.focal_factor, 1,
                model.shape_basis, before[6:46], model.shape_ranges, model.animation_basis, before[46:], model.animation_ranges,
                model.shape_mask, model.animation_mask)
            excluded = np.flatnonzero(weights[::2]>=100000)
            jac[2*excluded]=0; jac[2*excluded+1]=0
            weighted=jac.T*(np.float32(1)/weights)
            normal=(weighted.astype(np.float64)@jac.astype(np.float64)+np.diag(np.float32(1)/model.state_sensitivity)).astype(np.float32)
            inverse=inverse_float32(normal)
            gain=(inverse.astype(np.float64)@weighted.astype(np.float64)).astype(np.float32)
            arrays={}
            for offset, name, native in ((0x58,'prediction',pred.reshape(-1,1)),(0x68,'jacobian',jac),
                    (0x70,'weighted',weighted),(0x20,'normal',normal),(0x28,'inverse',inverse),(0x78,'gain',gain)):
                header=struct.unpack('<Q',m.vm.mem_read(fitter+offset,8))[0]
                pointer=struct.unpack('<Q',m.vm.mem_read(header+0x18,8))[0]
                original=np.asarray(m.floats(pointer,native.size),dtype=np.float32).reshape(native.shape)
                print(name,float(np.max(np.abs(original-native))),flush=True)
                arrays[name+'_original']=original;arrays[name+'_native']=native
            np.savez(ROOT/'evidence/private/pose-configured-matrices.npz',**arrays)
        np.testing.assert_allclose(actual, expected, rtol=1e-5, atol=1e-5)
        cases.append({'iteration': iteration, 'max_absolute_error': error, 'passes': True})
    report = {'runtime_sha256': SHA256, 'entries': ['0x1400dfbf0', '0x1400df6b0'],
        'state_count': 69, 'landmark_count': 57, 'encode_max_error': encode_error, 'cases': cases,
        'limitations': 'Installed normalized mesh, masks, ranges, sensitivities and focal factor. Synthetic observations. Model structures supplied by harness; original config/text loading, outer tracking policy and live hardware not exercised.'}
    (ROOT/'evidence/private/pose-configured-oracle.json').write_text(json.dumps(report, indent=2)+'\n')
if __name__ == '__main__':
    main()
