"""Prepare the installed fitting model; no camera capture or outer tracker policy."""
from dataclasses import dataclass
from pathlib import Path
from .face_assets import read_mesh, read_fdp, read_landmark_ids
from .pose_mesh import dense_deformations, normalize_mesh


def numeric_config(path, key, count):
    """Read one observed numeric field, including multiline continuation rows."""
    values = None
    for raw in Path(path).read_text().splitlines():
        fields = raw.split('#', 1)[0].split()
        if not fields:
            continue
        if fields[0] == key:
            if values is not None:
                raise ValueError(f'Duplicate configuration field {key}')
            values = []
            fields = fields[1:]
        elif values is None:
            continue
        try:
            row = [float(x) for x in fields]
        except ValueError:
            if values is not None:
                break
            continue
        values.extend(row)
    import math
    if values is None or len(values) != count or not all(math.isfinite(x) for x in values):
        raise ValueError(f'Expected {count} finite values for {key}')
    return values


def coefficient_latents(coefficients, ranges):
    """Original float32 log-ratio encoder, restricted to interior coefficients."""
    import numpy as np
    import math
    coefficients = np.asarray(coefficients, dtype=np.float32)
    ranges = np.asarray(ranges, dtype=np.float32)
    if (coefficients.ndim != 1 or ranges.shape != (len(coefficients), 2)
            or not np.isfinite(coefficients).all() or not np.isfinite(ranges).all()
            or np.any(ranges[:, 0] >= ranges[:, 1])):
        raise ValueError('Invalid coefficient bounds')
    half = (ranges[:, 1] - ranges[:, 0]) * np.float32(.5)
    middle = (ranges[:, 1] + ranges[:, 0]) * np.float32(.5)
    unit = (coefficients - middle) / half
    if np.any(np.abs(unit) >= 1):
        raise ValueError('Coefficient must be strictly inside its bounds')
    ratio = (unit + np.float32(1)) / (np.float32(1) - unit)
    return np.asarray([math.log(float(x)) for x in ratio], dtype=np.float32) * np.float32(.5)


@dataclass(frozen=True)
class PreparedPoseModel:
    landmark_ids: tuple
    vertices: object
    shape_basis: object
    animation_basis: object
    shape_ranges: object
    animation_ranges: object
    shape_mask: object
    animation_mask: object
    state_sensitivity: object
    rotation_limits: object
    translation_limits: object
    focal_factor: float

    def initial_state(self, rigid_state):
        import numpy as np
        rigid = np.asarray(rigid_state, dtype=np.float32)
        if rigid.shape != (6,) or not np.isfinite(rigid).all():
            raise ValueError('Expected six finite rigid state values')
        return np.concatenate((rigid,
            coefficient_latents(np.zeros(len(self.shape_ranges)), self.shape_ranges),
            coefficient_latents(np.zeros(len(self.animation_ranges)), self.animation_ranges)))

    def step(self, observations, state, observation_sensitivity=None):
        from .pose_fit import deformation_step
        return deformation_step(self.vertices, observations, state, self.focal_factor, 1,
            self.shape_basis, self.shape_ranges, self.animation_basis, self.animation_ranges,
            self.state_sensitivity, observation_sensitivity, self.shape_mask, self.animation_mask)


def load_pose_model(support_directory):
    """Load the observed jk_300/D1 configuration and select its 57 mapped vertices.

Keeps all 40 shape and 23 animation states, zeroing disabled basis columns as
observed in the runtime loader. This does not select a live tracker mode.
"""
    import numpy as np
    support = Path(support_directory)
    cfg = support / 'Head Tracker.cfg'
    mesh = read_mesh(support / 'jk_300.wfm')
    mapping = read_fdp(support / 'jk_300_rigid.fdp')
    ids = read_landmark_ids(support / 'bdtsdata/NN/vnn_nir/landmarks.bin')
    ns, na = len(mesh.shape_units), len(mesh.animation_units)
    def field(key, count):
        return np.asarray(numeric_config(cfg, key, count), dtype=np.float32)
    shape_mask = field('pose_fitting_su_use', ns)
    animation_mask = field('pose_fitting_au_use', na)
    if not all(np.isin(x, [0, 1]).all() for x in (shape_mask, animation_mask)):
        raise ValueError('Invalid deformation enable mask')
    def indices(identifiers):
        result = []
        for identifier in identifiers:
            record = mapping[identifier]
            if record['mesh'] != 0 or not 0 <= record['vertex'] < len(mesh.vertices):
                raise ValueError('Unsupported mesh reference')
            result.append(record['vertex'])
        return result
    animation, shape = dense_deformations(mesh)
    vertices, shape, animation = normalize_mesh(mesh.vertices, shape, animation,
        indices(((3, 7), (3, 11), (3, 8), (3, 12))))
    shape[:, :, shape_mask == 0] = 0
    animation[:, :, animation_mask == 0] = 0
    selected = indices(ids)
    shape_ranges = np.tile(np.array([-1, 1], dtype=np.float32), (ns, 1))
    animation_ranges = field('action_unit_limit', 2 * na).reshape(na, 2)
    sensitivity = np.concatenate((field('pose_fitting_pose_sensitivity', 6),
        field('pose_fitting_su_sensitivity', ns), field('pose_fitting_au_sensitivity', na)))
    if np.any(sensitivity <= 1e-8) or np.any(animation_ranges[:, 0] >= animation_ranges[:, 1]):
        raise ValueError('Unsupported sensitivity or coefficient range')
    return PreparedPoseModel(ids, vertices[selected], shape[selected], animation[selected],
        shape_ranges, animation_ranges, shape_mask.astype(bool), animation_mask.astype(bool), sensitivity,
        field('rotation_limit', 6).reshape(3, 2), field('translation_limit', 6).reshape(3, 2),
        float(field('camera_focus', 1)[0]))


def select_pose_landmarks(model, present_ids):
    """Select present mapped D1 landmarks in original numeric FDP scan order.

Returns the reduced/reordered model and indices into the input model's point
array. Aliases absent from the rigid FDP map are ignored. Presence is supplied
by the caller; this does not guess validity from a neural quality score.
"""
    import numpy as np
    from dataclasses import replace
    present = set(tuple(x) for x in present_ids)
    selected = tuple(sorted(present.intersection(model.landmark_ids)))
    if not selected:
        raise ValueError('No mapped present landmarks')
    indices = np.asarray([model.landmark_ids.index(identifier) for identifier in selected], dtype=np.intp)
    return replace(model, landmark_ids=selected, vertices=model.vertices[indices],
                   shape_basis=model.shape_basis[indices], animation_basis=model.animation_basis[indices]), indices
