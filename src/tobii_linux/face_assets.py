"""Readers for the observed plaintext face/landmark assets.

These preserve file coordinates and identifiers. They do not implement Tobii's
pose solver, assert coordinate units, or infer deformation coefficient scales.
The readers are crates/tobii-models/src/face.rs.
"""
from dataclasses import dataclass

from ._native import face_assets as _face


def read_shapes(path, dimensions=2):
    """Read LE uint32 dimensions followed by packed LE float32 values.

init_shape.bin has two dimensions; NN/is.bin has three. Exact file extent is
required, so an opaque or padded format is not silently accepted.
"""
    shape, values = _face.read_shapes(path, dimensions)
    return tuple(shape), tuple(values)


def read_landmark_ids(path):
    return tuple(_face.read_landmark_ids(path))


def read_fdp(path):
    return {identifier: {'position': position, 'mesh': mesh, 'vertex': vertex}
            for identifier, position, mesh, vertex in _face.read_fdp(path)}


@dataclass(frozen=True)
class FaceMesh:
    vertices: tuple
    texcoords: tuple
    triangles: tuple
    animation_units: tuple
    shape_units: tuple


def read_mesh(path):
    vertices, texcoords, triangles, animation, shapes = _face.read_mesh(path)

    def units(items):
        return tuple((name, tuple(entries)) for name, entries in items)

    return FaceMesh(tuple(vertices), tuple(texcoords), tuple(triangles), units(animation), units(shapes))


def landmark_vertices(mesh, mapping, ids):
    """Join file IDs to mesh vertices; this is not a fitted face or posed model."""
    result = []
    for identifier in ids:
        reference = mapping[identifier]
        if reference['mesh'] != 0 or not 0 <= reference['vertex'] < len(mesh.vertices):
            raise ValueError('Unsupported mesh reference')
        result.append(mesh.vertices[reference['vertex']])
    return tuple(result)
