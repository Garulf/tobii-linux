"""Recovered yaw-dependent LBF refinement 0x14007d270 that follows a successful fit.

VNNTrackerCore resets the tracker when this step returns false. The step fails
only when the working image carries an IplImage ROI that does not fit inside
the image (0x1400ac9c0); otherwise the four LBF stages always run. Refined
coordinates are written back only for points of FDP group 10.
"""
import math

MIRROR_YAW_DEGREES = 15.0
REFINED_GROUP = 10
REFINED_DEPTH = 1000.0
REFINED_QUALITY = -1.0
NEUTRAL_YAW_DEGREES = 180.0
RADIANS_TO_DEGREES = 57.2957763671875

# 0x140011900 for every FDP id (index 1..count per group); -1 means no mirror.
# Tabulated by executing the original function over the runtime's FDP count
# table at 0x14112f050; tools/eye_refine_oracle.py rechecks every entry.
MIRRORED_INDEX = {
    2: (1, 2, 3, 5, 4, 7, 6, 9, 8, -1, -1, -1, -1, -1),
    3: (2, 1, 4, 3, 6, 5, 12, 11, 10, 9, 8, 7, 14, 13),
    4: (2, 1, 4, 3, 6, 5),
    5: (-1,) * 4,
    6: (-1,) * 4,
    7: (-1,),
    8: (1, 2, 4, 3, 6, 5, 8, 7, 10, 9),
    9: (2, 1, 3, 5, 4) + (-1,) * 9 + (15,),
    10: (2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 18, 17, 20, 19, 22, 21, 24, 23),
    11: (-1,) * 6,
    12: (-1,) * 4 + (10, 9, 12, 11, 6, 5, 8, 7) + (-1,) * 35,
    13: (2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 17),
    14: (2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 18, 17, 20, 19, 21, 22, 23, 24, 25, -1),
    15: (2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 17),
}


def _f32(value):
    import numpy as np
    return np.float32(value)


def mirrored_index(group, index):
    table = MIRRORED_INDEX.get(group)
    if table is None or not 1 <= index <= len(table):
        raise ValueError('FDP id outside the runtime count table')
    return table[index - 1]


def tracker_yaw_degrees(rigid_state):
    """Yaw passed by 0x140049450: 180 - (ry - atan2(tx, tz)) in degrees, float32."""
    import numpy as np
    pose = np.asarray(rigid_state, dtype=np.float32)
    bearing = _f32(math.atan2(float(pose[3]), float(pose[5])))
    return _f32(_f32(NEUTRAL_YAW_DEGREES) - _f32(_f32(pose[1] - bearing) * _f32(RADIANS_TO_DEGREES)))


def select_lbf_model(yaw_degrees):
    """Return (model name, mirrored). COMISS makes a NaN yaw select fa unmirrored."""
    yaw = _f32(yaw_degrees)
    mirrored = bool(yaw > _f32(MIRROR_YAW_DEGREES))
    if abs(yaw) >= _f32(MIRROR_YAW_DEGREES):
        return 'fc', mirrored
    return 'fa', False


def roi_inside(width, height, roi):
    """0x1400ac9c0 guard; roi is (x, y, width, height) or None for no ROI."""
    if roi is None:
        return True
    x, y, roi_width, roi_height = roi
    return not (x < 0 or x + roi_width > width or y < 0 or y + roi_height > height)


def lbf_refiner(models, pixels, stride=None):
    """Callable running the four LBF stages (0x1400acb80) on an 8-bit image.

The ROI, when present, offsets the pixel origin and replaces the dimensions
while the landmark coordinates stay in full-image pixels, as in the original.
"""
    import numpy as np
    from .lbf_predictor import refine_landmarks
    image = np.asarray(pixels, dtype=np.uint8)
    def refine(name, xs, ys, scale, mirrored, roi):
        x, y, columns, rows = roi if roi is not None else (0, 0, image.shape[1], image.shape[0])
        region = image[y:y + rows, x:x + columns]
        refined = refine_landmarks(models[name], np.ascontiguousarray(region).tobytes(), rows, columns,
                                   columns, list(zip(ys, xs)), scale, mirrored)
        return [p[1] for p in refined], [p[0] for p in refined]
    return refine


def refine_eye_landmarks(point_ids, records, width, height, scale, yaw_degrees, refine, roi=None):
    """Reproduce 0x14007d270 for the LBF point table point_ids (fa's ID order).

records maps FDP ids to normalized (x, y); every point must be present because
the original reads absent records' stale memory. Returns (valid, written)
where written maps group-10 ids to record fields (x, y, z, present, flag, quality).
"""
    name, mirrored = select_lbf_model(yaw_degrees)
    w, h = _f32(width), _f32(height)
    xs, ys = [], []
    for group, index in point_ids:
        source = mirrored_index(group, index) if mirrored else index
        x, y = records[(group, source)]
        xs.append(_f32(w * _f32(x)))
        ys.append(_f32(_f32(y) * h))
    if not roi_inside(width, height, roi):
        return False, {}
    xs, ys = refine(name, xs, ys, scale, mirrored and name == 'fc', roi)
    written = {}
    for (group, index), x, y in zip(point_ids, xs, ys):
        if group != REFINED_GROUP:
            continue
        target = mirrored_index(group, index) if mirrored else index
        written[(group, target)] = (_f32(_f32(x) / w), _f32(_f32(y) / h), _f32(REFINED_DEPTH), 1, 1,
                                   _f32(REFINED_QUALITY))
    return True, written
