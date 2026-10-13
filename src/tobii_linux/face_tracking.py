"""Recovered VisionSDK::VNNTrackerCore frame loop (0x140049450) for grayscale frames.

States: 0 searching (full-frame detection), 1 recovering (re-detection inside
the working image), 2 tracking (box from the previous landmarks).
"""
from dataclasses import dataclass, replace
import math
from pathlib import Path

SEARCHING, RECOVERING, TRACKING = 0, 1, 2
EXPANSION = 2.5
TARGET_FACE_WIDTH = 96
REDETECT_SENSITIVITY = 0.949999988079071
REDETECT_MIN_OVERLAP = 0.10000000149011612
RADIANS_TO_DEGREES = 57.2957763671875
ROLL_POINTS = ((4, 1), (4, 2), (9, 4), (9, 5))


def _f32(value):
    import numpy as np
    return np.float32(value)


def _floor(value):
    return int(math.floor(float(_f32(value))))


def _round(value):
    return int(round(float(_f32(value))))


@dataclass(frozen=True)
class WorkingView:
    """Expanded, clamped face region resized so the face is TARGET_FACE_WIDTH wide."""
    image: object
    region: tuple
    ratio: float
    face: tuple


def expanded_region(face, frame_width, frame_height, expansion=EXPANSION):
    """0x140079ce0 expansion around the box centre, clamped by 0x14007a740."""
    x, y, width, height = face
    grown_w = _floor(_f32(width) * _f32(expansion))
    grown_h = _floor(_f32(height) * _f32(expansion))
    left = _floor((_f32(width) * _f32(0.5) + _f32(x)) - _f32(grown_w) * _f32(0.5))
    top = _floor((_f32(height) * _f32(0.5) + _f32(y)) - _f32(grown_h) * _f32(0.5))
    clamped_x, clamped_y = max(left, 0), max(top, 0)
    return (clamped_x, clamped_y, min(left + grown_w, frame_width) - clamped_x,
            min(top + grown_h, frame_height) - clamped_y)


def working_view(frame, face, expansion=EXPANSION, target=TARGET_FACE_WIDTH):
    """Crop and resizing adjustments of 0x140079ce0 applied to one frame."""
    import numpy as np
    from .face_detect import resize_bilinear
    pixels = np.asarray(frame, dtype=np.uint8)
    region = expanded_region(face, pixels.shape[1], pixels.shape[0], expansion)
    rx, ry, rw, rh = region
    if rw <= 0 or rh <= 0:
        raise ValueError('Face region lies outside the frame')
    ratio = _f32(target) / _f32(face[2])
    shifted = (face[0] - rx, face[1] - ry, face[2], face[3])
    scaled_face = tuple(_round(_f32(v) * ratio) for v in shifted)
    size = (_round(_f32(rw) * ratio), _round(_f32(rh) * ratio))
    crop = pixels[ry:ry + rh, rx:rx + rw]
    if face[2] < target * 8:
        image = resize_bilinear(crop, *size)
    else:
        image = resize_bilinear(resize_bilinear(crop, size[0] * 2, size[1] * 2), *size)
    return WorkingView(image, region, float(ratio), scaled_face)


def working_face_in_frame(view):
    """Inverse adjustments 0x140079c00 then 0x1400799c0 for the working face box."""
    inverse = _f32(1.0) / _f32(view.ratio)
    x, y, width, height = (_round(_f32(v) * inverse) for v in view.face)
    return (x + view.region[0], y + view.region[1], width, height)


def to_frame_points(normalized_points, view, frame_width, frame_height):
    """0x14007aa30 mapping of working-normalised records into the full frame."""
    import numpy as np
    rx, ry, rw, rh = (_f32(v) for v in view.region)
    points = np.asarray(normalized_points, dtype=np.float32)
    mapped = np.empty_like(points)
    mapped[:, 0] = (rw * points[:, 0] - (_f32(0) - rx)) / _f32(frame_width)
    mapped[:, 1] = (rh * points[:, 1] - (_f32(0) - ry)) / _f32(frame_height)
    return mapped


def landmark_box(frame_points, frame_width, frame_height):
    """0x14007a4b0: square box around all landmarks for the next frame."""
    low_x = low_y = _f32(3.4028234663852886e+38)
    high_x = high_y = _f32(0)
    for x, y in frame_points:
        px, py = _f32(frame_width) * _f32(x), _f32(frame_height) * _f32(y)
        low_x, high_x = min(low_x, px), max(high_x, px)
        low_y, high_y = min(low_y, py), max(high_y, py)
    half = _f32(0.5)
    side = ((high_y - low_y) + (high_x - low_x)) * half
    return (_round(((high_x - low_x) - side) * half + low_x), _round(((high_y - low_y) - side) * half + low_y),
            _round(side), _round(side))


def temporal_roll(normalized_points, landmark_ids, width, height):
    """Roll in degrees stored by 0x14007dc50 for the next crop, from brow and nostril midpoints."""
    lookup = dict(zip(landmark_ids, normalized_points))
    brow_a, brow_b, nose_a, nose_b = (lookup[i] for i in ROLL_POINTS)
    half, w, h = _f32(0.5), _f32(width), _f32(height)
    dy = (_f32(nose_b[1]) + _f32(nose_a[1])) * h * half - (_f32(brow_b[1]) + _f32(brow_a[1])) * h * half
    dx = (_f32(nose_b[0]) + _f32(nose_a[0])) * w * half - (_f32(brow_b[0]) + _f32(brow_a[0])) * w * half
    return float(_f32(_f32(math.atan2(dy, dx)) * _f32(RADIANS_TO_DEGREES) - _f32(90.0)))


def overlap(box, face):
    """Inclusive-pixel overlap ratio used by 0x1400499d0."""
    one = _f32(1.0)
    x1, y1 = _f32(face[0]), _f32(face[1])
    x2, y2 = _f32(face[2] + face[0]), _f32(face[3] + face[1])
    wide = max((min(_f32(box.x2), x2) - max(_f32(box.x1), x1)) + one, _f32(0))
    tall = max((min(_f32(box.y2), y2) - max(_f32(box.y1), y1)) + one, _f32(0))
    shared = tall * wide
    area = ((_f32(box.y2) - _f32(box.y1)) + one) * ((_f32(box.x2) - _f32(box.x1)) + one)
    return shared / ((area + ((y2 - y1) + one) * ((x2 - x1) + one)) - shared)


def redetect(image, face, run_network):
    """0x1400499d0: best-overlapping detection in the working image, or None."""
    from .face_detect import detect_faces
    found = detect_faces(image, run_network, sensitivity=REDETECT_SENSITIVITY, min_face=0.0, max_face=1.0)
    best, best_overlap = None, _f32(0)
    for box in found:
        ratio = overlap(box, face)
        if ratio > best_overlap:
            best, best_overlap = box, ratio
    if best is None or not best_overlap > _f32(REDETECT_MIN_OVERLAP):
        return None
    return best.rectangle()


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

`timestamp_ms` should be the frame time in milliseconds; without it the
monotonic host clock stands in, as the original falls back to _ftime.
"""
    def __init__(self, model_directory, region=None, smoothing_factors=None):
        import numpy as np
        from .d1 import D1LandmarkModel
        from .face_assets import read_shapes
        from .face_detect import FaceFinder
        from .landmark_smoothing import LandmarkFilter, table_entries
        from .pose_model import load_pose_model, numeric_config
        from .pose_temporal import TemporalPoseFitter
        directory = Path(model_directory)
        _, reference = read_shapes(directory/'support/bdtsdata/NN/vnn_nir/init_shape.bin')
        self.landmarks = D1LandmarkModel(directory/'d1', np.asarray(reference).reshape(57, 2))
        self.pose_model = load_pose_model(directory/'support')
        self.finder = FaceFinder(directory/'ff')
        self.fitter = TemporalPoseFitter(self.pose_model)
        config = directory/'support/Head Tracker.cfg'
        if smoothing_factors is None:
            smoothing_factors = numeric_config(config, 'smoothing_factors', 7)
        self.smoothing_entries = table_entries(smoothing_factors)
        self.recovery_timeout_ms = int(numeric_config(config, 'recovery_timeout', 1)[0])
        self.landmark_filter = LandmarkFilter()
        self.region = region
        self.roll = 0.0
        self.reset()

    def reset(self):
        self._reset_core()
        self.fitter.reset()
        self.landmark_filter.reset()
        self.object_state = OBJECT_RESTARTED
        self.smoothing_epoch = 0
        self.lost_since = -1
        self.smoothed = None

    def _reset_core(self):
        """0x140049400 (vtable slot 2): back to searching with no box."""
        self.state = SEARCHING
        self.face = (-1, -1, -1, -1)

    def _failed_core(self):
        self._reset_core()
        return CoreFrame(TrackedFrame(self.state, None, None, None, None, float('nan'), False, None, self.roll),
                         False, None)

    def _track(self, pixels):
        """0x140049450 with the core fit disabled; records are frame-normalised with Y down."""
        from .d1_quality import d1_quality_score, passes_d1_quality
        height, width = pixels.shape
        state = self.state
        if state == SEARCHING:
            found = self.finder.detect(pixels, region=self.region)
            if not found:
                return self._failed_core()
            face = found[0].rectangle()
        else:
            face = self.face
        view = working_view(pixels, face)
        if state == RECOVERING:
            refreshed = redetect(view.image, view.face, self.finder)
            if refreshed is None:
                return self._failed_core()
            view = WorkingView(view.image, view.region, view.ratio, refreshed)
        work_h, work_w = view.image.shape
        landmarks = self.landmarks.predict_frame(view.image.tobytes(), work_w, work_h, view.face,
                                                 self.roll, compensate_roll=state == TRACKING)
        ids = self.pose_model.landmark_ids
        self.roll = temporal_roll(landmarks.normalized_points, ids, work_w, work_h)
        score = d1_quality_score(landmarks.quality, ids)
        accepted = passes_d1_quality(score, state)
        frame_points = to_frame_points(landmarks.normalized_points, view, width, height)
        if accepted:
            self.state, self.face = TRACKING, landmark_box(frame_points, width, height)
        elif state == TRACKING:
            self.state, self.face = RECOVERING, working_face_in_frame(view)
        else:
            return self._failed_core()
        tracked = TrackedFrame(state, face, (work_w, work_h), landmarks.normalized_points, frame_points,
                               float(score), bool(accepted), None, self.roll)
        return CoreFrame(tracked, True, frame_records(frame_points, landmarks.quality, ids))

    def _smooth(self, core, time_ms):
        """0x14004a530 from the core result up to the copy that 0x14004a2c0 fits."""
        from .landmark_smoothing import flip_y, smooth_records
        landmark_filter = self.landmark_filter
        landmark_filter.enabled = core.succeeded
        landmark_filter.begin_frame()
        if core.succeeded:
            records = copy_records(core.records)
            flip_y(records)
            if self.object_state != OBJECT_TRACKING:
                self.smoothing_epoch = time_ms
                landmark_filter.reset()
            smooth_records(landmark_filter, records, time_ms - self.smoothing_epoch, self.smoothing_entries)
            self.smoothed = records
        fitted = None
        if self.smoothed is not None:
            fitted = copy_records(self.smoothed)
            flip_y(fitted)
        if self.object_state == OBJECT_LOST:
            if self.lost_since == -1:
                self.lost_since = time_ms
            elif self.recovery_timeout_ms <= time_ms - self.lost_since:
                self.fitter.reset()
                self.lost_since = -1
                landmark_filter.reset()
                self.object_state = OBJECT_RESTARTED
        else:
            self.lost_since = -1
        return fitted

    def _fit(self, core, fitted, width, height):
        """0x14004a2c0: 0x14007e440 on the flipped copy at frame size; a failure resets core and fitter."""
        import numpy as np
        fit = None
        if core.succeeded:
            points = np.array([(fitted[tuple(i)].x, fitted[tuple(i)].y) for i in self.pose_model.landmark_ids],
                              dtype=np.float32)
            fit = self.fitter.process(points, width, height)
        if fit is None or not fit['valid']:
            self._reset_core()
            self.fitter.reset()
            return None
        return fit

    def process(self, frame, timestamp_ms=None):
        import time
        import numpy as np
        pixels = np.asarray(frame, dtype=np.uint8)
        height, width = pixels.shape
        time_ms = time.monotonic_ns() // 1_000_000 if timestamp_ms is None else int(timestamp_ms)
        core = self._track(pixels)
        fitted = self._smooth(core, time_ms)
        fit = self._fit(core, fitted, width, height)
        if fit is not None:
            self.object_state = OBJECT_TRACKING
        elif self.object_state == OBJECT_TRACKING:
            self.object_state = OBJECT_LOST
        points = None
        if fit is not None:
            points = np.array([(fitted[tuple(i)].x, fitted[tuple(i)].y) for i in self.pose_model.landmark_ids],
                              dtype=np.float32)
        return replace(core.tracked, fit=fit, fitted_points=points)
