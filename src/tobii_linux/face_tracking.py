"""Recovered VisionSDK::VNNTrackerCore frame loop (0x140049450) for grayscale frames.

States: 0 searching (full-frame detection), 1 recovering (re-detection inside
the working image), 2 tracking (box from the previous landmarks).
The geometry and the tracker are crates/tobii-headpose/src/tracking.rs; images
are NumPy uint8 arrays.
"""
from dataclasses import dataclass
from pathlib import Path

from ._arrays import array, floats
from ._native import face_tracking as _tracking

SEARCHING, RECOVERING, TRACKING = 0, 1, 2
EXPANSION = 2.5
TARGET_FACE_WIDTH = 96
REDETECT_SENSITIVITY = 0.949999988079071
REDETECT_MIN_OVERLAP = 0.10000000149011612
RADIANS_TO_DEGREES = 57.2957763671875
ROLL_POINTS = ((4, 1), (4, 2), (9, 4), (9, 5))


def _image(frame):
    import numpy as np
    return np.asarray(frame, dtype=np.uint8)


@dataclass(frozen=True)
class WorkingView:
    """Expanded, clamped face region resized so the face is TARGET_FACE_WIDTH wide."""
    image: object
    region: tuple
    ratio: float
    face: tuple


def expanded_region(face, frame_width, frame_height, expansion=EXPANSION):
    """0x140079ce0 expansion around the box centre, clamped by 0x14007a740."""
    return tuple(_tracking.expanded_region(tuple(face), frame_width, frame_height, expansion))


def working_view(frame, face, expansion=EXPANSION, target=TARGET_FACE_WIDTH):
    """Crop and resizing adjustments of 0x140079ce0 applied to one frame."""
    import numpy as np
    pixels = _image(frame)
    data, shape, region, ratio, scaled_face = _tracking.working_view(pixels.tobytes(), pixels.shape, tuple(face),
                                                                     expansion, target)
    image = np.frombuffer(data, dtype=np.uint8).reshape(shape).copy()
    return WorkingView(image, tuple(region), ratio, tuple(scaled_face))


def working_face_in_frame(view):
    """Inverse adjustments 0x140079c00 then 0x1400799c0 for the working face box."""
    return tuple(_tracking.working_face_in_frame(tuple(view.face), tuple(view.region), view.ratio))


def to_frame_points(normalized_points, view, frame_width, frame_height):
    """0x14007aa30 mapping of working-normalised records into the full frame."""
    points = floats(normalized_points)
    mapped = _tracking.to_frame_points(points.reshape(-1, 2).tobytes(), tuple(view.region), frame_width, frame_height)
    return array(mapped, *points.shape)


def landmark_box(frame_points, frame_width, frame_height):
    """0x14007a4b0: square box around all landmarks for the next frame."""
    points = floats([(x, y) for x, y in frame_points]).reshape(-1, 2)
    return tuple(_tracking.landmark_box(points.tobytes(), frame_width, frame_height))


def temporal_roll(normalized_points, landmark_ids, width, height):
    """Roll in degrees stored by 0x14007dc50 for the next crop, from brow and nostril midpoints."""
    lookup = dict(zip(landmark_ids, normalized_points))
    points = [tuple(float(v) for v in lookup[i]) for i in ROLL_POINTS]
    return _tracking.roll_degrees(points, width, height)


def overlap(box, face):
    """Inclusive-pixel overlap ratio used by 0x1400499d0."""
    import numpy as np
    return np.float32(_tracking.overlap((box.x1, box.y1, box.x2, box.y2), tuple(face)))


def redetect(image, face, run_network):
    """0x1400499d0: best-overlapping detection in the working image, or None.

`run_network` maps the letterboxed image to raw face finder outputs, as for
face_detect.detect_faces; FaceTracker runs the native face finder instead.
"""
    from .face_detect import detect_faces
    found = detect_faces(image, run_network, sensitivity=REDETECT_SENSITIVITY, min_face=0.0, max_face=1.0)
    best = _tracking.best_redetection([(box.x1, box.y1, box.x2, box.y2) for box in found], tuple(face))
    return None if best is None else tuple(best)


ALIASED_RECORDS = (((2, 1), (15, 17)), ((13, 17), (15, 17)), ((14, 21), (9, 3)), ((9, 12), (14, 23)))
OBJECT_TRACKING, OBJECT_LOST, OBJECT_RESTARTED = 1, 2, 3


def frame_records(frame_points, quality, landmark_ids):
    """Core records after 0x140049450: frame-normalised D1 points plus the 0x140055a20 aliases."""
    from .landmark_smoothing import LandmarkRecord
    records = {tuple(identifier): LandmarkRecord(float(x), float(y), True, float(q))
               for identifier, (x, y), q in zip(landmark_ids, frame_points, quality)}
    for alias, source in ALIASED_RECORDS:
        original = records[source]
        records[alias] = LandmarkRecord(original.x, original.y, True, original.quality)
    return records


def copy_records(records):
    """0x140010560 copy of the fields the smoothing and the fit use."""
    from .landmark_smoothing import LandmarkRecord
    return {key: LandmarkRecord(r.x, r.y, r.present, r.quality) for key, r in records.items()}


@dataclass(frozen=True)
class TrackedFrame:
    state: int
    face: object
    working_size: object
    working_points: object
    frame_points: object
    quality_score: float
    accepted: bool
    fit: object
    roll_degrees: float
    fitted_points: object = None


@dataclass(frozen=True)
class CoreFrame:
    """What 0x140049450 hands back: success flag and the frame-normalised records."""
    tracked: object
    succeeded: bool
    records: object


class FaceTracker:
    """VNNTrackerCore frame loop followed by the per-object step 0x14004a530.

The core (0x140049450) finds the face, runs D1 in the working view and applies
the confidence gate. Its own fit is disabled in the shipped configuration:
0x140049e60 clears the core flag +0x15 because jk_300_rigid.fdp maps no FDP
group 10 point, which also skips the LBF eye refinement. The object step then
smooths the frame-normalised records (landmark_smoothing), copies them, flips Y
back and fits them with 0x14007e440 at full-frame dimensions (0x14004a2c0).
The whole frame runs in the Rust core, on its OpenVINO face finder and D1.

`timestamp_ms` should be the frame time in milliseconds; without it the
monotonic host clock stands in, as the original falls back to _ftime.
"""
    def __init__(self, model_directory, region=None, smoothing_factors=None):
        from .pose_model import load_pose_model
        directory = Path(model_directory)
        factors = None if smoothing_factors is None else [float(f) for f in smoothing_factors]
        self._native = _tracking.FaceTracker(directory, None, factors)
        self.pose_model = load_pose_model(directory/'support')
        self.region = region

    @property
    def region(self):
        return self._region

    @region.setter
    def region(self, region):
        self._native.region = None if region is None else tuple(region)
        self._region = region

    @property
    def state(self):
        return self._native.state

    @property
    def face(self):
        return tuple(self._native.face)

    @property
    def roll(self):
        return self._native.roll

    @property
    def object_state(self):
        return self._native.object_state

    @property
    def smoothing_epoch(self):
        return self._native.smoothing_epoch

    @property
    def lost_since(self):
        return self._native.lost_since

    @property
    def recovery_timeout_ms(self):
        return self._native.recovery_timeout_ms

    @property
    def smoothing_entries(self):
        return tuple(tuple(entry) for entry in self._native.smoothing_entries)

    def reset(self):
        self._native.reset()

    def process(self, frame, timestamp_ms=None):
        import time
        from .pose_temporal import fit_arrays
        pixels = _image(frame)
        time_ms = time.monotonic_ns() // 1_000_000 if timestamp_ms is None else int(timestamp_ms)
        state, face, working_size, working_points, frame_points, score, accepted, fit, roll, fitted = \
            self._native.process(pixels.tobytes(), pixels.shape, time_ms)
        return TrackedFrame(
            state, None if face is None else tuple(face), None if working_size is None else tuple(working_size),
            None if working_points is None else tuple(working_points),
            None if frame_points is None else array(frame_points, -1, 2), score, accepted,
            None if fit is None else fit_arrays(fit, self.pose_model), roll,
            None if fitted is None else array(fitted, -1, 2))
