import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
MODELS = ROOT/'models/tobii-et5'
RECORDED = ROOT/'evidence/private/e2e-person/linux-frames.jsonl'
AREA = ((-296.62, 326.0, 111.82), (300.38, 326.0, 111.82), (-296.62, 10.27, -3.1))
NEUTRAL = (0.0, 3.1415927410125732, 0.0, 0.0, 0.0, 0.6)


def neutral_fit(model, rigid=NEUTRAL):
    import numpy as np
    return {'rigid_state': np.asarray(rigid, np.float32),
            'shape_coefficients': np.zeros(len(model.shape_ranges), np.float32),
            'animation_coefficients': np.zeros(len(model.animation_ranges), np.float32)}


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
@unittest.skipUnless((MODELS/'support').is_dir(), 'models/tobii-et5 is absent')
class FaceWireframeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from tobii_linux.face_wireframe import FaceWireframe
        from tobii_linux.pose_model import load_pose_model
        cls.model = load_pose_model(MODELS/'support')
        cls.wireframe = FaceWireframe.load(MODELS/'support', cls.model)

    def test_mesh_matches_the_fitted_landmark_model(self):
        import numpy as np
        wireframe = self.wireframe
        self.assertEqual(wireframe.vertices.shape, (357, 3))
        indices = list(wireframe.landmark_indices)
        np.testing.assert_array_equal(wireframe.vertices[indices], self.model.vertices)
        np.testing.assert_array_equal(wireframe.shape_basis[indices], self.model.shape_basis)
        np.testing.assert_array_equal(wireframe.animation_basis[indices], self.model.animation_basis)
        self.assertEqual(len(set(wireframe.edges)), len(wireframe.edges))
        self.assertTrue(all(a < b for a, b in wireframe.edges))

    def test_image_points_invert_the_fitting_plane(self):
        import numpy as np
        from tobii_linux.pose_projection import project_points
        fit = neutral_fit(self.model)
        points = self.wireframe.image_points(fit, 280, 280)
        plane, _ = project_points(self.model.vertices, fit['rigid_state'][:3], fit['rigid_state'][3:],
                                  self.model.focal_factor, 1)
        landmarks = points[list(self.wireframe.landmark_indices)]
        np.testing.assert_allclose(np.float32(1) - (landmarks / 280 * 2), plane, atol=1e-6)
        centre = points.mean(axis=0)
        self.assertLess(np.abs(centre - 140).max(), 30)

    def test_display_points_follow_the_filtered_pose(self):
        import numpy as np
        from tobii_linux.face_wireframe import unfiltered_pose
        from tobii_linux.head_pose_pipeline import DisplayArea, HeadPoseOutput, HeadPoseSample
        output = HeadPoseOutput((0.0, 0.0, 4.14), (0.0, 0.0, 0.0), DisplayArea(*AREA))
        turned = (0.2, 3.0, 0.05, 0.02, -0.01, 0.62)
        rotation, position = unfiltered_pose(turned, output)
        sample = HeadPoseSample(0, True, tuple(map(float, position)), tuple(map(float, rotation)))
        moved = self.wireframe.display_points(neutral_fit(self.model), output, sample)
        placed = self.wireframe.display_points(neutral_fit(self.model, turned), output, sample)
        np.testing.assert_allclose(moved, placed, atol=0.01)
        self.assertLess(np.abs(placed.mean(axis=0) - position).max(), 120)


@unittest.skipUnless(importlib.util.find_spec('numpy') and importlib.util.find_spec('openvino') and RECORDED.exists(),
                     'Needs NumPy, OpenVINO and the recorded frames in evidence/private/e2e-person')
class RecordedWireframeTests(unittest.TestCase):
    def test_projected_mesh_lands_on_the_recorded_face(self):
        import numpy as np
        from tobii_linux.demo.sources import recorded_frames
        from tobii_linux.face_tracking import FaceTracker
        from tobii_linux.face_wireframe import FaceWireframe
        _, frames = recorded_frames(RECORDED)
        tracker = FaceTracker(MODELS)
        wireframe = FaceWireframe.load(MODELS/'support', tracker.pose_model)
        for timestamp, width, height, path in frames[:3]:
            pixels = np.frombuffer(path.read_bytes(), np.uint8).reshape(height, width)
            tracked = tracker.process(pixels, timestamp // 1000)
        self.assertIsNotNone(tracked.fit)
        points = wireframe.image_points(tracked.fit, width, height)
        x, y, w, h = tracked.face
        inside = ((points[:, 0] >= x - w / 4) & (points[:, 0] <= x + w * 5 / 4)
                  & (points[:, 1] >= y - h / 4) & (points[:, 1] <= y + h * 5 / 4))
        self.assertGreater(inside.mean(), 0.95)
        landmarks = points[list(wireframe.landmark_indices)]
        error = np.linalg.norm(landmarks - tracked.fitted_points * (width, height), axis=1)
        self.assertLess(error.mean(), 2.5)


if __name__ == '__main__':
    unittest.main()
