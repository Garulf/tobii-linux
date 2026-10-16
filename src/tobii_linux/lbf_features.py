"""Observed LBF pixel-pair tree traversal, without crop or pose assumptions.

The traversal is crates/tobii-headpose/src/lbf.rs.
"""
from ._native import lbf_features as _features


def forest_leaves(forest, pixels, rows, columns, stride, center0, center1, scale, axis1_sign=1):
    """Return per-tree leaf indices using zero-initialized native thresholds.

Axes follow buffer indexing: axis0 * stride + axis1. Runtime crop/axis mapping
and any later threshold mutation are not established. Inputs are bounded to
avoid native int32 overflow behavior outside observed image dimensions.
"""
    return tuple(_features.forest_leaves(forest, pixels, rows, columns, stride, center0, center1, scale,
                                         axis1_sign))
