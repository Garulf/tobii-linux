"""Decode the observed version-4 fa.lbf, fc.lbf and LBF/lv layouts.

Based on the installed runtime's file readers; not a landmark predictor.
Tree payloads retain their original bytes because their semantics are unresolved.
Coefficient matrices preserve decoded file values, before runtime int8 quantization.
The readers are crates/tobii-models/src/lbf.rs.
"""
from ._native import lbf as _lbf


def _matrix(matrix):
    inputs, outputs, biases, weights, has_bias, maxima = matrix
    return {'shape': (inputs, outputs), 'biases': biases, 'weights': weights, 'has_bias': has_bias,
            'codebook_max_abs': tuple(maxima) if maxima is not None else None}


def read_lbf(path):
    """Read only the observed NN/fa.lbf and NN/fc.lbf format, requiring EOF."""
    version, points, tree_count, forest_end, stages, source_bytes = _lbf.read_lbf(path)
    return {'version': version,
            'points': [{'id': identifier, 'xy': xy, 'submodels': forests}
                       for identifier, xy, forests in points],
            'tree_count': tree_count, 'forest_end': forest_end,
            'stages': [[_matrix(m) for m in stage] for stage in stages], 'source_bytes': source_bytes}


def read_lv(path):
    """Read the separate version-4 LBF/lv layout (native loader 0x1400aadc0)."""
    version, records, source_bytes = _lbf.read_lv(path)
    return {'version': version,
            'records': [{'id': identifier, 'forest': forest,
                         'matrix_offset': offset, 'matrix': _matrix(matrix)}
                        for identifier, forest, offset, matrix in records],
            'source_bytes': source_bytes}


def quantize_matrix(matrix):
    """Reproduce the observed first-matrix int8 branch (finite positive ranges).

The scale uses the entire codebook, including entries not used by weights.
Bias stays float32. Division and multiplication each round to float32 before
truncation toward zero, matching the scalar SSE path.
"""
    inputs, outputs = matrix['shape']
    return _lbf.quantize_matrix(inputs, outputs, matrix['weights'], matrix['codebook_max_abs'])
