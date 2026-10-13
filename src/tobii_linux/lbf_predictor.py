"""Recovered LBF stages with explicit buffer geometry; not a camera tracker.

NumPy is optional and imported only for prediction. Native reciprocal/tanh
approximations can differ slightly; this is not bit-exact predictor parity.
"""
import struct
from .lbf import quantize_matrix
from .lbf_features import forest_leaves


def f32(value):
    return struct.unpack('<f', struct.pack('<f', value))[0]


def gather_features(model, stage, pixels, rows, columns, stride, centers, scale, mirrored=False):
    if len(centers) != len(model['points']) or not 0 <= stage < len(model['stages']):
        raise ValueError('Landmark count or stage mismatch')
    first = next(p['submodels'] for p in model['points'] if p['submodels'])
    factor = struct.unpack('<f', struct.pack('<I', first[stage][1] & 0xffffffff))[0]
    stage_scale = f32(f32(scale) * factor)
    result = []
    base = 0
    for point, (c0, c1) in zip(model['points'], centers):
        if not point['submodels']:
            continue
        forest = point['submodels'][stage]
        leaves = forest_leaves(forest, pixels, rows, columns, stride, c0, c1, stage_scale, -1 if mirrored else 1)
        for leaf, tree in zip(leaves, forest[2]):
            result.append(base + leaf)
            base += 1 << tree[2]
    if base != model['stages'][stage][0]['shape'][0]:
        raise ValueError('Feature extent does not match predictor')
    return tuple(result), stage_scale


def predict_stage(matrices, features):
    """Sparse int8 first matrix, tanh hidden activations, float32 dense stages."""
    import numpy as np
    first = matrices[0]
    inputs, outputs = first['shape']
    indices = np.asarray(features, dtype=np.int64)
    if indices.ndim != 1 or np.any(indices < 0) or np.any(indices >= inputs):
        raise ValueError('Feature index outside predictor')
    weights, scales = quantize_matrix(first)
    qweights = np.frombuffer(weights, dtype=np.int8).reshape(inputs, outputs)
    sums = qweights[indices].sum(axis=0, dtype=np.int32)
    values = sums.astype(np.float32) / np.frombuffer(scales, dtype='<f4')
    values += np.frombuffer(first['biases'], dtype='<f4')
    for matrix in matrices[1:]:
        inputs, outputs = matrix['shape']
        if values.shape != (inputs,):
            raise ValueError('Disconnected stage matrices')
        values = np.tanh(values)
        weights = np.frombuffer(matrix['weights'], dtype='<f4').reshape(inputs, outputs)
        output = np.frombuffer(matrix['biases'], dtype='<f4').copy()
        # Preserve native multiply/add order instead of BLAS/FMA reassociation.
        for i in range(inputs):
            output += weights[i] * values[i]
        values = output
    if not np.isfinite(values).all():
        raise ValueError('Non-finite predictor result')
    return values


def refine_landmarks(model, pixels, rows, columns, stride, centers, scale, mirrored=False):
    """Run all observed stages from caller-supplied initial landmarks and scale."""
    current = [(f32(a), f32(b)) for a, b in centers]
    for stage, matrices in enumerate(model['stages']):
        features, stage_scale = gather_features(model, stage, pixels, rows, columns, stride, current, scale, mirrored)
        delta = predict_stage(matrices, features)
        if len(delta) != 2 * len(current):
            raise ValueError('Landmark output count mismatch')
        current = [(f32(a + f32(float(delta[2*i]) * stage_scale)),
                    f32(b + f32(f32(float(delta[2*i+1]) * (-1 if mirrored else 1)) * stage_scale)))
                   for i, (a, b) in enumerate(current)]
    return tuple(current)
