"""Recovered LBF stages with explicit buffer geometry; not a camera tracker.

The stages are crates/tobii-headpose/src/lbf.rs. The hidden activations use the
platform libm tanhf; the native reciprocal/tanh approximations can differ
slightly, so this is not bit-exact predictor parity.
"""
import struct

from ._native import lbf_predictor as _predictor


def f32(value):
    return struct.unpack('<f', struct.pack('<f', value))[0]


def gather_features(model, stage, pixels, rows, columns, stride, centers, scale, mirrored=False):
    features, stage_scale = _predictor.gather_features(model, stage, pixels, rows, columns, stride,
                                                       [tuple(c) for c in centers], scale, mirrored)
    return tuple(features), stage_scale


def predict_stage(matrices, features):
    """Sparse int8 first matrix, tanh hidden activations, float32 dense stages."""
    import numpy as np
    indices = np.asarray(features, dtype=np.int64)
    if indices.ndim != 1:
        raise ValueError('Feature index outside predictor')
    return np.frombuffer(_predictor.predict_stage(matrices, indices.tolist()), dtype=np.float32).copy()


def refine_landmarks(model, pixels, rows, columns, stride, centers, scale, mirrored=False):
    """Run all observed stages from caller-supplied initial landmarks and scale."""
    return tuple(_predictor.refine_landmarks(model, pixels, rows, columns, stride, [tuple(c) for c in centers],
                                             scale, mirrored))
