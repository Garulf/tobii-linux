"""Observed LBF pixel-pair tree traversal, without crop or pose assumptions."""
import math
import struct


def forest_leaves(forest, pixels, rows, columns, stride, center0, center1, scale, axis1_sign=1):
    """Return per-tree leaf indices using zero-initialized native thresholds.

Axes follow buffer indexing: axis0 * stride + axis1. Runtime crop/axis mapping
and any later threshold mutation are not established. Inputs are bounded to
avoid native int32 overflow behavior outside observed image dimensions.
"""
    if not 0 < rows <= 32768 or not 0 < columns <= stride <= 32768:
        raise ValueError('Invalid image dimensions')
    if len(pixels) < (rows - 1) * stride + columns or axis1_sign not in (-1, 1):
        raise ValueError('Invalid image buffer or axis sign')
    values = (center0, center1, scale)
    if any(not math.isfinite(x) or abs(x) > 32768 for x in values) or scale < 0:
        raise ValueError('Unsupported center/scale')
    # Native cvttss2si first rounds the argument to float32, then truncates.
    c0, c1, distance = [math.trunc(struct.unpack('<f', struct.pack('<f', x))[0]) for x in values]
    result = []
    for marker, word, depth, payload in forest[2]:
        if not 0 <= depth <= 20 or len(payload) != 4 * (1 << depth):
            raise ValueError('Invalid tree payload')
        node = 0
        for _ in range(depth):
            a0, a1, b0, b1 = struct.unpack_from('<bbbb', payload, node * 4)
            row_a = min(rows - 1, max(0, c0 + ((a0 * distance) >> 8)))
            col_a = min(columns - 1, max(0, c1 + ((a1 * distance * axis1_sign) >> 8)))
            row_b = min(rows - 1, max(0, c0 + ((b0 * distance) >> 8)))
            col_b = min(columns - 1, max(0, c1 + ((b1 * distance * axis1_sign) >> 8)))
            difference = pixels[row_a * stride + col_a] - pixels[row_b * stride + col_b]
            node = 2 * node + (2 if difference > 0 else 1)
        result.append(node - (1 << depth) + 1)
    return tuple(result)
