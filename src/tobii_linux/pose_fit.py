"""Recovered rigid fitting step, separate from the complete tracking policy.

The arithmetic is crates/tobii-headpose/src/fit.rs.
"""
from ._arrays import array, floats, raw
from ._native import pose_fit as _fit


def _sensitivity(values):
    return None if values is None else raw(values)


def _observations(observations, count, message):
    observed = floats(observations)
    if observed.shape != (count, 2):
        raise ValueError(message)
    return observed


def rigid_step(vertices, observations, state, focal_factor, model_scale=1.0,
               state_sensitivity=None, observation_sensitivity=None):
    """One regularized weighted update for six pose states.

The original update uses inverse sensitivity diagonals and an explicitly
inverted normal matrix. This finite positive-sensitivity implementation covers the observed point exclusion threshold but does
not cover near-zero sensitivity fallbacks or deformation states.
"""
    message = 'Expected six finite state values and matching Nx2 observations'
    state, vertices = floats(state), floats(vertices)
    if state.shape != (6,) or vertices.ndim != 2 or vertices.shape[1] != 3:
        raise ValueError(message)
    observed = _observations(observations, len(vertices), message)
    return array(_fit.rigid_step(vertices.tobytes(), observed.tobytes(), state.tobytes(), float(focal_factor),
                                 float(model_scale), _sensitivity(state_sensitivity),
                                 _sensitivity(observation_sensitivity)), 6)


def prepare_observations(normalized_points, landmark_ids, width, height):
    """Convert valid FDP landmarks to the fitting plane and observed weights.

Supports D1 landmark groups. Group 10/13 view-dependent weighting and missing
landmark retention require additional tracker state and are not implemented.
"""
    points = floats(normalized_points)
    ids = tuple(tuple(x) for x in landmark_ids)
    if points.shape != (len(ids), 2) or any(len(x) != 2 for x in ids):
        raise ValueError('Expected finite normalized XY landmarks and image dimensions')
    observed, sensitivity = _fit.prepare_observations(points.tobytes(), ids, width, height)
    return array(observed, len(ids), 2), array(sensitivity, 2 * len(ids))


def initial_rigid_state(model_points, image_points, width, height, focal_factor, neutral_yaw=3.1415927410125732):
    """Recover reset pose from four corresponding group-3 landmarks.

Both arrays are ordered IDs (3,7), (3,11), (3,8), (3,12). Model points are
Nx3 in the caller's already-prepared mesh space; image points are normalized
XY. This does not assume raw file mesh coordinates equal runtime model space.
"""
    model, image = floats(model_points), floats(image_points)
    if model.shape != (4, 3) or image.shape != (4, 2):
        raise ValueError('Expected four finite model/image correspondences and positive camera geometry')
    return array(_fit.initial_rigid_state(model.tobytes(), image.tobytes(), width, height, float(focal_factor),
                                          float(neutral_yaw)), 6)


def _block(basis, ranges, mask, vertices):
    import numpy as np
    basis, ranges = floats(basis), floats(ranges)
    if basis.ndim != 3 or basis.shape[:2] != vertices.shape or ranges.shape != (basis.shape[2], 2):
        raise ValueError('Invalid full fitting state or observations')
    enabled = None if mask is None else [bool(x) for x in np.asarray(mask, dtype=bool).reshape(-1)]
    return basis.tobytes(), basis.shape[2], ranges.tobytes(), enabled


def deformation_step(vertices, observations, state, focal_factor, model_scale,
                     shape_basis, shape_ranges, animation_basis, animation_ranges,
                     state_sensitivity=None, observation_sensitivity=None,
                     shape_mask=None, animation_mask=None):
    """One original-style update including bounded shape and animation states.

Latents are clamped before constructing the model, as in the original update.
The returned step may exceed that bound until the caller's subsequent decode.
Outer retry/validation policy is not supplied by this component.
"""
    message = 'Invalid full fitting state or observations'
    vertices, state = floats(vertices), floats(state)
    if vertices.ndim != 2 or vertices.shape[1] != 3 or state.ndim != 1:
        raise ValueError(message)
    observed = _observations(observations, len(vertices), message)
    updated = _fit.deformation_step(
        vertices.tobytes(), observed.tobytes(), state.tobytes(), float(focal_factor), float(model_scale),
        _block(shape_basis, shape_ranges, shape_mask, vertices),
        _block(animation_basis, animation_ranges, animation_mask, vertices),
        _sensitivity(state_sensitivity), _sensitivity(observation_sensitivity))
    return array(updated, len(state))


def mode_step(model, observations, rigid_state, shape_coefficients, animation_coefficients,
              fitting_mode, observation_sensitivity=None, active_state=None):
    """One flag-3 (pose/shape) or flag-5 (pose/animation) update.

Inactive deformation coefficients stay in physical coefficient space, exactly
as stored by the original model. The returned latents belong only to the active
state block and are clipped by the subsequent decode, as in the outer loop.
Pass active_state back for consecutive updates in the same mode; re-encoding
physical coefficients between those updates can introduce additional rounding.
"""
    from .pose_model import _native_model
    if fitting_mode not in (3, 5):
        raise ValueError('Expected fitting flag 3 or 5')
    message = 'Invalid mode-specific state or observations'
    pose, shape, animation = floats(rigid_state), floats(shape_coefficients), floats(animation_coefficients)
    if pose.shape != (6,) or shape.ndim != 1 or animation.ndim != 1:
        raise ValueError(message)
    observed = _observations(observations, len(model.vertices), message)
    retained = None
    if active_state is not None:
        retained = floats(active_state)
        if retained.ndim != 1:
            raise ValueError('Invalid retained active fitting state')
        retained = retained.tobytes()
    result = _fit.mode_step(_native_model(model), observed.tobytes(), pose.tobytes(), shape.tobytes(),
                            animation.tobytes(), fitting_mode, _sensitivity(observation_sensitivity), retained)
    active = 6 + (len(shape) if fitting_mode == 3 else len(animation))
    return {'rigid_state': array(result['rigid_state'], 6),
            'shape_coefficients': array(result['shape_coefficients'], len(shape)),
            'animation_coefficients': array(result['animation_coefficients'], len(animation)),
            'active_state': array(result['active_state'], active)}
