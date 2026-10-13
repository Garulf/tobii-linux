"""Head-pose geometry stage recovered from the platform algorithm module.

Matrices are float32 4x4 or 3x3 arrays in the original's memory order and are
applied to row vectors, as the stage itself does.
"""
import math
from tobii_linux.pose_output import PI, euler_matrix, matrix_euler

HALF = 0.5


def _float32(value):
    import numpy as np
    return np.float32(value)


def _vector(values, name):
    import numpy as np
    vector = np.asarray(values, dtype=np.float32)
    if vector.shape != (3,):
        raise ValueError(f'{name} must have three values')
    return vector


def _matrix(values, size, name):
    import numpy as np
    matrix = np.asarray(values, dtype=np.float32)
    if matrix.shape != (size, size):
        raise ValueError(f'{name} must be {size}x{size}')
    return matrix


def _normalized(vector):
    """Divide by the length when the squared length (y, z first) is positive."""
    x, y, z = vector
    squared = _float32(_float32(y * y + z * z) + x * x)
    if squared > 0:
        length = _float32(math.sqrt(squared))
        return (vector / length).astype(vector.dtype)
    return vector


def _dot(row, vector):
    """Three-term dot product summed as (1 + 2) + 0, the order used throughout the stage."""
    return _float32(_float32(row[1] * vector[1] + row[2] * vector[2]) + row[0] * vector[0])


def _sincos(angle):
    angle = _float32(angle)
    return _float32(math.cos(angle)), _float32(math.sin(angle))


def x_rotation(angle):
    """0x1402b10a0."""
    import numpy as np
    c, s = _sincos(angle)
    return np.array([[1, 0, 0], [0, c, s], [0, -s, c]], dtype=np.float32)


def y_rotation(angle):
    """0x1402b1120."""
    import numpy as np
    c, s = _sincos(angle)
    return np.array([[c, 0, -s], [0, 1, 0], [s, 0, c]], dtype=np.float32)


def z_rotation(angle):
    """0x1402b11a0."""
    import numpy as np
    c, s = _sincos(angle)
    return np.array([[c, s, 0], [-s, c, 0], [0, 0, 1]], dtype=np.float32)


def _homogeneous(rotation, translation):
    import numpy as np
    matrix = np.zeros((4, 4), dtype=np.float32)
    matrix[:3, :3] = rotation
    matrix[3, :3] = translation
    matrix[3, 3] = 1
    return matrix


def geometry_config():
    """Constant settings of 0x14028fc30: the rotation 3x3 and the position 4x4."""
    import numpy as np
    return x_rotation(PI), _homogeneous(z_rotation(PI), np.zeros(3, dtype=np.float32))


def _product3(left, right):
    """3x3 product with each entry summed as (1 + 2) + 0, as 0x140294ba0."""
    import numpy as np
    result = np.empty((3, 3), dtype=np.float32)
    for i in range(3):
        for j in range(3):
            result[i, j] = _dot(left[i], right[:, j])
    return result


def _product4(left, right):
    """4x4 product with each entry summed as ((0 + 1) + 2) + 3, as 0x140294820."""
    import numpy as np
    result = np.empty((4, 4), dtype=np.float32)
    for i in range(4):
        for j in range(4):
            a, b = left[i], right[:, j]
            total = _float32(a[1] * b[1] + a[0] * b[0])
            total = _float32(total + a[2] * b[2])
            result[i, j] = _float32(total + a[3] * b[3])
    return result


def display_normal(top_left, bottom_left, top_right):
    """0x1402af610: unit normal (top_right - top_left) x (top_left - bottom_left)."""
    import numpy as np
    top_left, bottom_left, top_right = (_vector(v, 'Corner') for v in (top_left, bottom_left, top_right))
    up = top_left - bottom_left
    right = top_right - top_left
    normal = np.array([right[1] * up[2] - right[2] * up[1],
                       right[2] * up[0] - up[2] * right[0],
                       up[1] * right[0] - right[1] * up[0]], dtype=np.float32)
    return _normalized(normal)


def display_center(top_left, bottom_left, top_right):
    """Display object slot +0x28 (0x1402afad0)."""
    top_left, bottom_left, top_right = (_vector(v, 'Corner') for v in (top_left, bottom_left, top_right))
    half_up = ((top_left - bottom_left) * _float32(HALF)).astype(top_left.dtype)
    half_right = ((top_right - top_left) * _float32(HALF)).astype(top_left.dtype)
    return ((half_up + bottom_left) + half_right).astype(top_left.dtype)


def display_matrix(top_left, bottom_left, top_right):
    """0x140294230: maps a point in tracker millimetres to display-centred axes.

The axes are right (top_left to top_right), up (bottom_left to top_left) and
the display normal, so `[p, 1] @ matrix` gives `((p - c).right, (p - c).up,
(p - c).normal)` for display centre `c`.
"""
    import numpy as np
    top_left, bottom_left, top_right = (_vector(v, 'Corner') for v in (top_left, bottom_left, top_right))
    up = _normalized(top_left - bottom_left)
    right = _normalized(top_right - top_left)
    normal = display_normal(top_left, bottom_left, top_right)
    center = display_center(top_left, bottom_left, top_right)
    axes = np.stack([right, up, normal], axis=1)
    offset = np.array([-_dot(axis, center) for axis in (right, up, normal)], dtype=np.float32)
    return _homogeneous(axes, offset)


def camera_rotation(angles):
    """0x1402b2100 as called by 0x1402b00f0 with (a, b, c + pi): Rx(a) Ry(b) Rz(c + pi) for row vectors."""
    a, b, c = (_float32(v) for v in angles)
    c = _float32(c + _float32(PI))
    x, y, z = x_rotation(a), y_rotation(b), z_rotation(c)
    return _product3(z, _product3(y, x))


def camera_extrinsic(translation, angles):
    """Settings `+0x50` 4x4 built by 0x1402b00f0 from camera hardware info.

`translation` and `angles` are the camera record's six extrinsic values in
the order they are stored; the matrix maps camera coordinates to tracker
coordinates for row vectors.
"""
    return _homogeneous(camera_rotation(angles), _vector(translation, 'Translation'))


def transform_position(position, camera, display, config4=None):
    """0x140294820: `[p, 1] @ (config4 @ camera @ display)`, divided by w."""
    import numpy as np
    if config4 is None:
        config4 = geometry_config()[1]
    position = _vector(position, 'Position')
    combined = _product4(_matrix(config4, 4, 'Configuration'),
                         _product4(_matrix(camera, 4, 'Camera matrix'), _matrix(display, 4, 'Display matrix')))
    x, y, z = position
    row = np.empty(4, dtype=np.float32)
    for j in range(4):
        total = _float32(y * combined[1, j] + x * combined[0, j])
        total = _float32(total + z * combined[2, j])
        row[j] = _float32(combined[3, j] * _float32(1) + total)
    return (row[:3] / row[3]).astype(np.float32)


def transform_rotation(angles, camera, display, config3=None):
    """0x140294ba0: order-3 angles in camera axes to angles in display axes.

The matrix `euler_matrix(angles) @ config3 @ camera3 @ display3 @ Ry(pi)` is
converted back with `matrix_euler`; the first and third angles are negated.
"""
    import numpy as np
    if config3 is None:
        config3 = geometry_config()[0]
    camera3 = _matrix(camera, 4, 'Camera matrix')[:3, :3]
    display3 = _matrix(display, 4, 'Display matrix')[:3, :3]
    product = _product3(display3, y_rotation(PI))
    product = _product3(camera3, product)
    product = _product3(_matrix(config3, 3, 'Configuration'), product)
    product = _product3(euler_matrix(angles), product)
    a, b, c = matrix_euler(product)
    return np.array([-a, b, -c], dtype=np.float32)


def geometry_stage(angles, position, camera, display, config=None):
    """Stage process 0x140294670: (rotation, position) in display axes."""
    config3, config4 = geometry_config() if config is None else config
    return (transform_rotation(angles, camera, display, config3),
            transform_position(position, camera, display, config4))
