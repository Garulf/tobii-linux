"""Recovered face-model projection primitive, not a pose estimator.

Coordinates remain in the fitting model's local convention. Camera calibration,
parameter optimization and public head-pose coordinate conversion are separate.
The arithmetic is crates/tobii-headpose/src/projection.rs.
"""
import math

from ._arrays import array, floats, raw
from ._native import pose_projection as _projection


def _points(vertices):
    points = floats(vertices)
    if points.ndim != 2 or points.shape[1] != 3:
        raise ValueError('Expected finite Nx3 vertices and three rotation/translation values')
    return points


def _vector3(values):
    vector = floats(values)
    if vector.shape != (3,):
        raise ValueError('Expected finite Nx3 vertices and three rotation/translation values')
    return vector


def project_points(vertices, rotation, translation, focal_factor, model_scale=1.0):
    """Project finite Nx3 model points with Ry(ry) @ Rx(rx) @ Rz(rz).

Angles are radians. Rotation/translation operate before perspective division;
no principal-point offset is added in this observed primitive. Returned values
are (projected Nx2, transformed Nx3). Inputs with zero depth are rejected.
"""
    points = _points(vertices)
    angles, offset = _vector3(rotation), _vector3(translation)
    if not math.isfinite(focal_factor) or not math.isfinite(model_scale):
        raise ValueError('Expected finite Nx3 vertices and three rotation/translation values')
    projected, transformed = _projection.project_points(points.tobytes(), angles.tobytes(), offset.tobytes(),
                                                        float(focal_factor), float(model_scale))
    return array(projected, len(points), 2), array(transformed, len(points), 3)


def _bounds(latent, ranges):
    latent, ranges = floats(latent), floats(ranges)
    if latent.ndim != 1 or ranges.shape != (len(latent), 2):
        raise ValueError('Expected finite latent vector and increasing bounds')
    return latent, ranges


def bounded_coefficients(latent, ranges):
    """Decode fitting latents via tanh(clamp(q,-3,3))*half_range+midpoint.

Returns both decoded coefficients and clamped latent state. This does not
assign a particular deformation block to animation/shape configuration.
"""
    latent, ranges = _bounds(latent, ranges)
    values, clamped = _projection.bounded_coefficients(latent.tobytes(), ranges.tobytes())
    return array(values, len(latent)), array(clamped, len(latent))


def _basis(basis, points, coefficients):
    basis, coefficients = floats(basis), floats(coefficients)
    if basis.shape != (*points.shape, len(coefficients)) or coefficients.ndim != 1:
        raise ValueError('Inconsistent/non-finite deformation basis')
    return basis, coefficients


def deform_vertices(vertices, first_basis, first_coefficients, second_basis, second_coefficients):
    """Add two Nx3xK deformation bases in the observed first-then-second order."""
    vertices = floats(vertices)
    if vertices.ndim != 2 or vertices.shape[1] != 3:
        raise ValueError('Expected finite Nx3 vertices')
    first, c1 = _basis(first_basis, vertices, first_coefficients)
    second, c2 = _basis(second_basis, vertices, second_coefficients)
    deformed = _projection.deform_vertices(vertices.tobytes(), first.tobytes(), c1.tobytes(),
                                           second.tobytes(), c2.tobytes())
    return array(deformed, len(vertices), 3)


def rigid_jacobian(vertices, rotation, translation, focal_factor, model_scale=1.0):
    """Analytic 2N×6 projection Jacobian for the six enabled rigid pose states.

Deformation-state columns and observation gating are not included here.
"""
    points = _points(vertices)
    jacobian = _projection.rigid_jacobian(points.tobytes(), raw(_vector3(rotation)), raw(_vector3(translation)),
                                          float(focal_factor), float(model_scale))
    return array(jacobian, 2 * len(points), 6)


def _block(basis, latent, ranges, mask, fixed, points):
    import numpy as np
    basis = floats(basis)
    latent, ranges = _bounds(latent, ranges)
    if basis.ndim != 3 or basis.shape[:2] != points.shape:
        raise ValueError('Inconsistent/non-finite deformation basis')
    enabled = None
    if mask is not None:
        enabled = np.asarray(mask, dtype=bool)
        if enabled.shape != (basis.shape[2],):
            raise ValueError('Deformation mask extent mismatch')
        enabled = [bool(x) for x in enabled]
    return (basis.tobytes(), basis.shape[2], latent.tobytes(), ranges.tobytes(), enabled,
            None if fixed is None else raw(fixed))


def model_jacobian(vertices, rotation, translation, focal_factor, model_scale,
                   first_basis, first_latent, first_ranges,
                   second_basis, second_latent, second_ranges,
                   first_mask=None, second_mask=None, *,
                   fixed_first_coefficients=None, fixed_second_coefficients=None):
    """Reproduce the original full model's update matrix and prediction.

The installed implementation transforms each basis component with
(1-tanh(b)^2)*tanh(b)*half_range. This is deliberately preserved; it is not
replaced with the usual derivative with respect to the current latent state.
"""
    points = _points(vertices)
    first = _block(first_basis, first_latent, first_ranges, first_mask, fixed_first_coefficients, points)
    second = _block(second_basis, second_latent, second_ranges, second_mask, fixed_second_coefficients, points)
    jacobian, columns, prediction = _projection.model_jacobian(
        points.tobytes(), raw(_vector3(rotation)), raw(_vector3(translation)), float(focal_factor),
        float(model_scale), first, second)
    return array(jacobian, 2 * len(points), columns), array(prediction, len(points), 2)
