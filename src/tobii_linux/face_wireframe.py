"""The fitted jk_300 face mesh as a wireframe: in the camera image and in display coordinates.

The mesh, its normalisation and its deformation bases are prepared exactly as
pose_model.load_pose_model prepares them, but for all 357 vertices instead of
the 57 landmark vertices the fit uses. A fit result (rigid state plus shape and
animation coefficients) deforms and projects it the way the fitter does
(pose_projection.deform_vertices and project_points at model scale 1).
"""
from dataclasses import dataclass
from pathlib import Path

from .face_assets import read_fdp, read_mesh
from .pose_geometry import geometry_config, geometry_stage, y_rotation
from .pose_mesh import dense_deformations, normalize_mesh
from .pose_output import PI, MILLIMETRES_PER_METRE, adapter_head_pose, euler_matrix, matrix_euler
from .pose_projection import deform_vertices, project_points

NORMALIZATION_POINTS = ((3, 7), (3, 11), (3, 8), (3, 12))


def triangle_edges(triangles):
    """Each undirected triangle edge once, in first-seen order."""
    seen, edges = set(), []
    for a, b, c in triangles:
        for edge in ((a, b), (b, c), (c, a)):
            key = (min(edge), max(edge))
            if key not in seen:
                seen.add(key)
                edges.append(key)
    return tuple(edges)


@dataclass(frozen=True)
class FaceWireframe:
    """Normalised mesh, deformation bases and edges; landmark_indices are the vertices of the fitted landmarks."""
    vertices: object
    shape_basis: object
    animation_basis: object
    edges: tuple
    focal_factor: float
    landmark_indices: tuple

    @classmethod
    def load(cls, support_directory, pose_model=None):
        """The mesh of `support_directory`; masks and focal factor come from its loaded pose model."""
        from .pose_model import load_pose_model
        support = Path(support_directory)
        model = pose_model if pose_model is not None else load_pose_model(support)
        mesh = read_mesh(support/'jk_300.wfm')
        mapping = read_fdp(support/'jk_300_rigid.fdp')
        animation, shape = dense_deformations(mesh)
        vertices, shape, animation = normalize_mesh(
            mesh.vertices, shape, animation, [mapping[i]['vertex'] for i in NORMALIZATION_POINTS])
        shape[:, :, ~model.shape_mask] = 0
        animation[:, :, ~model.animation_mask] = 0
        landmarks = tuple(mapping[tuple(i)]['vertex'] for i in model.landmark_ids)
        return cls(vertices, shape, animation, triangle_edges(mesh.triangles), model.focal_factor, landmarks)

    def deformed(self, fit):
        return deform_vertices(self.vertices, self.shape_basis, fit['shape_coefficients'],
                               self.animation_basis, fit['animation_coefficients'])

    def camera_points(self, fit):
        """(fitting-plane Nx2, camera-space Nx3 in metres) of the deformed mesh at the fitted pose."""
        rigid = fit['rigid_state']
        return project_points(self.deformed(fit), rigid[:3], rigid[3:], self.focal_factor, 1)

    def image_points(self, fit, width, height):
        """Pixel coordinates, Y down, inverting the fitter's observed = (1 - 2 p) * aspect."""
        import numpy as np
        plane, _ = self.camera_points(fit)
        aspect = np.array([width / height if width > height else 1.0,
                           height / width if height > width else 1.0], dtype=np.float32)
        normalized = (1 - plane / aspect) * 0.5
        return normalized * np.array([width, height], dtype=np.float32)

    def display_points(self, fit, output, sample):
        """Mesh in display millimetres (display-centred: right, up, normal) at `sample`'s filtered pose.

`output` is the HeadPoseOutput that produced `sample` from this fit. The mesh is
placed with the geometry stage's position transform, then turned and moved
from the unfiltered pose to the filtered one.
"""
        import numpy as np
        _, camera = self.camera_points(fit)
        combined = geometry_config()[1] @ output.camera @ output.display
        rows = np.column_stack((camera * np.float32(MILLIMETRES_PER_METRE), np.ones(len(camera), np.float32)))
        placed = rows @ combined
        placed = placed[:, :3] / placed[:, 3:]
        unfiltered_rotation, unfiltered_position = unfiltered_pose(fit['rigid_state'], output)
        turn = (_display_rotation(unfiltered_rotation).T @ _display_rotation(sample.rotation_rad))
        flip = y_rotation(PI)
        return (placed - unfiltered_position) @ (flip @ turn @ flip) + np.asarray(sample.position_mm, np.float32)


def unfiltered_pose(rigid_state, output):
    """Display-axes rotation and position of HeadPoseOutput before its One Euro filters."""
    import numpy as np
    state = np.asarray(rigid_state, dtype=np.float32)
    rotation = (state[0], np.float32(state[1] - np.float32(PI)), state[2])
    matrix, position = adapter_head_pose(rotation, state[3:])
    angles = matrix_euler(matrix)
    return geometry_stage(angles[:3], position, output.camera, output.display)


def _display_rotation(angles):
    """Row-vector matrix whose matrix_euler gives `angles` before the geometry stage's sign flips."""
    a, b, c = angles
    return euler_matrix((-a, b, -c))


def display_scene(display_area, display_matrix):
    """Display corners and the tracker origin in the display-centred axes head pose is reported in."""
    import numpy as np
    top_left, top_right, bottom_left = (np.asarray(c, np.float32) for c in
                                        (display_area.top_left, display_area.top_right, display_area.bottom_left))
    corners = np.stack([top_left, top_right, top_right + bottom_left - top_left, bottom_left,
                        np.zeros(3, np.float32)])
    placed = np.column_stack((corners, np.ones(len(corners), np.float32))) @ display_matrix
    return placed[:4, :3], placed[4, :3]
