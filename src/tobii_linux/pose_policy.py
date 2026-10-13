"""Recovered pose validation policy, separate from public head-pose output."""
import math


def check_pose_bounds(rigid_state, shape_coefficients, animation_coefficients,
                      rotation_limits, translation_limits):
    """Return (invalid, cleaned_shape, cleaned_animation).

Original 0x1400dcc30 checks roll modulo the observed float32 pi constant,
without changing the stored roll. The x64 unordered lower-bound comparisons
reject NaNs on every axis. Nonfinite decoded
deformation coefficients are cleared and mark the result invalid.
This helper is not an input-sanitization boundary for an external API.
"""
    import numpy as np
    state = np.asarray(rigid_state, dtype=np.float32)
    rotation = np.asarray(rotation_limits, dtype=np.float32)
    translation = np.asarray(translation_limits, dtype=np.float32)
    shape = np.asarray(shape_coefficients, dtype=np.float32).copy()
    animation = np.asarray(animation_coefficients, dtype=np.float32).copy()
    if (state.shape != (6,) or rotation.shape != (3,2) or translation.shape != (3,2)
            or shape.ndim != 1 or animation.ndim != 1
            or not np.isfinite(rotation).all() or not np.isfinite(translation).all()
            or np.any(rotation[:,0] > rotation[:,1]) or np.any(translation[:,0] > translation[:,1])):
        raise ValueError('Invalid pose or bound dimensions')
    invalid = not bool(np.isfinite(state).all())
    checked = state.copy()
    checked[2] = np.float32(math.fmod(float(state[2]), 3.141592502593994)) if math.isfinite(float(state[2])) else np.float32('nan')
    limits = np.concatenate((rotation, translation))
    invalid |= bool(np.any(checked < limits[:,0]) or np.any(checked > limits[:,1]))
    for coefficients in (shape, animation):
        bad = ~np.isfinite(coefficients)
        invalid |= bool(np.any(bad))
        coefficients[bad] = 0
    return invalid, shape, animation


def fit_landmarks(model, normalized_points, width, height, state=None, iterations=3):
    """Fit complete D1 landmark records with the recovered bounded retry policy.

This component supports the no-external-pose-hint branch. All landmarks must be
present. Missing-record retention, view-dependent groups, temporal selection,
and conversion to public head-pose coordinates remain separate.
"""
    import numpy as np
    from .pose_fit import prepare_observations, initial_rigid_state
    from .pose_projection import bounded_coefficients
    from .pose_model import coefficient_latents
    if not isinstance(iterations, int) or not 0 <= iterations <= 100:
        raise ValueError('Expected a bounded nonnegative iteration count')
    points = np.asarray(normalized_points, dtype=np.float32)
    observations, sensitivity = prepare_observations(points, model.landmark_ids, width, height)
    indices = [model.landmark_ids.index(identifier) for identifier in ((3,7),(3,11),(3,8),(3,12))]
    def reset():
        return initial_rigid_state(model.vertices[indices], points[indices], width, height, model.focal_factor)
    count = 6 + len(model.shape_ranges) + len(model.animation_ranges)
    current = model.initial_state(reset()) if state is None else np.asarray(state, dtype=np.float32).copy()
    if current.shape != (count,) or not np.isfinite(current).all():
        raise ValueError('Expected a finite initialized fitting state')
    ns = len(model.shape_ranges)
    shape, current[6:6+ns] = bounded_coefficients(current[6:6+ns], model.shape_ranges)
    animation, current[6+ns:] = bounded_coefficients(current[6+ns:], model.animation_ranges)
    updates = 0
    for attempt in range(2):
        if attempt:
            # Original reset keeps decoded shape/animation coefficients, then
            # re-encodes them along with the new rigid pose.
            current = np.concatenate((reset(), coefficient_latents(shape, model.shape_ranges),
                                      coefficient_latents(animation, model.animation_ranges)))
        for _ in range(iterations if attempt == 0 else 3):
            current = model.step(observations, current, sensitivity)
            shape, current[6:6+ns] = bounded_coefficients(current[6:6+ns], model.shape_ranges)
            animation, current[6+ns:] = bounded_coefficients(current[6+ns:], model.animation_ranges)
            updates += 1
        invalid, shape, animation = check_pose_bounds(current[:6], shape, animation,
                                                     model.rotation_limits, model.translation_limits)
        if not invalid:
            break
    return {'state': current, 'shape_coefficients': shape, 'animation_coefficients': animation,
            'valid': not invalid, 'attempts': attempt + 1, 'updates': updates}


def fit_staged_landmarks(model, normalized_points, width, height):
    """No-hint, single-parent 3→3→5 caller branch on complete mapped landmarks.

Input points follow model.landmark_ids; selection is reordered to the original
numeric FDP scan. Confidence gating must happen before using the result for
tracking. Optional extra parent models and temporal caller state are absent.
"""
    import numpy as np
    from .pose_model import select_pose_landmarks
    from .pose_fit import initial_rigid_state, prepare_observations, mode_step
    points=np.asarray(normalized_points,dtype=np.float32)
    if points.shape!=(len(model.landmark_ids),2) or not np.isfinite(points).all():
        raise ValueError('Expected complete finite normalized landmarks')
    selected,order=select_pose_landmarks(model,model.landmark_ids)
    points=points[order]
    observations,weights=prepare_observations(points,selected.landmark_ids,width,height)
    indices=[selected.landmark_ids.index(i) for i in ((3,7),(3,11),(3,8),(3,12))]
    def reset(yaw=3.1415927410125732):
        return initial_rigid_state(selected.vertices[indices],points[indices],width,height,selected.focal_factor,yaw)
    # Caller reset 0x14007e270 stores integer 3; 0x14007e840 converts it to
    # float for initial yaw. The outer retry uses float32(pi) instead.
    pose=reset(3);shape=np.zeros(len(selected.shape_ranges),dtype=np.float32)
    animation=np.zeros(len(selected.animation_ranges),dtype=np.float32)
    stages=[]
    for mode,iterations in ((3,2),(5,1)):
        active=None;updates=0
        for attempt in range(2):
            if attempt:pose=reset();active=None
            for _ in range(iterations if attempt==0 else 3):
                result=mode_step(selected,observations,pose,shape,animation,mode,weights,active)
                pose,shape,animation,active=(result[k] for k in ('rigid_state','shape_coefficients','animation_coefficients','active_state'))
                updates+=1
            invalid,shape,animation=check_pose_bounds(pose,shape,animation,selected.rotation_limits,selected.translation_limits)
            if not invalid:break
        stages.append({'mode':mode,'updates':updates,'attempts':attempt+1,'valid':not invalid})
        if invalid:break
    return {'rigid_state':pose,'shape_coefficients':shape,'animation_coefficients':animation,
            'valid':not invalid,'stages':stages}
