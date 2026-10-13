import importlib.util
import math
import unittest


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class PoseOutputTests(unittest.TestCase):
    def test_matrix_is_transposed_yxz_composition(self):
        import numpy as np
        from tobii_linux.pose_output import euler_matrix
        a, b, c = 0.3, 0.5, 0.7
        rx = np.array([[1, 0, 0], [0, math.cos(a), -math.sin(a)], [0, math.sin(a), math.cos(a)]])
        ry = np.array([[math.cos(b), 0, math.sin(b)], [0, 1, 0], [-math.sin(b), 0, math.cos(b)]])
        rz = np.array([[math.cos(c), -math.sin(c), 0], [math.sin(c), math.cos(c), 0], [0, 0, 1]])
        np.testing.assert_allclose(euler_matrix((a, b, c)), (ry @ rx @ rz).T, atol=1e-6)

    def test_round_trip_recovers_angles(self):
        import numpy as np
        from tobii_linux.pose_output import euler_matrix, matrix_euler
        for angles in ((0.15, -0.15, 0.005), (-0.4, 0.7, -0.2), (0.2, -2.9, 1.1)):
            np.testing.assert_allclose(matrix_euler(euler_matrix(angles)), angles, atol=2e-6)

    def test_wrap_shifts_into_pi_interval_and_keeps_nan(self):
        from tobii_linux.pose_output import wrap_angle
        self.assertAlmostEqual(float(wrap_angle(4.0)), 4.0 - 2 * math.pi, places=5)
        self.assertAlmostEqual(float(wrap_angle(-7.0)), -7.0 + 2 * math.pi, places=5)
        self.assertTrue(math.isnan(float(wrap_angle(float('nan')))))

    def test_adapter_scales_translation_to_millimetres(self):
        import numpy as np
        from tobii_linux.pose_output import adapter_head_pose
        matrix, position = adapter_head_pose((0, 0, 0), (0.01, -0.02, 0.5))
        np.testing.assert_allclose(position, [10, -20, 500], rtol=1e-6)
        np.testing.assert_array_equal(matrix, np.eye(3, dtype=np.float32))

    def test_adapter_rejects_bad_translation(self):
        from tobii_linux.pose_output import adapter_head_pose
        with self.assertRaises(ValueError):
            adapter_head_pose((0, 0, 0), (1, 2))

    def test_filter_passes_first_sample_and_holds_steady_input(self):
        import numpy as np
        from tobii_linux.pose_output import HeadPoseFilter
        stage = HeadPoseFilter()
        for step in range(5):
            rotation, position = stage((0.1, -0.2, 0.3), (-50, -15, 530), 1_000_000 + 8333 * step)
            np.testing.assert_allclose(rotation, [0.1, -0.2, 0.3], rtol=1e-6)
            np.testing.assert_allclose(position, [-50, -15, 530], rtol=1e-6)

    def test_filter_lags_a_position_step(self):
        from tobii_linux.pose_output import OneEuroVectorFilter
        smoother = OneEuroVectorFilter()
        smoother((0, 0, 500), 1_000_000)
        moved = smoother((0, 0, 600), 1_008_333)
        self.assertTrue(500 < float(moved[2]) < 600)

    def test_repeated_timestamp_uses_fallback_rate(self):
        from tobii_linux.pose_output import OneEuroFilter
        smoother = OneEuroFilter()
        smoother(0.0, 5)
        self.assertTrue(0 < float(smoother(1.0, 5)) < 1)

    def test_vector_filter_rejects_wrong_shape(self):
        from tobii_linux.pose_output import OneEuroVectorFilter
        with self.assertRaises(ValueError):
            OneEuroVectorFilter()((1, 2), 0)


if __name__ == '__main__':
    unittest.main()
