"""Recovered yaw-dependent LBF refinement 0x14007d270 that follows a successful fit.

VNNTrackerCore resets the tracker when this step returns false. The step fails
only when the working image carries an IplImage ROI that does not fit inside
the image (0x1400ac9c0); otherwise the four LBF stages always run. Refined
coordinates are written back only for points of FDP group 10.
The refinement is crates/tobii-headpose/src/eye_refine.rs.
"""
from ._arrays import raw
from ._native import eye_refine as _eye

MIRROR_YAW_DEGREES = 15.0
REFINED_GROUP = 10
REFINED_DEPTH = 1000.0
REFINED_QUALITY = -1.0
NEUTRAL_YAW_DEGREES = 180.0
RADIANS_TO_DEGREES = 57.2957763671875

# 0x140011900 for every FDP id (index 1..count per group); -1 means no mirror.
# Tabulated by executing the original function over the runtime's FDP count
# table at 0x14112f050; tools/eye_refine_oracle.py rechecks every entry.
MIRRORED_INDEX = _eye.MIRRORED_INDEX


def mirrored_index(group, index):
    if not 0 <= group < 1 << 32 or not 1 <= index < 1 << 32:
        raise ValueError('FDP id outside the runtime count table')
    return _eye.mirrored_index(group, index)


def tracker_yaw_degrees(rigid_state):
    """Yaw passed by 0x140049450: 180 - (ry - atan2(tx, tz)) in degrees, float32."""
    import numpy as np
    return np.float32(_eye.tracker_yaw_degrees(raw(rigid_state)))


def select_lbf_model(yaw_degrees):
    """Return (model name, mirrored). COMISS makes a NaN yaw select fa unmirrored."""
    return _eye.select_lbf_model(yaw_degrees)


def roi_inside(width, height, roi):
    """0x1400ac9c0 guard; roi is (x, y, width, height) or None for no ROI."""
    return _eye.roi_inside(width, height, None if roi is None else tuple(roi))


def lbf_refiner(models, pixels, stride=None):
    """Callable running the four LBF stages (0x1400acb80) on an 8-bit image.

The ROI, when present, offsets the pixel origin and replaces the dimensions
while the landmark coordinates stay in full-image pixels, as in the original.
"""
    import numpy as np
    image = np.ascontiguousarray(pixels, dtype=np.uint8)
    data, width = image.tobytes(), image.shape[1]

    def refine(name, xs, ys, scale, mirrored, roi):
        return _eye.refine_region(models[name], data, width, list(xs), list(ys), scale, mirrored,
                                  None if roi is None else tuple(roi))
    return refine


def refine_eye_landmarks(point_ids, records, width, height, scale, yaw_degrees, refine, roi=None):
    """Reproduce 0x14007d270 for the LBF point table point_ids (fa's ID order).

records maps FDP ids to normalized (x, y); every point must be present because
the original reads absent records' stale memory. Returns (valid, written)
where written maps group-10 ids to record fields (x, y, z, present, flag, quality).
"""
    ids = [tuple(identifier) for identifier in point_ids]
    name, mirrored, xs, ys = _eye.lbf_inputs(ids, dict(records), width, height, yaw_degrees)
    if not roi_inside(width, height, roi):
        return False, {}
    xs, ys = refine(name, xs, ys, scale, mirrored and name == 'fc', roi)
    written = _eye.refined_records(ids, [float(x) for x in xs], [float(y) for y in ys], width, height, mirrored)
    return True, {identifier: fields for identifier, fields in written}
