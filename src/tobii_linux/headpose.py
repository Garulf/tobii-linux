"""Native supplied-face-box head-pose component; no USB or public pose bridge.

The component is crates/tobii-headpose/src/d1_pose.rs.
"""
from dataclasses import dataclass
from pathlib import Path

from ._native import headpose as _headpose
from .d1 import landmarks_of
from .pose_model import load_pose_model
from .pose_policy import staged_fit


@dataclass(frozen=True)
class FramePoseResult:
    landmarks: object
    quality_score: float
    quality_accepted: bool
    fitting_result: object


class NativeD1Pose:
    """Connect D1 and the reconstructed no-hint single-parent fitting branch.

Caller supplies grayscale pixels, a face box, quality mode and prior crop roll.
Returned coordinates are internal fitting coordinates, not the final Tobii API.
Face detection, temporal mode selection and runtime hardware calibration are
not implemented by this component.
"""
    def __init__(self, model_directory):
        directory = Path(model_directory)
        self._native = _headpose.NativeD1Pose(directory)
        self.pose_model = load_pose_model(directory/'support')

    def predict_frame(self, pixels, width, height, face_box, caller_mode=0, roll_degrees=0):
        if caller_mode not in (0, 1, 2):
            raise ValueError('Expected an observed caller quality mode (0, 1 or 2)')
        landmarks, score, accepted, fitted = self._native.predict_frame(pixels, width, height, tuple(face_box),
                                                                         int(caller_mode), roll_degrees)
        return FramePoseResult(landmarks_of(landmarks), score, accepted,
                               None if fitted is None else staged_fit(fitted, self.pose_model))
