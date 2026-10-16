"""Confidence aggregation for complete D1 records and the observed caller gate.

The aggregation and gate are crates/tobii-inference/src/d1_quality.rs.
"""
from ._native import d1_quality as _quality


D1_IDS = frozenset(_quality.D1_IDS)


def d1_quality_score(quality, landmark_ids):
    """Average nonnegative qualities in FDP order after observed alias filling.

Complete D1 records all have their present/valid flags set by the writer.
The wrapper fills four additional record aliases, so this is not a plain
mean of the 57 neural scores. Missing or mixed-provider records are unsupported.
"""
    import numpy as np
    quality = np.asarray(quality, dtype=np.float32)
    ids = tuple(tuple(x) for x in landmark_ids)
    return _quality.d1_quality_score(quality.tobytes(), quality.shape, ids)


def passes_d1_quality(score, caller_mode):
    """Observed strict gate: modes 0/1 > 0.5, mode 2 > float32(0.35).

Mode meaning/selection is still a caller concern. Passing this gate alone
does not establish tracking validity or a successfully fitted public pose.
"""
    return _quality.passes_d1_quality(score, caller_mode)
