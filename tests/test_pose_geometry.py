import importlib.util
import math
import unittest

TOP_LEFT = (-296.6200866699219, 326.0040588378906, 111.8187484741211)
BOTTOM_LEFT = (-296.6200866699219, 10.267330169677734, -3.100020170211792)
TOP_RIGHT = (300.3799133300781, 326.0040588378906, 111.8187484741211)
CAMERA = ((10.0, -20.0, 5.0), (0.3, 0.1, -0.2))


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class PoseGeometryTests(unittest.TestCase):
    def test_display_matrix_matches_emulated_original(self):
        import numpy as np
        from tobii_linux.pose_geometry import display_matrix
        expected = np.array([[1.0, 0.0, 0.0, 0.0],
                             [0.0, 0.9396926164627075, -0.3420201241970062, 0.0],
                             [0.0, 0.3420201539993286, 0.9396925568580627, 0.0],
                             [-1.879913330078125, -176.58786010742188, 6.424701690673828, 1.0]], dtype=np.float32)
        np.testing.assert_array_equal(display_matrix(TOP_LEFT, BOTTOM_LEFT, TOP_RIGHT), expected)

    def test_display_matrix_maps_centre_to_origin_and_corners_to_half_extents(self):
        import numpy as np
        from tobii_linux.pose_geometry import display_center, display_matrix
        matrix = display_matrix(TOP_LEFT, BOTTOM_LEFT, TOP_RIGHT)
        def apply(point):
            return (np.append(np.asarray(point, dtype=np.float32), 1) @ matrix)[:3]
        np.testing.assert_allclose(apply(display_center(TOP_LEFT, BOTTOM_LEFT, TOP_RIGHT)), 0, atol=1e-4)
        width = TOP_RIGHT[0] - TOP_LEFT[0]
        height = math.dist(TOP_LEFT, BOTTOM_LEFT)
        np.testing.assert_allclose(apply(TOP_RIGHT), [width / 2, height / 2, 0], atol=1e-3)
        np.testing.assert_allclose(apply(BOTTOM_LEFT), [-width / 2, -height / 2, 0], atol=1e-3)

    def test_normal_points_from_screen_towards_viewer(self):
        import numpy as np
        from tobii_linux.pose_geometry import display_normal
        normal = display_normal((-250, 300, 0), (-250, 10, 0), (250, 300, 0))
        np.testing.assert_array_equal(normal, [0, 0, 1])

    def test_camera_extrinsic_is_xyz_rotation_with_half_turn_about_z(self):
        import numpy as np
        from tobii_linux.pose_geometry import camera_extrinsic
        (tx, ty, tz), (a, b, c) = CAMERA
        c += math.pi
        rx = np.array([[1, 0, 0], [0, math.cos(a), -math.sin(a)], [0, math.sin(a), math.cos(a)]])
        ry = np.array([[math.cos(b), 0, math.sin(b)], [0, 1, 0], [-math.sin(b), 0, math.cos(b)]])
        rz = np.array([[math.cos(c), -math.sin(c), 0], [math.sin(c), math.cos(c), 0], [0, 0, 1]])
        matrix = camera_extrinsic(*CAMERA)
        np.testing.assert_allclose(matrix[:3, :3], (rx @ ry @ rz).T, atol=1e-6)
        np.testing.assert_array_equal(matrix[3], [tx, ty, tz, 1])
        np.testing.assert_array_equal(matrix[:3, 3], 0)

    def test_geometry_config_constants(self):
        import numpy as np
        from tobii_linux.pose_geometry import geometry_config
        config3, config4 = geometry_config()
        np.testing.assert_allclose(config3, np.diag([1, -1, -1]), atol=1e-7)
        np.testing.assert_allclose(config4, np.diag([-1, -1, 1, 1]), atol=1e-7)
        self.assertEqual(float(config3[1, 2]), float(np.float32(math.sin(np.float32(math.pi)))))

    def test_stage_matches_emulated_original(self):
        import numpy as np
        from tobii_linux.pose_geometry import camera_extrinsic, display_matrix, geometry_stage
        cases = [
            ((0.0, 0.0, 0.0), (0.0, 0.0, 600.0),
             (-0.048820629715919495, 0.10011979937553406, -0.2049022614955902),
             (68.0201416015625, -164.39093017578125, 614.24755859375)),
            ((0.15, 2.941592653589793, 0.05), (-40.0, 30.0, 600.0),
             (-0.05895845964550972, 3.016695261001587, 0.14097116887569427),
             (34.943626403808594, -126.92433166503906, 615.73046875)),
            ((0.12, -3.0, 0.2), (-120.0, 80.0, 900.0),
             (-0.09773179888725281, -2.9278714656829834, 0.010633852332830429),
             (-3.23614501953125, -47.12995910644531, 917.5170288085938)),
        ]
        camera = camera_extrinsic(*CAMERA)
        display = display_matrix(TOP_LEFT, BOTTOM_LEFT, TOP_RIGHT)
        for angles, position, rotation, translated in cases:
            got_rotation, got_position = geometry_stage(angles, position, camera, display)
            np.testing.assert_allclose(got_rotation, rotation, atol=1e-6)
            np.testing.assert_array_equal(got_position, np.asarray(translated, dtype=np.float32))

    def test_aligned_inputs_only_negate_first_and_third_angles(self):
        import numpy as np
        from tobii_linux.pose_geometry import camera_extrinsic, display_matrix, geometry_stage
        camera = camera_extrinsic((0, 0, 0), (0, 0, 0))
        display = display_matrix((-250, 300, 0), (-250, 10, 0), (250, 300, 0))
        rotation, position = geometry_stage((0.2, -0.1, 0.05), (10, 20, 600), camera, display)
        np.testing.assert_allclose(rotation, [-0.2, -0.1, -0.05], atol=1e-6)
        np.testing.assert_allclose(position, [10, 20 - 155, 600], atol=1e-3)

    def test_rejects_bad_shapes(self):
        from tobii_linux.pose_geometry import camera_extrinsic, display_matrix, transform_position
        with self.assertRaises(ValueError):
            display_matrix((0, 0), (0, 1, 0), (1, 0, 0))
        with self.assertRaises(ValueError):
            transform_position((0, 0, 1), camera_extrinsic((0, 0, 0), (0, 0, 0))[:3, :3],
                               display_matrix((0, 1, 0), (0, 0, 0), (1, 1, 0)))


if __name__ == '__main__':
    unittest.main()
