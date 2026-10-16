"""Recovered fitting mesh centering and scaling, without device-unit claims.

The arithmetic is crates/tobii-headpose/src/mesh.rs.
"""
from ._arrays import array, floats
from ._native import pose_mesh as _mesh


def dense_deformations(mesh):
    """Expand plaintext sparse file units to Nx3xK arrays in file order."""
    points = len(mesh.vertices)
    animation, shape = _mesh.dense_deformations(points, [list(entries) for _, entries in mesh.animation_units],
                                                [list(entries) for _, entries in mesh.shape_units])
    return (array(animation, points, 3, len(mesh.animation_units)),
            array(shape, points, 3, len(mesh.shape_units)))


def _bases(vertices, first_basis, second_basis):
    first, second = floats(first_basis), floats(second_basis)
    for basis in (first, second):
        if basis.ndim != 3 or basis.shape[:2] != vertices.shape:
            raise ValueError('Invalid deformation basis')
    return first, second


def _result(vertices, first, second, result):
    moved, scaled_first, scaled_second = result
    return (array(moved, *vertices.shape), array(scaled_first, *first.shape),
            array(scaled_second, *second.shape))


def normalize_mesh(vertices, first_basis, second_basis, landmark_vertex_ids, target_distance=0.065):
    """Center four mapped landmarks and scale their XY midpoint separation.

Indices are ordered FDP (3,7), (3,11), (3,8), (3,12). The default 0.065 is
observed in the runtime constructors; public coordinate units are not inferred.
"""
    import numpy as np
    vertices = floats(vertices)
    ids = tuple(landmark_vertex_ids)
    if (vertices.ndim != 2 or vertices.shape[1] != 3 or len(ids) != 4
            or any(not isinstance(i, (int, np.integer)) or not 0 <= i < len(vertices) for i in ids)):
        raise ValueError('Invalid model vertices, four indices or scale target')
    first, second = _bases(vertices, first_basis, second_basis)
    return _result(vertices, first, second, _mesh.normalize_mesh(
        vertices.tobytes(), first.tobytes(), first.shape[2], second.tobytes(), second.shape[2],
        [int(i) for i in ids], float(target_distance)))


def rescale_mesh(vertices, first_basis, second_basis, previous_distance, target_distance):
    """Apply the subsequent runtime distance change (0x140052bc0)."""
    vertices = floats(vertices)
    if vertices.ndim != 2 or vertices.shape[1] != 3:
        raise ValueError('Expected Nx3 vertices')
    first, second = _bases(vertices, first_basis, second_basis)
    return _result(vertices, first, second, _mesh.rescale_mesh(
        vertices.tobytes(), first.tobytes(), first.shape[2], second.tobytes(), second.shape[2],
        float(previous_distance), float(target_distance)))
