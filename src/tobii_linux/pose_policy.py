"""Recovered pose validation policy, separate from public head-pose output.

The policies are crates/tobii-headpose/src/policy.rs.
"""
from ._arrays import array, floats
from ._native import pose_policy as _policy


def check_pose_bounds(rigid_state, shape_coefficients, animation_coefficients,
                      rotation_limits, translation_limits):
    """Return (invalid, cleaned_shape, cleaned_animation).

Original 0x1400dcc30 checks roll modulo the observed float32 pi constant,
without changing the stored roll. The x64 unordered lower-bound comparisons
reject NaNs on every axis. Nonfinite decoded
deformation coefficients are cleared and mark the result invalid.
This helper is not an input-sanitization boundary for an external API.
"""
    state, rotation, translation = floats(rigid_state), floats(rotation_limits), floats(translation_limits)
    shape, animation = floats(shape_coefficients), floats(animation_coefficients)
    if (state.shape != (6,) or rotation.shape != (3, 2) or translation.shape != (3, 2)
            or shape.ndim != 1 or animation.ndim != 1):
        raise ValueError('Invalid pose or bound dimensions')
    invalid, shape_bytes, animation_bytes = _policy.check_pose_bounds(
        state.tobytes(), shape.tobytes(), animation.tobytes(), rotation.tobytes(), translation.tobytes())
    return invalid, array(shape_bytes, len(shape)), array(animation_bytes, len(animation))


def _points(model, normalized_points, message):
    points = floats(normalized_points)
    if points.shape != (len(model.landmark_ids), 2):
        raise ValueError(message)
    return points


def fit_landmarks(model, normalized_points, width, height, state=None, iterations=3):
    """Fit complete D1 landmark records with the recovered bounded retry policy.

This component supports the no-external-pose-hint branch. All landmarks must be
present. Missing-record retention, view-dependent groups, temporal selection,
and conversion to public head-pose coordinates remain separate.
"""
    from .pose_model import _native_model
    if not isinstance(iterations, int) or not 0 <= iterations <= 100:
        raise ValueError('Expected a bounded nonnegative iteration count')
    points = _points(model, normalized_points, 'Expected finite normalized XY landmarks and image dimensions')
    initial = None
    if state is not None:
        initial = floats(state)
        if initial.ndim != 1:
            raise ValueError('Expected a finite initialized fitting state')
        initial = initial.tobytes()
    current, shape, animation, valid, attempts, updates = _policy.fit_landmarks(
        _native_model(model), points.tobytes(), width, height, initial, iterations)
    ns, na = len(model.shape_ranges), len(model.animation_ranges)
    return {'state': array(current, 6 + ns + na), 'shape_coefficients': array(shape, ns),
            'animation_coefficients': array(animation, na), 'valid': valid, 'attempts': attempts, 'updates': updates}


def fit_staged_landmarks(model, normalized_points, width, height):
    """No-hint, single-parent 3→3→5 caller branch on complete mapped landmarks.

Input points follow model.landmark_ids; selection is reordered to the original
numeric FDP scan. Confidence gating must happen before using the result for
tracking. Optional extra parent models and temporal caller state are absent.
"""
    from .pose_model import _native_model
    points = _points(model, normalized_points, 'Expected complete finite normalized landmarks')
    pose, shape, animation, valid, stages = _policy.fit_staged_landmarks(
        _native_model(model), points.tobytes(), width, height)
    return {'rigid_state': array(pose, 6), 'shape_coefficients': array(shape, len(model.shape_ranges)),
            'animation_coefficients': array(animation, len(model.animation_ranges)), 'valid': valid,
            'stages': [{'mode': mode, 'updates': updates, 'attempts': attempts, 'valid': ok}
                       for mode, updates, attempts, ok in stages]}
