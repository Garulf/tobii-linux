"""Fitted face pose to the public 48-byte head-pose sample (producer 0x140293ca0 onwards)."""
from dataclasses import dataclass
import struct
from .pose_output import PI, HeadPoseFilter, adapter_head_pose, matrix_euler
from .pose_geometry import camera_extrinsic, display_matrix, geometry_stage

SAMPLE = struct.Struct('<qi3fiii3f')


@dataclass(frozen=True)
class HeadPoseSample:
    """tobii_head_pose_t as filled by 0x140287120."""
    timestamp_us: int
    valid: bool
    position_mm: tuple
    rotation_rad: tuple

    def encode(self):
        flag = 1 if self.valid else 0
        position = self.position_mm if self.valid else (0.0, 0.0, 0.0)
        rotation = self.rotation_rad if self.valid else (0.0, 0.0, 0.0)
        return SAMPLE.pack(self.timestamp_us, flag, *position, flag, flag, flag, *rotation)


@dataclass(frozen=True)
class DisplayArea:
    """Stream Engine display area in millimetres, in its top-left, top-right, bottom-left order."""
    top_left: tuple
    top_right: tuple
    bottom_left: tuple


class HeadPoseOutput:
    """Adapter slot 5, matrix-to-angles, geometry stage and One Euro filter stage.

`camera_translation` and `camera_angles` are the tracker's camera extrinsic from
its hardware info. Without a display area the original produces no head pose.
"""
    def __init__(self, camera_translation, camera_angles, display_area):
        self.camera = camera_extrinsic(camera_translation, camera_angles)
        area = display_area
        self.display = display_matrix(area.top_left, area.bottom_left, area.top_right)
        self.filter = HeadPoseFilter()

    def __call__(self, fitted_rigid_state, timestamp_us):
        """`fitted_rigid_state` is (rx, ry, rz, tx, ty, tz) from the face-model fit, or None."""
        import numpy as np
        if fitted_rigid_state is None:
            return HeadPoseSample(timestamp_us, False, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
        state = np.asarray(fitted_rigid_state, dtype=np.float32)
        rotation = (state[0], np.float32(state[1] - np.float32(PI)), state[2])
        matrix, position = adapter_head_pose(rotation, state[3:])
        angles = matrix_euler(matrix)
        rotated, moved = geometry_stage(angles[:3], position, self.camera, self.display)
        smoothed_rotation, smoothed_position = self.filter(np.asarray(rotated, np.float32)[:3], moved, timestamp_us)
        return HeadPoseSample(timestamp_us, True, tuple(float(v) for v in smoothed_position),
                              tuple(float(v) for v in smoothed_rotation))
