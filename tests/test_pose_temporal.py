import importlib.util
from pathlib import Path
import unittest

SUPPORT = Path(__file__).resolve().parents[1] / 'models/tobii-et5/support'


def synthetic_frame(model, angles, translation, noise=0, seed=0):
    import numpy as np
    from tobii_linux.pose_projection import model_jacobian
    target = model.initial_state([*angles, *translation])
    _, projected = model_jacobian(model.vertices, target[:3], target[3:6], model.focal_factor, 1,
        model.shape_basis, target[6:46], model.shape_ranges, model.animation_basis, target[46:], model.animation_ranges)
    points = (np.float32(1) - projected / np.array([640 / 480, 1], dtype=np.float32)) * np.float32(.5)
    if noise:
        points = points + np.random.default_rng(seed).normal(0, noise, points.shape).astype(np.float32)
    return points.astype(np.float32)


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class TemporalMeasureTests(unittest.TestCase):
    def test_rectangle_uses_largest_eye_mouth_distance(self):
        from tobii_linux.pose_temporal import eye_face_rectangle
        points = {(3, 11): (.4, .4), (3, 7): (.3, .4), (3, 12): (.6, .4), (3, 8): (.7, .4),
                  (9, 4): (.5, .6), (9, 5): (.5, .6), (8, 1): (.5, .8)}
        self.assertEqual(eye_face_rectangle(list(points.values()), list(points), 100, 100), (12, 18, 77, 77))
        del points[(8, 1)]
        self.assertIsNone(eye_face_rectangle(list(points.values()), list(points), 100, 100))

    def test_reprojection_error_skips_groups_13_and_15(self):
        import numpy as np
        from types import SimpleNamespace
        from tobii_linux.pose_temporal import reprojection_error
        model = SimpleNamespace(landmark_ids=((3, 1), (13, 1), (15, 1)))
        flipped = np.array([[.25, .75], [.1, .1], [.9, .9]], dtype=np.float32)
        # Fitting-plane point that maps back to (0.25, 0.75) at 400x400.
        projected = np.array([[.5, .5], [0, 0], [0, 0]], dtype=np.float32)
        self.assertEqual(float(reprojection_error(model, projected, flipped, 400, 400, 50)), 0.0)
        projected[0, 0] = 0
        self.assertAlmostEqual(float(reprojection_error(model, projected, flipped, 400, 400, 50)), 2.0, places=6)
        self.assertTrue(np.isnan(reprojection_error(SimpleNamespace(landmark_ids=((13, 1),)),
                                                    projected[:1], flipped[:1], 400, 400, 50)))

    def test_frontal_test_and_facing_angle(self):
        import math
        from tobii_linux.pose_temporal import is_near_frontal, facing_degrees, FLOAT_PI
        self.assertTrue(is_near_frontal([.15, -.15]))
        self.assertFalse(is_near_frontal([0, .15707963705062866]))
        self.assertTrue(is_near_frontal([math.nan, 0]))
        self.assertAlmostEqual(float(facing_degrees([0, FLOAT_PI + .5, 0, 0, 0, .65])), 28.648, places=2)
        self.assertAlmostEqual(float(facing_degrees([0, FLOAT_PI, 0, .65, 0, .65])), -45.0, places=3)


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
@unittest.skipUnless(SUPPORT.exists(), 'Private installed model assets are absent')
class TemporalPoseFitterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from tobii_linux.pose_model import load_pose_model
        cls.model = load_pose_model(SUPPORT)

    def modes(self, result):
        return [(fit['mode'], fit['valid']) for fit in result['trace']]

    def test_first_frame_steady_frame_and_frontal_refit(self):
        import numpy as np
        from tobii_linux.pose_temporal import TemporalPoseFitter
        fitter = TemporalPoseFitter(self.model)
        first = fitter.process(synthetic_frame(self.model, [.02, np.pi + .3, .01], [.01, 0, .65]), 640, 480)
        self.assertEqual(self.modes(first), [(3, True), (5, True)])
        self.assertTrue(first['valid'] and first['initialised'] and not first['frontal'])
        np.testing.assert_allclose(first['rotation'], [.02, .3, .01], atol=.01)
        steady = fitter.process(synthetic_frame(self.model, [.02, np.pi + .32, .01], [.012, 0, .65]), 640, 480)
        self.assertEqual(self.modes(steady), [(5, True)])
        fitter.process(synthetic_frame(self.model, [.02, np.pi + .18, .01], [.01, 0, .65]), 640, 480)
        frontal = fitter.process(synthetic_frame(self.model, [0, np.pi + .04, 0], [.01, 0, .65]), 640, 480)
        self.assertEqual(self.modes(frontal), [(5, True), (3, True), (5, True)])
        self.assertTrue(frontal['frontal'])

    def test_error_rejection_reinitialises_near_frontal_pose(self):
        import numpy as np
        from tobii_linux.pose_temporal import TemporalPoseFitter
        fitter = TemporalPoseFitter(self.model)
        fitter.process(synthetic_frame(self.model, [0, np.pi + .2, 0], [.01, 0, .65]), 640, 480)
        noisy = fitter.process(synthetic_frame(self.model, [0, np.pi + .2, 0], [.01, 0, .65], .04, 4), 640, 480)
        self.assertGreater(noisy['trace'][0]['error'], .1)
        self.assertEqual(self.modes(noisy), [(5, True), (3, True), (5, True)])
        self.assertTrue(noisy['valid'])

    def test_failure_resets_output_pose_and_keeps_fitting_object(self):
        import numpy as np
        from dataclasses import replace
        from tobii_linux.pose_temporal import TemporalPoseFitter, FLOAT_PI
        model = replace(self.model, translation_limits=np.array([[-4, 4], [-3, 3], [.3, 1]], dtype=np.float32))
        fitter = TemporalPoseFitter(model)
        fitter.process(synthetic_frame(model, [.02, np.pi + .3, .01], [.01, 0, .65]), 640, 480)
        failed = fitter.process(synthetic_frame(model, [.02, np.pi + .3, .01], [.01, 0, 1.6]), 640, 480)
        self.assertFalse(failed['valid'] or failed['initialised'])
        np.testing.assert_array_equal(failed['rigid_state'], np.array([0, FLOAT_PI, 0, 0, 0, 0], dtype=np.float32))
        self.assertEqual(fitter.fit_mode, 3)
        recovered = fitter.process(synthetic_frame(model, [.02, np.pi + .28, .01], [.01, 0, .65]), 640, 480)
        self.assertTrue(recovered['valid'])
        self.assertEqual(recovered['trace'][0]['attempts'], 2)

    def test_reset_rebuilds_fitting_object(self):
        import numpy as np
        from tobii_linux.pose_temporal import TemporalPoseFitter
        fitter = TemporalPoseFitter(self.model)
        fitter.process(synthetic_frame(self.model, [0, np.pi + .2, 0], [0, 0, .65]), 640, 480)
        self.assertEqual(fitter.fit_mode, 5)
        fitter.reset()
        self.assertIsNone(fitter.fit_mode)
        self.assertFalse(fitter.initialised)
        self.assertFalse(np.any(fitter.shape))


if __name__ == '__main__':
    unittest.main()
