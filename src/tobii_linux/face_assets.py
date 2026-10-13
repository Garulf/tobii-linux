"""Readers for the observed plaintext face/landmark assets.

These preserve file coordinates and identifiers. They do not implement Tobii's
pose solver, assert coordinate units, or infer deformation coefficient scales.
"""
from dataclasses import dataclass
import math
from pathlib import Path
import struct


def _floats(items):
    result = tuple(float(x) for x in items)
    if not all(math.isfinite(x) for x in result):
        raise ValueError('Non-finite coordinate')
    return result


def read_shapes(path, dimensions=2):
    """Read LE uint32 dimensions followed by packed LE float32 values.

init_shape.bin has two dimensions; NN/is.bin has three. Exact file extent is
required, so an opaque or padded format is not silently accepted.
"""
    data = Path(path).read_bytes()
    if dimensions not in (2, 3) or len(data) < dimensions * 4:
        raise ValueError('Invalid shape header')
    shape = struct.unpack_from('<' + 'I' * dimensions, data)
    count = math.prod(shape)
    if not all(shape) or count > 1_000_000 or len(data) != (dimensions + count) * 4:
        raise ValueError('Shape extent mismatch')
    values = struct.unpack_from('<' + 'f' * count, data, dimensions * 4)
    if not all(math.isfinite(x) for x in values):
        raise ValueError('Non-finite shape coordinate')
    return shape, values


def read_landmark_ids(path):
    data = Path(path).read_bytes()
    if len(data) < 4:
        raise ValueError('Missing landmark header')
    count = struct.unpack_from('<I', data)[0]
    if not 0 < count <= 10000 or len(data) != 4 + count * 8:
        raise ValueError('Landmark extent mismatch')
    pairs = tuple(struct.iter_unpack('<II', data[4:]))
    if len(set(pairs)) != count:
        raise ValueError('Duplicate landmark identifier')
    return pairs


def read_fdp(path):
    lines = [line.strip() for line in Path(path).read_text().splitlines()
             if line.strip() and not line.lstrip().startswith('#')]
    if not lines or not lines[0].startswith('1.0 '):
        raise ValueError('Unsupported FDP header')
    result = {}
    for line in lines[1:]:
        fields = line.split()
        if len(fields) != 5:
            raise ValueError('Unexpected FDP row')
        identifier = tuple(int(x) for x in fields[0].split('.'))
        reference = tuple(int(x) for x in fields[4].split('.'))
        if len(identifier) != 2 or len(reference) != 2 or identifier in result:
            raise ValueError('Invalid/duplicate FDP mapping')
        result[identifier] = {'position': _floats(fields[1:4]),
                              'mesh': reference[0], 'vertex': reference[1]}
    return result


@dataclass(frozen=True)
class FaceMesh:
    vertices: tuple
    texcoords: tuple
    triangles: tuple
    animation_units: tuple
    shape_units: tuple


def read_mesh(path):
    lines = iter(line.strip() for line in Path(path).read_text().splitlines() if line.strip())

    def take():
        try:
            return next(lines)
        except StopIteration as exc:
            raise ValueError('Truncated mesh') from exc

    def header(expected):
        if take() != expected:
            raise ValueError(f'Expected mesh section {expected}')
        count = int(take())
        if not 0 <= count <= 1_000_000:
            raise ValueError('Invalid mesh count')
        return count

    def rows(count, width, convert):
        result = []
        for _ in range(count):
            fields = take().split()
            if len(fields) != width:
                raise ValueError('Unexpected mesh row width')
            result.append(convert(fields))
        return tuple(result)

    vertices = rows(header('# VERTEX LIST:'), 3, _floats)
    texcoords = rows(header('# TEXCOORD LIST:'), 2, _floats)
    triangles = rows(header('# FACE LIST:'), 3, lambda fields: tuple(map(int, fields)))
    if len(vertices) != len(texcoords) or any(i < 0 or i >= len(vertices) for t in triangles for i in t):
        raise ValueError('Invalid mesh indexing')

    def units(label):
        result = []
        for _ in range(header(label)):
            name = take()
            if not name.startswith('# '):
                raise ValueError('Missing deformation name')
            count = int(take())
            if not 0 <= count <= len(vertices):
                raise ValueError('Invalid deformation count')
            entries = rows(count, 4, lambda f: (int(f[0]), *_floats(f[1:])))
            indices = [e[0] for e in entries]
            if len(set(indices)) != count or any(i < 0 or i >= len(vertices) for i in indices):
                raise ValueError('Invalid deformation indexing')
            result.append((name[2:], entries))
        return tuple(result)

    animation = units('# ANIMATION UNITS LIST:')
    shapes = units('# SHAPE UNITS LIST:')
    if next(lines, None) is not None:
        raise ValueError('Unexpected trailing mesh data')
    return FaceMesh(vertices, texcoords, triangles, animation, shapes)


def landmark_vertices(mesh, mapping, ids):
    """Join file IDs to mesh vertices; this is not a fitted face or posed model."""
    result = []
    for identifier in ids:
        reference = mapping[identifier]
        if reference['mesh'] != 0 or not 0 <= reference['vertex'] < len(mesh.vertices):
            raise ValueError('Unsupported mesh reference')
        result.append(mesh.vertices[reference['vertex']])
    return tuple(result)
