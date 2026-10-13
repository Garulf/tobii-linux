"""Native supplied-face-box head-pose component; no USB or public pose bridge."""
from dataclasses import dataclass
from pathlib import Path
from .d1 import D1LandmarkModel
from .d1_quality import d1_quality_score, passes_d1_quality
from .face_assets import read_shapes
from .pose_model import load_pose_model
from .pose_policy import fit_staged_landmarks


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
        import numpy as np
        directory=Path(model_directory)
        support=directory/'support'
        _,reference=read_shapes(support/'bdtsdata/NN/vnn_nir/init_shape.bin')
        self.landmarks=D1LandmarkModel(directory/'d1',np.asarray(reference).reshape(57,2))
        self.pose_model=load_pose_model(support)

    def predict_frame(self,pixels,width,height,face_box,caller_mode=0,roll_degrees=0):
        if caller_mode not in (0,1,2):
            raise ValueError('Expected an observed caller quality mode (0, 1 or 2)')
        landmarks=self.landmarks.predict_frame(pixels,width,height,face_box,roll_degrees)
        score=d1_quality_score(landmarks.quality,self.pose_model.landmark_ids)
        accepted=passes_d1_quality(score,caller_mode)
        fitted=None
        if accepted:
            fitted=fit_staged_landmarks(self.pose_model,landmarks.normalized_points,width,height)
        return FramePoseResult(landmarks,score,accepted,fitted)
