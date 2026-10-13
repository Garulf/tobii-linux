"""Recovered temporal fitting caller 0x14007e440 for the single-parent model.

VNNTrackerCore keeps one parent face model across frames. The first frame (or
the first after tracking is lost) runs the flag-3 initial fit 0x14007ec70 and
then the flag-5 temporal fit 0x14007ee60; later frames run only the temporal
fit, starting from the previous frame's pose and coefficients.
"""
import math

FLOAT_PI = 3.1415927410125732
INITIAL_YAW = 3
FACING_PI = 3.141590118408203
RADIANS_TO_DEGREES = 57.2957763671875
FACING_REINITIALISE_DEGREES = 30.0
REPROJECTION_ERROR_LIMIT = 0.10000000149011612
FRONTAL_LIMIT = 0.15707963705062866
RECTANGLE_SCALE = 1.7999999523162842
RECTANGLE_EYE_WEIGHT = 0.20000000298023224
RECTANGLE_NOSE_WEIGHT = 0.800000011920929
ERROR_EXCLUDED_GROUPS = (13, 15)
RESET_POINTS = ((3, 7), (3, 11), (3, 8), (3, 12))
MINIMUM_FIT_POINTS = 4


def _f32(value):
    import numpy as np
    return np.float32(value)


def _round(value):
    """cvtps2pd + cvtsd2si under the default round-to-nearest-even mode."""
    return int(round(float(_f32(value))))


def facing_degrees(rigid_state):
    """0x14007ee60 facing angle from the model pose, before that attempt's fit."""
    import numpy as np
    pose = np.asarray(rigid_state, dtype=np.float32)
    bearing = _f32(math.atan2(float(pose[3]), float(pose[5])))
    return _f32(_f32(_f32(pose[1] - bearing) - _f32(FACING_PI)) * _f32(RADIANS_TO_DEGREES))


def pose_output(rigid_state):
    """0x140055470 output: (rx, ry - float32(pi), rz) and the translation."""
    import numpy as np
    pose = np.asarray(rigid_state, dtype=np.float32)
    rotation = np.array([pose[0], pose[1] - _f32(FLOAT_PI), pose[2]], dtype=np.float32)
    return rotation, pose[3:].copy()


def is_near_frontal(output_rotation):
    """COMISS + JAE: a NaN angle counts as below the limit."""
    limit = _f32(FRONTAL_LIMIT)
    return not (abs(_f32(output_rotation[0])) >= limit or abs(_f32(output_rotation[1])) >= limit)


def eye_face_rectangle(normalized_points, landmark_ids, width, height):
    """0x14007a090 square from eye centres, nose (9.4/9.5) and FDP 8.1.

Returns (x, y, size, size) in pixels, or None when a required record is absent;
the original then leaves its output uninitialised.
"""
    lookup = {tuple(identifier): point for identifier, point in zip(landmark_ids, normalized_points)}
    required = ((3, 11), (3, 7), (3, 12), (3, 8), (9, 5), (9, 4), (8, 1))
    if any(identifier not in lookup for identifier in required):
        return None
    w, h = _f32(width), _f32(height)
    def pixel(identifier):
        x, y = lookup[identifier]
        return _round(w * _f32(x)), _round(_f32(y) * h)
    def middle(a, b):
        return (_round(_f32(a[0] + b[0]) * _f32(.5)), _round(_f32(a[1] + b[1]) * _f32(.5)))
    left = middle(pixel((3, 11)), pixel((3, 7)))
    right = middle(pixel((3, 12)), pixel((3, 8)))
    nose = middle(pixel((9, 5)), pixel((9, 4)))
    eyes = middle(right, left)
    mouth = pixel((8, 1))
    def squared(a, b):
        dx, dy = _f32(a[0] - b[0]), _f32(a[1] - b[1])
        return _f32(dy * dy + dx * dx)
    def root(value):
        return _f32(math.sqrt(float(value)))
    right_mouth, left_right, left_mouth = squared(right, mouth), squared(left, right), squared(left, mouth)
    largest = right_mouth if root(right_mouth) >= root(left_right) else left_right
    if not root(left_mouth) >= root(largest):
        left_mouth = left_right if root(right_mouth) < root(left_right) else right_mouth
    size = _round(root(left_mouth) * _f32(RECTANGLE_SCALE))
    half = _f32(_f32(size) * _f32(.5))
    x = _round(_f32(_f32(_f32(eyes[0]) * _f32(RECTANGLE_EYE_WEIGHT)) + _f32(_f32(nose[0]) * _f32(RECTANGLE_NOSE_WEIGHT))) - half)
    y = _round(_f32(_f32(_f32(eyes[1]) * _f32(RECTANGLE_EYE_WEIGHT)) + _f32(_f32(nose[1]) * _f32(RECTANGLE_NOSE_WEIGHT))) - half)
    return x, y, size, size


def finalized_projection(model, rigid_state, shape_coefficients, animation_coefficients):
    """Mesh finalization 0x140052710 projection (+0xb8) with zero eye offsets.

The divide is focal/Z followed by X*(focal/Z), unlike the fitting prediction.
"""
    import numpy as np
    from .pose_projection import deform_vertices, project_points
    pose = np.asarray(rigid_state, dtype=np.float32)
    deformed = deform_vertices(model.vertices, model.shape_basis, shape_coefficients,
                               model.animation_basis, animation_coefficients)
    _, camera = project_points(deformed, pose[:3], pose[3:], model.focal_factor, 1)
    factor = np.float32(model.focal_factor) / camera[:, 2]
    return np.column_stack((camera[:, 0] * factor, camera[:, 1] * factor))


def reprojection_error(model, projected, flipped_points, width, height, rectangle_width):
    """0x1400551c0 mean pixel distance between records and model, over rectangle width.

flipped_points are the Y-flipped caller copy (0x14007ac80) in model.landmark_ids
order. Groups 13 and 15 are skipped; summation follows numeric FDP order.
"""
    import numpy as np
    w, h = _f32(width), _f32(height)
    aspect_x = w / h if height < width else _f32(1)
    aspect_y = h / w if width < height else _f32(1)
    half = _f32(.5)
    inverse = _f32(1) / _f32(rectangle_width)
    total, count = _f32(0), 0
    for index in sorted(range(len(model.landmark_ids)), key=lambda i: tuple(model.landmark_ids[i])):
        if model.landmark_ids[index][0] in ERROR_EXCLUDED_GROUPS:
            continue
        model_x = half - (projected[index, 0] * half) / aspect_x
        model_y = (projected[index, 1] * half) / aspect_y + half
        record_x, record_y = (_f32(v) for v in flipped_points[index])
        dy = _f32(h * record_y) - _f32(h * model_y)
        dx = _f32(w * record_x) - _f32(w * model_x)
        total = _f32(total + _f32(np.sqrt(_f32(_f32(dx * dx) + _f32(dy * dy)))) * inverse)
        count += 1
    with np.errstate(invalid='ignore', divide='ignore'):
        return _f32(total / _f32(count))


class TemporalPoseFitter:
    """Stateful 0x14007e440 caller (last argument 1) for complete D1 landmarks.

Persistent state mirrors caller +0x59 (initialised), +0x39 (frontal shape refit
done), the parent model's pose and physical coefficients, and the fitting
object built by 0x1400dbba0. That object keeps its own latent state and is
re-encoded from the parent only when the requested flag changes, so parent
pose resets made between two same-flag fits do not move the next fit's start.
Extra parent models and the caller +0x58 external pose hint are not supported.
"""
    def __init__(self, model):
        import numpy as np
        from .pose_model import select_pose_landmarks
        self.model = model
        self.selected, self.order = select_pose_landmarks(model, model.landmark_ids)
        self.reset_indices = [self.selected.landmark_ids.index(i) for i in RESET_POINTS]
        self.rigid_state = np.zeros(6, dtype=np.float32)
        self.projected = np.zeros((len(self.selected.landmark_ids), 2), dtype=np.float32)
        self.reset()

    def reset(self):
        """Caller reset 0x14007e270 plus the fitting-object rebuild it triggers in 0x14007e330.

The parent pose is left as it was."""
        import numpy as np
        self.initialised = False
        self.frontal = False
        self.shape = np.zeros(len(self.model.shape_ranges), dtype=np.float32)
        self.animation = np.zeros(len(self.model.animation_ranges), dtype=np.float32)
        self.fit_mode = None
        self.fit_state = None
        self.fixed_shape = self.fixed_animation = None

    def process(self, normalized_points, width, height):
        import numpy as np
        from .pose_fit import prepare_observations
        points = np.asarray(normalized_points, dtype=np.float32)
        if points.shape != (len(self.model.landmark_ids), 2) or not np.isfinite(points).all():
            raise ValueError('Expected complete finite normalized landmarks')
        frame = _Frame(self, points[self.order], width, height)
        frame.observations, frame.weights = prepare_observations(
            frame.points, self.selected.landmark_ids, width, height)
        if not self.initialised:
            self.initialised = frame.initial_fit()
            self.frontal = is_near_frontal(pose_output(self.rigid_state)[0])
        valid = frame.temporal_fit()
        if not valid:
            self.rigid_state = np.array([0, FLOAT_PI, 0, 0, 0, 0], dtype=np.float32)
        elif is_near_frontal(pose_output(self.rigid_state)[0]) and not self.frontal:
            self.initialised = frame.initial_fit()
            frame.temporal_fit()
            self.frontal = True
        rotation, translation = pose_output(self.rigid_state)
        return {'valid': valid, 'rotation': rotation, 'translation': translation,
                'rigid_state': self.rigid_state.copy(), 'shape_coefficients': self.shape.copy(),
                'animation_coefficients': self.animation.copy(), 'initialised': self.initialised,
                'frontal': self.frontal, 'trace': frame.trace}


class _Frame:
    def __init__(self, fitter, points, width, height):
        import numpy as np
        self.fitter = fitter
        self.points = points
        self.width, self.height = width, height
        self.flipped = np.column_stack((points[:, 0], np.float32(1) - points[:, 1]))
        self.trace = []

    def reset_pose(self, yaw):
        """0x140052d70 rigid reset; deformation coefficients are left unchanged."""
        from .pose_fit import initial_rigid_state
        selected = self.fitter.selected
        index = self.fitter.reset_indices
        return initial_rigid_state(selected.vertices[index], self.points[index],
                                   self.width, self.height, selected.focal_factor, float(yaw))

    def build(self, mode):
        """0x1400dbba0: a new fitting object only when the flag differs from the last one."""
        fitter = self.fitter
        if fitter.fit_mode == mode:
            return fitter.fit_state
        fitter.fit_mode = mode
        fitter.fixed_shape, fitter.fixed_animation = fitter.shape.copy(), fitter.animation.copy()
        return None

    def fit(self, mode, iterations):
        """0x1400dbba0 + 0x1400dc3b0 without a pose hint; True when bounds hold.

Decoding writes only the active coefficient block back to the parent; the
fitting object keeps the inactive block copied when it was built.
"""
        from .pose_fit import mode_step
        from .pose_policy import check_pose_bounds
        fitter, selected = self.fitter, self.fitter.selected
        active = self.build(mode)
        if len(selected.landmark_ids) < MINIMUM_FIT_POINTS:
            return False
        valid = False
        for attempt in range(2):
            if attempt:
                fitter.rigid_state = self.reset_pose(FLOAT_PI)
                active = None
                iterations = 3
            for _ in range(iterations):
                shape = fitter.shape if mode == 3 else fitter.fixed_shape
                animation = fitter.animation if mode == 5 else fitter.fixed_animation
                result = mode_step(selected, self.observations, fitter.rigid_state, shape,
                                   animation, mode, self.weights, active)
                fitter.rigid_state, active = result['rigid_state'], result['active_state']
                if mode == 3:
                    fitter.shape = result['shape_coefficients']
                else:
                    fitter.animation = result['animation_coefficients']
            fitter.fit_state = active
            invalid, fitter.shape, fitter.animation = check_pose_bounds(
                fitter.rigid_state, fitter.shape, fitter.animation,
                selected.rotation_limits, selected.translation_limits)
            if not invalid:
                fitter.projected = finalized_projection(selected, fitter.rigid_state,
                                                        fitter.shape, fitter.animation)
                valid = True
                break
        self.trace.append({'mode': mode, 'valid': valid, 'attempts': attempt + 1})
        return valid

    def initial_fit(self):
        """0x14007ec70: zero coefficients, reset with yaw float(3), flag 3 for two updates."""
        import numpy as np
        fitter = self.fitter
        fitter.shape = np.zeros_like(fitter.shape)
        fitter.animation = np.zeros_like(fitter.animation)
        fitter.rigid_state = self.reset_pose(INITIAL_YAW)
        return self.fit(3, 2)

    def temporal_fit(self):
        """0x14007ee60: up to two flag-5 single-update attempts from the current pose."""
        fitter = self.fitter
        valid, attempt = False, 0
        while fitter.initialised:
            if valid or attempt >= 2:
                break
            facing = facing_degrees(fitter.rigid_state)
            valid = self.fit(5, 1)
            rectangle = eye_face_rectangle(self.points, fitter.selected.landmark_ids, self.width, self.height)
            if rectangle is None:
                raise NotImplementedError('Original rectangle output is uninitialised without eye/nose/mouth records')
            error = reprojection_error(fitter.selected, fitter.projected, self.flipped,
                                       self.width, self.height, float(rectangle[2]))
            self.trace[-1].update({'facing': float(facing), 'error': float(error), 'rectangle': rectangle})
            if not valid or (error > _f32(REPROJECTION_ERROR_LIMIT) and attempt == 0):
                if -FACING_REINITIALISE_DEGREES < facing < FACING_REINITIALISE_DEGREES:
                    fitter.initialised = self.initial_fit()
                else:
                    fitter.rigid_state = self.reset_pose(FLOAT_PI)
                valid = False
            attempt += 1
        return valid
