"""Recovered VisionSDK::VNNTrackerCore frame loop (0x140049450) for grayscale frames.

States: 0 searching (full-frame detection), 1 recovering (re-detection inside
the working image), 2 tracking (box from the previous landmarks).
"""
from dataclasses import dataclass
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


class FaceTracker:
    """Detection, working-view D1 landmarks, confidence gate and temporal fit per frame.

The original then runs a yaw-dependent LBF eye refinement. It can only fail for
a source-image region outside the image, which this loop never sets, and the
installed models carry no group-10 points for it to write (docs/pose-temporal.md),
so it is omitted.
"""
    def __init__(self, model_directory, region=None):
        import numpy as np
        from .d1 import D1LandmarkModel
        from .face_assets import read_shapes
        from .face_detect import FaceFinder
        from .pose_model import load_pose_model
        from .pose_temporal import TemporalPoseFitter
        directory = Path(model_directory)
        _, reference = read_shapes(directory/'support/bdtsdata/NN/vnn_nir/init_shape.bin')
        self.landmarks = D1LandmarkModel(directory/'d1', np.asarray(reference).reshape(57, 2))
        self.pose_model = load_pose_model(directory/'support')
        self.finder = FaceFinder(directory/'ff')
        self.fitter = TemporalPoseFitter(self.pose_model)
        self.region = region
        self.roll = 0.0
        self.reset()

    def reset(self):
        self.state = SEARCHING
        self.face = (-1, -1, -1, -1)
        if hasattr(self, 'fitter'):
            self.fitter.reset()

    def _empty(self):
        return TrackedFrame(self.state, None, None, None, None, float('nan'), False, None, self.roll)

    def process(self, frame):
        import numpy as np
        from .d1_quality import d1_quality_score, passes_d1_quality
        pixels = np.asarray(frame, dtype=np.uint8)
        height, width = pixels.shape
        state = self.state
        if state == SEARCHING:
            found = self.finder.detect(pixels, region=self.region)
            if not found:
                self.reset()
                return self._empty()
            face = found[0].rectangle()
        else:
            face = self.face
        view = working_view(pixels, face)
        if state == RECOVERING:
            refreshed = redetect(view.image, view.face, self.finder)
            if refreshed is None:
                self.reset()
                return self._empty()
            view = WorkingView(view.image, view.region, view.ratio, refreshed)
        work_h, work_w = view.image.shape
        compensate = state == TRACKING
        landmarks = self.landmarks.predict_frame(view.image.tobytes(), work_w, work_h, view.face,
                                                 self.roll, compensate_roll=compensate)
        ids = self.pose_model.landmark_ids
        self.roll = temporal_roll(landmarks.normalized_points, ids, work_w, work_h)
        score = d1_quality_score(landmarks.quality, ids)
        accepted = passes_d1_quality(score, state)
        fit = None
        if accepted:
            fit = self.fitter.process(landmarks.normalized_points, work_w, work_h)
            if not fit['valid']:
                self.reset()
                return self._empty()
        frame_points = to_frame_points(landmarks.normalized_points, view, width, height)
        if accepted:
            self.state, self.face = TRACKING, landmark_box(frame_points, width, height)
        elif state == TRACKING:
            self.state, self.face = RECOVERING, working_face_in_frame(view)
        else:
            self.reset()
        return TrackedFrame(state, face, (work_w, work_h), landmarks.normalized_points, frame_points,
                            float(score), bool(accepted), fit, self.roll)
