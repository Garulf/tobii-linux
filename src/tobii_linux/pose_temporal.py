"""Recovered temporal fitting caller 0x14007e440 for the single-parent model.

VNNTrackerCore keeps one parent face model across frames. The first frame (or
the first after tracking is lost) runs the flag-3 initial fit 0x14007ec70 and
then the flag-5 temporal fit 0x14007ee60; later frames run only the temporal
fit, starting from the previous frame's pose and coefficients.
The caller is crates/tobii-headpose/src/temporal.rs.
"""
from ._arrays import array, floats, raw
from ._native import pose_temporal as _temporal

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


def facing_degrees(rigid_state):
    """0x14007ee60 facing angle from the model pose, before that attempt's fit."""
    import numpy as np
    return np.float32(_temporal.facing_degrees(raw(rigid_state)))


def pose_output(rigid_state):
    """0x140055470 output: (rx, ry - float32(pi), rz) and the translation."""
    import numpy as np
    rotation, translation = _temporal.pose_output(raw(rigid_state))
    return np.array(rotation, dtype=np.float32), np.array(translation, dtype=np.float32)


def is_near_frontal(output_rotation):
    """COMISS + JAE: a NaN angle counts as below the limit."""
    return _temporal.is_near_frontal((output_rotation[0], output_rotation[1]))


def eye_face_rectangle(normalized_points, landmark_ids, width, height):
    """0x14007a090 square from eye centres, nose (9.4/9.5) and FDP 8.1.

Returns (x, y, size, size) in pixels, or None when a required record is absent;
the original then leaves its output uninitialised.
"""
    ids = [tuple(identifier) for identifier in landmark_ids]
    points = floats(normalized_points).reshape(-1, 2)
    count = min(len(ids), len(points))
    rectangle = _temporal.eye_face_rectangle(points[:count].tobytes(), ids[:count], width, height)
    return None if rectangle is None else tuple(rectangle)


def finalized_projection(model, rigid_state, shape_coefficients, animation_coefficients):
    """Mesh finalization 0x140052710 projection (+0xb8) with zero eye offsets.

The divide is focal/Z followed by X*(focal/Z), unlike the fitting prediction.
"""
    from .pose_model import _native_model
    projected = _temporal.finalized_projection(_native_model(model), raw(rigid_state), raw(shape_coefficients),
                                               raw(animation_coefficients))
    return array(projected, len(model.vertices), 2)


def reprojection_error(model, projected, flipped_points, width, height, rectangle_width):
    """0x1400551c0 mean pixel distance between records and model, over rectangle width.

flipped_points are the Y-flipped caller copy (0x14007ac80) in model.landmark_ids
order. Groups 13 and 15 are skipped; summation follows numeric FDP order.
"""
    import numpy as np
    return np.float32(_temporal.reprojection_error([tuple(i) for i in model.landmark_ids], raw(projected),
                                                   raw(flipped_points), width, height, float(rectangle_width)))


class TemporalPoseFitter:
    """Stateful 0x14007e440 caller (last argument 1) for complete D1 landmarks.

Persistent state mirrors caller +0x59 (initialised), +0x39 (frontal shape refit
done), the parent model's pose and physical coefficients, and the fitting
object built by 0x1400dbba0. That object keeps its own latent state and is
re-encoded from the parent only when the requested flag changes, so parent
pose resets made between two same-flag fits do not move the next fit's start.
Extra parent models and the caller +0x58 external pose hint are not supported.
`accumulation='float32'` sums the updates in float32 instead of float64, only
to measure how sensitive a frame is to arithmetic order.
"""
    def __init__(self, model, accumulation='float64'):
        from .pose_model import _native_model, select_pose_landmarks
        self.model = model
        self.selected, self.order = select_pose_landmarks(model, model.landmark_ids)
        self.reset_indices = [self.selected.landmark_ids.index(i) for i in RESET_POINTS]
        self._native = _temporal.TemporalPoseFitter(_native_model(model), accumulation)

    def reset(self):
        """Caller reset 0x14007e270 plus the fitting-object rebuild it triggers in 0x14007e330.

The parent pose is left as it was."""
        self._native.reset()

    def process(self, normalized_points, width, height):
        import numpy as np
        points = floats(normalized_points)
        if points.shape != (len(self.model.landmark_ids), 2) or not np.isfinite(points).all():
            raise ValueError('Expected complete finite normalized landmarks')
        result = self._native.process(points.tobytes(), width, height)
        ns, na = len(self.model.shape_ranges), len(self.model.animation_ranges)
        return {'valid': result['valid'], 'rotation': array(result['rotation'], 3),
                'translation': array(result['translation'], 3), 'rigid_state': array(result['rigid_state'], 6),
                'shape_coefficients': array(result['shape_coefficients'], ns),
                'animation_coefficients': array(result['animation_coefficients'], na),
                'initialised': result['initialised'], 'frontal': result['frontal'], 'trace': result['trace']}

    @property
    def rigid_state(self):
        return array(self._native.rigid_state, 6)

    @rigid_state.setter
    def rigid_state(self, state):
        self._native.rigid_state = raw(state)

    @property
    def initialised(self):
        return self._native.initialised

    @initialised.setter
    def initialised(self, value):
        self._native.initialised = bool(value)

    @property
    def frontal(self):
        return self._native.frontal

    @frontal.setter
    def frontal(self, value):
        self._native.frontal = bool(value)

    @property
    def shape(self):
        return array(self._native.shape, len(self.model.shape_ranges))

    @shape.setter
    def shape(self, values):
        self._native.shape = raw(values)

    @property
    def animation(self):
        return array(self._native.animation, len(self.model.animation_ranges))

    @animation.setter
    def animation(self, values):
        self._native.animation = raw(values)

    @property
    def projected(self):
        return array(self._native.projected, len(self.selected.landmark_ids), 2)

    @property
    def fit_mode(self):
        return self._native.fit_mode

    @property
    def fit_state(self):
        state = self._native.fit_state
        return None if state is None else array(state, len(state) // 4)
