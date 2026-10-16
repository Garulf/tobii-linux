"""Fitted face pose to the public 48-byte head-pose sample (producer 0x140293ca0 onwards).

The stages are crates/tobii-headpose/src/pipeline.rs.
"""
from dataclasses import dataclass
import struct

from ._native import head_pose_pipeline as _pipeline

SAMPLE = struct.Struct('<qi3fiii3f')


@dataclass(frozen=True)
class HeadPoseSample:
    """tobii_head_pose_t as filled by 0x140287120."""
    timestamp_us: int
    valid: bool
    position_mm: tuple
    rotation_rad: tuple

    def encode(self):
        return _pipeline.encode_sample(self.timestamp_us, bool(self.valid), tuple(self.position_mm),
                                       tuple(self.rotation_rad))


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
        import numpy as np
        area = display_area
        self._native = _pipeline.HeadPoseOutput(tuple(camera_translation), tuple(camera_angles),
                                                tuple(area.top_left), tuple(area.top_right), tuple(area.bottom_left))
        self.camera = np.array(self._native.camera, dtype=np.float32).reshape(4, 4)
        self.display = np.array(self._native.display, dtype=np.float32).reshape(4, 4)

    def __call__(self, fitted_rigid_state, timestamp_us):
        """`fitted_rigid_state` is (rx, ry, rz, tx, ty, tz) from the face-model fit, or None."""
        from ._arrays import raw
        state = None if fitted_rigid_state is None else raw(fitted_rigid_state)
        _, valid, position, rotation = self._native.process(state, timestamp_us)
        return HeadPoseSample(timestamp_us, valid, tuple(position), tuple(rotation))
