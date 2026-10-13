import importlib.util
from pathlib import Path
import unittest

SUPPORT = Path(__file__).resolve().parents[1] / 'models/tobii-et5/support'


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class PoseModelTests(unittest.TestCase):
    def test_asymmetric_bounds_encode_physical_zero(self):
        import numpy as np
        from tobii_linux.pose_model import coefficient_latents
        from tobii_linux.pose_projection import bounded_coefficients
        bounds = [[-.2, .7], [-2, 1]]
        latent = coefficient_latents([0, 0], bounds)
        self.assertFalse(np.any(latent == 0))
        decoded, _ = bounded_coefficients(latent, bounds)
        np.testing.assert_allclose(decoded, [0, 0], atol=1e-7)

    def test_endpoint_is_not_a_finite_initial_state(self):
        from tobii_linux.pose_model import coefficient_latents
        with self.assertRaises(ValueError):
            coefficient_latents([1], [[-1, 1]])

    @unittest.skipUnless(SUPPORT.exists(), 'Private installed model assets are absent')
    def test_installed_model_can_fit_synthetic_pose(self):
        import numpy as np
        from tobii_linux.pose_model import load_pose_model
        from tobii_linux.pose_projection import model_jacobian
        from tobii_linux.pose_fit import prepare_observations
        model = load_pose_model(SUPPORT)
        start = model.initial_state([0, np.pi, 0, 0, 0, .65])
        self.assertEqual(start.shape, (69,))
        self.assertEqual(model.vertices.shape, (57, 3))
        self.assertFalse(np.any(model.shape_basis[:, :, ~model.shape_mask]))
        self.assertFalse(np.any(model.animation_basis[:, :, ~model.animation_mask]))
        def project(state):
            return model_jacobian(model.vertices, state[:3], state[3:6], model.focal_factor, 1,
                model.shape_basis, state[6:46], model.shape_ranges,
                model.animation_basis, state[46:], model.animation_ranges,
                model.shape_mask, model.animation_mask)[1]
        target = start.copy()
        target[:6] += np.array([.025, -.03, .02, .008, -.005, -.02], dtype=np.float32)
        observed = project(target)
        _, weights = prepare_observations(np.full((57, 2), .5), model.landmark_ids, 640, 480)
        updated = model.step(observed, start, weights)
        self.assertLess(float(np.linalg.norm(project(updated)-observed)),
                        float(np.linalg.norm(project(start)-observed)) * .1)

    def test_float_inverse_handles_pivot_swap_and_singularity(self):
        import numpy as np
        from tobii_linux.pose_linalg import inverse_float32
        permutation = np.roll(np.eye(6, dtype=np.float32), 2, axis=0)
        np.testing.assert_array_equal(inverse_float32(permutation), permutation.T)
        np.testing.assert_array_equal(inverse_float32(np.ones((6,6))), np.zeros((6,6)))

    def test_bounds_reject_nan_and_clear_invalid_coefficients(self):
        import numpy as np
        from tobii_linux.pose_policy import check_pose_bounds
        invalid, shape, animation = check_pose_bounds([0, np.nan, 0, 0, 0, .65],
            [np.nan, .2], [np.inf, -.1], [[-2,2],[1,5],[-3.2,3.2]], [[-4,4],[-3,3],[0,11]])
        self.assertTrue(invalid)
        np.testing.assert_array_equal(shape, np.array([0,.2], dtype=np.float32))
        np.testing.assert_array_equal(animation, np.array([0,-.1], dtype=np.float32))

    @unittest.skipUnless(SUPPORT.exists(), 'Private installed model assets are absent')
    def test_bounds_failure_retries_once_and_reports_failure(self):
        import numpy as np
        from dataclasses import replace
        from tobii_linux.pose_model import load_pose_model
        from tobii_linux.pose_projection import project_points
        from tobii_linux.pose_policy import fit_landmarks
        model = load_pose_model(SUPPORT)
        # Use impossible pitch limits so even a good geometric fit stays invalid.
        limits = model.rotation_limits.copy()
        limits[0] = [10,11]
        model = replace(model, rotation_limits=limits)
        projected, _ = project_points(model.vertices, [0,np.pi,0], [0,0,.65], model.focal_factor)
        points = (1 - projected / np.array([640/480,1], dtype=np.float32)) * .5
        fitted = fit_landmarks(model, points, 640, 480, model.initial_state([2,np.pi,0,0,0,.65]), 0)
        self.assertEqual(fitted['attempts'], 2)
        self.assertEqual(fitted['updates'], 3)
        self.assertFalse(fitted['valid'])

    @unittest.skipUnless(SUPPORT.exists(), 'Private installed model assets are absent')
    def test_shape_mode_preserves_inactive_physical_animation(self):
        import numpy as np
        from tobii_linux.pose_model import load_pose_model
        from tobii_linux.pose_fit import mode_step
        from tobii_linux.pose_projection import project_points
        model = load_pose_model(SUPPORT)
        pose = np.array([0,np.pi,0,0,0,.65],dtype=np.float32)
        observed, _ = project_points(model.vertices,pose[:3],pose[3:],model.focal_factor)
        animation = np.linspace(-.02,.02,23,dtype=np.float32)
        result = mode_step(model,observed,pose,np.zeros(40),animation,3)
        self.assertEqual(result['active_state'].shape,(46,))
        np.testing.assert_array_equal(result['animation_coefficients'],animation)

    @unittest.skipUnless(SUPPORT.exists(), 'Private installed model assets are absent')
    def test_selection_is_numeric_and_excludes_unmapped_aliases(self):
        import numpy as np
        from tobii_linux.pose_model import load_pose_model, select_pose_landmarks
        model = load_pose_model(SUPPORT)
        selected, order = select_pose_landmarks(model, (*model.landmark_ids,(13,17),(2,1)))
        self.assertEqual(selected.landmark_ids, tuple(sorted(model.landmark_ids)))
        self.assertEqual(len(selected.landmark_ids), 57)
        np.testing.assert_array_equal(selected.vertices, model.vertices[order])

    @unittest.skipUnless(SUPPORT.exists(), 'Private installed model assets are absent')
    def test_staged_caller_fits_a_neutral_synthetic_face(self):
        import numpy as np
        from tobii_linux.pose_model import load_pose_model
        from tobii_linux.pose_projection import project_points
        from tobii_linux.pose_policy import fit_staged_landmarks
        model = load_pose_model(SUPPORT)
        projected, _ = project_points(model.vertices, [0,np.pi,0], [0,0,.65], model.focal_factor)
        points = (1 - projected / np.array([640/480,1],dtype=np.float32)) * .5
        result = fit_staged_landmarks(model, points, 640, 480)
        self.assertTrue(result['valid'])
        self.assertEqual([(s['mode'],s['updates']) for s in result['stages']], [(3,2),(5,1)])
        self.assertLess(abs(float(result['rigid_state'][1])-np.pi), .02)
