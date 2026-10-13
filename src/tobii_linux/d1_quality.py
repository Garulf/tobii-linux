"""Confidence aggregation for complete D1 records and the observed caller gate."""
import math


D1_IDS = frozenset([(2,i) for i in (2,3)] + [(3,i) for i in range(1,15)]
    + [(4,i) for i in range(1,7)] + [(8,i) for i in range(1,11)]
    + [(9,i) for i in (3,4,5,15)] + [(14,i) for i in range(22,26)]
    + [(15,i) for i in range(1,18)])


def d1_quality_score(quality, landmark_ids):
    """Average nonnegative qualities in FDP order after observed alias filling.

Complete D1 records all have their present/valid flags set by the writer.
The wrapper fills four additional record aliases, so this is not a plain
mean of the 57 neural scores. Missing or mixed-provider records are unsupported.
"""
    import numpy as np
    ids = tuple(tuple(x) for x in landmark_ids)
    quality = np.asarray(quality, dtype=np.float32)
    if len(ids) != 57 or frozenset(ids) != D1_IDS or quality.shape != (57,) or not np.isfinite(quality).all():
        raise ValueError('Expected 57 finite complete D1 quality records')
    records = dict(zip(ids, quality))
    records[(2,1)] = records[(15,17)]
    records[(13,17)] = records[(15,17)]
    records[(14,21)] = records[(9,3)]
    records[(9,12)] = records[(14,23)]
    total = np.float32(0)
    count = 0
    for identifier in sorted(records):
        value = records[identifier]
        if value >= 0:
            total = total + value
            count += 1
    return float(total / np.float32(count)) if count else float('nan')


def passes_d1_quality(score, caller_mode):
    """Observed strict gate: modes 0/1 > 0.5, mode 2 > float32(0.35).

Mode meaning/selection is still a caller concern. Passing this gate alone
does not establish tracking validity or a successfully fitted public pose.
"""
    import numpy as np
    if caller_mode not in (0,1,2):
        return False
    score = float(np.float32(score))
    threshold = .5 if caller_mode in (0,1) else 0.3499999940395355
    return not math.isnan(score) and score > threshold
