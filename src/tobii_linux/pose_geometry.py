"""Head-pose geometry stage recovered from the platform algorithm module.

Matrices are float32 4x4 or 3x3 arrays in the original's memory order and are
applied to row vectors, as the stage itself does. The arithmetic is
crates/tobii-headpose/src/geometry.rs.
"""
from ._arrays import floats
from ._native import pose_geometry as _geometry
from .pose_output import PI, euler_matrix, matrix_euler  # noqa: F401  (re-exported)

HALF = 0.5


def _vector(values, name):
    vector = floats(values)
    if vector.shape != (3,):
        raise ValueError(f'{name} must have three values')
    return vector.tolist()


def _matrix(values, size, name):
    matrix = floats(values)
    if matrix.shape != (size, size):
        raise ValueError(f'{name} must be {size}x{size}')
    return matrix.reshape(-1).tolist()


def _square(values, size):
    import numpy as np
    return np.array(values, dtype=np.float32).reshape(size, size)


def _vector3(values):
    import numpy as np
    return np.array(values, dtype=np.float32)


def x_rotation(angle):
    """0x1402b10a0."""
    return _square(_geometry.x_rotation(angle), 3)


def y_rotation(angle):
    """0x1402b1120."""
    return _square(_geometry.y_rotation(angle), 3)


def z_rotation(angle):
    """0x1402b11a0."""
    return _square(_geometry.z_rotation(angle), 3)


def geometry_config():
    """Constant settings of 0x14028fc30: the rotation 3x3 and the position 4x4."""
    config3, config4 = _geometry.geometry_config()
    return _square(config3, 3), _square(config4, 4)


def _corners(top_left, bottom_left, top_right):
    return tuple(_vector(v, 'Corner') for v in (top_left, bottom_left, top_right))


def display_normal(top_left, bottom_left, top_right):
    """0x1402af610: unit normal (top_right - top_left) x (top_left - bottom_left)."""
    return _vector3(_geometry.display_normal(*_corners(top_left, bottom_left, top_right)))


def display_center(top_left, bottom_left, top_right):
    """Display object slot +0x28 (0x1402afad0)."""
    return _vector3(_geometry.display_center(*_corners(top_left, bottom_left, top_right)))


def display_matrix(top_left, bottom_left, top_right):
    """0x140294230: maps a point in tracker millimetres to display-centred axes.

The axes are right (top_left to top_right), up (bottom_left to top_left) and
the display normal, so `[p, 1] @ matrix` gives `((p - c).right, (p - c).up,
(p - c).normal)` for display centre `c`.
"""
    return _square(_geometry.display_matrix(*_corners(top_left, bottom_left, top_right)), 4)


def camera_rotation(angles):
    """0x1402b2100 as called by 0x1402b00f0 with (a, b, c + pi): Rx(a) Ry(b) Rz(c + pi) for row vectors."""
    return _square(_geometry.camera_rotation(tuple(angles)), 3)


def camera_extrinsic(translation, angles):
    """Settings `+0x50` 4x4 built by 0x1402b00f0 from camera hardware info.

`translation` and `angles` are the camera record's six extrinsic values in
the order they are stored; the matrix maps camera coordinates to tracker
coordinates for row vectors.
"""
    return _square(_geometry.camera_extrinsic(_vector(translation, 'Translation'), tuple(angles)), 4)


def transform_position(position, camera, display, config4=None):
    """0x140294820: `[p, 1] @ (config4 @ camera @ display)`, divided by w."""
    if config4 is None:
        config4 = geometry_config()[1]
    position = _vector(position, 'Position')
    config4 = _matrix(config4, 4, 'Configuration')
    camera = _matrix(camera, 4, 'Camera matrix')
    display = _matrix(display, 4, 'Display matrix')
    return _vector3(_geometry.transform_position(position, camera, display, config4))


def transform_rotation(angles, camera, display, config3=None):
    """0x140294ba0: order-3 angles in camera axes to angles in display axes.

The matrix `euler_matrix(angles) @ config3 @ camera3 @ display3 @ Ry(pi)` is
converted back with `matrix_euler`; the first and third angles are negated.
"""
    if config3 is None:
        config3 = geometry_config()[0]
    camera = _matrix(camera, 4, 'Camera matrix')
    display = _matrix(display, 4, 'Display matrix')
    config3 = _matrix(config3, 3, 'Configuration')
    return _vector3(_geometry.transform_rotation(tuple(angles), camera, display, config3))


def geometry_stage(angles, position, camera, display, config=None):
    """Stage process 0x140294670: (rotation, position) in display axes."""
    config3, config4 = geometry_config() if config is None else config
    return (transform_rotation(angles, camera, display, config3),
            transform_position(position, camera, display, config4))
