"""Decode the observed version-4 fa.lbf, fc.lbf and LBF/lv layouts.

Based on the installed runtime's file readers; not a landmark predictor.
Tree payloads retain their original bytes because their semantics are unresolved.
Coefficient matrices preserve decoded file values, before runtime int8 quantization.
"""
import math
from pathlib import Path
import struct


class Reader:
    def __init__(self, data):
        self.data = data
        self.offset = 0

    def take(self, size):
        if size < 0 or size > len(self.data) - self.offset:
            raise ValueError(f'Truncated LBF at {self.offset}')
        start = self.offset
        self.offset += size
        return self.data[start:self.offset]

    def unpack(self, fmt):
        return struct.unpack(fmt, self.take(struct.calcsize(fmt)))

    def count(self, maximum=100000):
        value, = self.unpack('<i')
        if not 0 < value <= maximum:
            raise ValueError('Unsupported LBF count')
        return value

    def floats(self, count):
        data = self.take(count * 4)
        if any(not math.isfinite(x[0]) for x in struct.iter_unpack('<f', data)):
            raise ValueError('Non-finite LBF value')
        return data


def _forest(reader):
    first, = reader.unpack('<i')
    count = reader.count(10000)
    third, = reader.unpack('<i')
    trees = []
    for _ in range(count):
        marker, = reader.unpack('<I')
        word = reader.unpack('<I')[0] if marker in (0xb00b5, 0xc00b5) else marker
        bits, = reader.unpack('<I')
        if bits > 20:
            raise ValueError('Unsupported tree extent')
        payload = reader.take(4 * (1 << bits))
        trees.append((marker, word, bits, payload))
    return first, third, trees


def _matrix(reader, compressed, has_bias):
    inputs, outputs = reader.count(), reader.count()
    if inputs * outputs > 10_000_000:
        raise ValueError('Excessive matrix extent')
    if compressed:
        maxima = []
        biases = bytearray(outputs * 4)
        weights = bytearray(inputs * outputs * 4)
        for column in range(outputs):
            codebook = reader.floats(16)
            maxima.append(max(abs(v[0]) for v in struct.iter_unpack("<f", codebook)))
            values = inputs + int(has_bias)
            packed = reader.take((values + 1) // 2)
            for j in range(values):
                code = (packed[j // 2] >> ((j % 2) * 4)) & 15
                value = codebook[code * 4:code * 4 + 4]
                if has_bias and j == 0:
                    biases[column * 4:column * 4 + 4] = value
                else:
                    offset = ((j - int(has_bias)) * outputs + column) * 4
                    weights[offset:offset + 4] = value
    else:
        biases = reader.floats(outputs) if has_bias else bytes(outputs * 4)
        weights = reader.floats(inputs * outputs)
    return {'shape': (inputs, outputs), 'biases': bytes(biases),
            'weights': bytes(weights), 'has_bias': has_bias,
            'codebook_max_abs': tuple(maxima) if compressed else None}


def _end(reader):
    if reader.offset != len(reader.data):
        raise ValueError(f'Unexpected trailing LBF bytes: {len(reader.data) - reader.offset}')


def read_lbf(path):
    """Read only the observed NN/fa.lbf and NN/fc.lbf format, requiring EOF."""
    reader = Reader(Path(path).read_bytes())
    version, = reader.unpack('<B')
    if version != 4:
        raise ValueError('Only observed version-4 LBF is supported')
    landmarks = reader.count(1000)
    stage_count = reader.count(100)
    matrix_count = reader.count(100)
    if matrix_count != 3:
        raise ValueError('Unsupported LBF layout (this reader excludes LBF/lv)')
    points = []
    tree_count = 0
    for _ in range(landmarks):
        group, index, present = reader.unpack('<iii')
        if present not in (0, 1):
            raise ValueError('Invalid LBF presence flag')
        xy = struct.unpack('<ff', reader.floats(2))
        submodels = []
        if present:
            for _ in range(reader.count(100)):
                forest = _forest(reader)
                tree_count += len(forest[2])
                submodels.append(forest)
        points.append({'id': (group, index), 'xy': xy, 'submodels': submodels})
    forest_end = reader.offset
    stages = []
    for _ in range(stage_count):
        matrices = []
        for matrix_index in range(matrix_count):
            matrices.append(_matrix(reader, matrix_index == 0, True))
        stages.append(matrices)
    _end(reader)
    return {'version': version, 'points': points, 'tree_count': tree_count,
            'forest_end': forest_end, 'stages': stages, 'source_bytes': len(reader.data)}


def read_lv(path):
    """Read the separate version-4 LBF/lv layout (native loader 0x1400aadc0)."""
    reader = Reader(Path(path).read_bytes())
    version, = reader.unpack('<B')
    if version != 4:
        raise ValueError('Only observed version-4 LV is supported')
    count = reader.count(1000)
    records = []
    for _ in range(count):
        group, index, present = reader.unpack('<iii')
        if present not in (0, 1):
            raise ValueError('Invalid LV presence flag')
        forest = _forest(reader) if present else None
        matrix_offset = reader.offset
        matrix = _matrix(reader, True, False)
        records.append({'id': (group, index), 'forest': forest,
                        'matrix_offset': matrix_offset, 'matrix': matrix})
    _end(reader)
    return {'version': version, 'records': records, 'source_bytes': len(reader.data)}


def quantize_matrix(matrix):
    """Reproduce the observed first-matrix int8 branch (finite positive ranges).

The scale uses the entire codebook, including entries not used by weights.
Bias stays float32. Division and multiplication each round to float32 before
truncation toward zero, matching the scalar SSE path.
"""
    def f32(value):
        return struct.unpack('<f', struct.pack('<f', value))[0]
    maxima = matrix['codebook_max_abs']
    if maxima is None or any(not math.isfinite(v) or v <= 0 for v in maxima):
        raise ValueError('Quantization requires nonzero finite codebooks')
    scales = [f32(math.trunc(f32(127.0 / value))) for value in maxima]
    outputs = matrix['shape'][1]
    weights = bytearray()
    for i, (value,) in enumerate(struct.iter_unpack('<f', matrix['weights'])):
        quantized = math.trunc(f32(value * scales[i % outputs]))
        if not -127 <= quantized <= 127:
            raise ValueError('Quantization out of observed range')
        weights.append(quantized & 255)
    return bytes(weights), struct.pack('<' + 'f' * len(scales), *scales)
