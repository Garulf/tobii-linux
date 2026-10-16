"""Prepare the installed fitting model; no camera capture or outer tracker policy.

The model loader and encoders are crates/tobii-headpose/src/model.rs.
"""
from dataclasses import dataclass

from ._arrays import array, floats, raw
from ._native import pose_model as _model


def numeric_config(path, key, count):
    """Read one observed numeric field, including multiline continuation rows."""
    return _model.numeric_config(path, key, count)


def coefficient_latents(coefficients, ranges):
    """Original float32 log-ratio encoder, restricted to interior coefficients."""
    coefficients, ranges = floats(coefficients), floats(ranges)
    if coefficients.ndim != 1 or ranges.shape != (len(coefficients), 2):
        raise ValueError('Invalid coefficient bounds')
    return array(_model.coefficient_latents(coefficients.tobytes(), ranges.tobytes()), len(coefficients))


@dataclass(frozen=True)
class PreparedPoseModel:
    landmark_ids: tuple
    vertices: object
    shape_basis: object
    animation_basis: object
    shape_ranges: object
    animation_ranges: object
    shape_mask: object
    animation_mask: object
    state_sensitivity: object
    rotation_limits: object
    translation_limits: object
    focal_factor: float

    def initial_state(self, rigid_state):
        rigid = floats(rigid_state)
        if rigid.shape != (6,):
            raise ValueError('Expected six finite rigid state values')
        count = 6 + len(self.shape_ranges) + len(self.animation_ranges)
        return array(_model.initial_state(_native_model(self), rigid.tobytes()), count)

    def step(self, observations, state, observation_sensitivity=None):
        from .pose_fit import deformation_step
        return deformation_step(self.vertices, observations, state, self.focal_factor, 1,
            self.shape_basis, self.shape_ranges, self.animation_basis, self.animation_ranges,
            self.state_sensitivity, observation_sensitivity, self.shape_mask, self.animation_mask)


def _native_model(model):
    """The fields of a PreparedPoseModel as the native fitting functions take them."""
    import numpy as np
    shape, animation = floats(model.shape_basis), floats(model.animation_basis)
    masks = [[bool(x) for x in np.asarray(mask, dtype=bool).reshape(-1)]
             for mask in (model.shape_mask, model.animation_mask)]
    return ([tuple(identifier) for identifier in model.landmark_ids],
            (raw(model.vertices), shape.tobytes(), shape.shape[2], animation.tobytes(), animation.shape[2]),
            (raw(model.shape_ranges), raw(model.animation_ranges), *masks),
            (raw(model.state_sensitivity), raw(model.rotation_limits), raw(model.translation_limits),
             float(model.focal_factor)))


def load_pose_model(support_directory):
    """Load the observed jk_300/D1 configuration and select its 57 mapped vertices.

Keeps all 40 shape and 23 animation states, zeroing disabled basis columns as
observed in the runtime loader. This does not select a live tracker mode.
"""
    import numpy as np
    ids, (vertices, shape, ns, animation, na), (shape_ranges, animation_ranges, shape_mask, animation_mask), \
        (sensitivity, rotation, translation, focal) = _model.load_pose_model(support_directory)
    points = len(ids)
    return PreparedPoseModel(tuple(ids), array(vertices, points, 3), array(shape, points, 3, ns),
        array(animation, points, 3, na), array(shape_ranges, ns, 2), array(animation_ranges, na, 2),
        np.array(shape_mask, dtype=bool), np.array(animation_mask, dtype=bool), array(sensitivity, 6 + ns + na),
        array(rotation, 3, 2), array(translation, 3, 2), focal)


def select_pose_landmarks(model, present_ids):
    """Select present mapped D1 landmarks in original numeric FDP scan order.

Returns the reduced/reordered model and indices into the input model's point
array. Aliases absent from the rigid FDP map are ignored. Presence is supplied
by the caller; this does not guess validity from a neural quality score.
"""
    import numpy as np
    from dataclasses import replace
    selected, indices = _model.select_pose_landmarks(_native_model(model), [tuple(x) for x in present_ids])
    indices = np.asarray(indices, dtype=np.intp)
    return replace(model, landmark_ids=tuple(selected), vertices=model.vertices[indices],
                   shape_basis=model.shape_basis[indices], animation_basis=model.animation_basis[indices]), indices
