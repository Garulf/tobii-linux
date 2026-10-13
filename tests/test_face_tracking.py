import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
MODELS = ROOT/'models/tobii-et5'


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class FaceTrackingGeometryTests(unittest.TestCase):
    def test_region_expands_around_centre_and_clamps(self):
        from tobii_linux.face_tracking import expanded_region
        self.assertEqual(expanded_region((120, 80, 60, 60), 320, 240), (75, 35, 150, 150))
        self.assertEqual(expanded_region((10, 5, 70, 70), 320, 240), (0, 0, 132, 127))

    def test_working_view_scales_face_to_target_width(self):
        import numpy as np
        from tobii_linux.face_tracking import working_view, working_face_in_frame
        frame = np.zeros((240, 320), np.uint8)
        view = working_view(frame, (120, 80, 60, 60))
        self.assertEqual(view.image.shape, (240, 240))
        self.assertEqual(view.face, (72, 72, 96, 96))
        self.assertEqual(working_face_in_frame(view), (120, 80, 60, 60))

    def test_points_map_back_to_frame(self):
        import numpy as np
        from tobii_linux.face_tracking import WorkingView, to_frame_points
        view = WorkingView(None, (75, 35, 150, 150), 1.6, (72, 72, 96, 96))
        mapped = to_frame_points([[0.0, 0.0], [1.0, 1.0]], view, 320, 240)
        np.testing.assert_allclose(mapped, [[75/320, 35/240], [225/320, 185/240]], rtol=1e-6)

    def test_landmark_box_is_square_around_points(self):
        from tobii_linux.face_tracking import landmark_box
        self.assertEqual(landmark_box([(0.25, 0.25), (0.5, 0.75)], 200, 100), (50, 25, 50, 50))

    def test_upright_face_has_zero_roll(self):
        from tobii_linux.face_tracking import temporal_roll
        ids = [(4, 1), (4, 2), (9, 4), (9, 5)]
        points = [(0.4, 0.3), (0.6, 0.3), (0.45, 0.6), (0.55, 0.6)]
        self.assertAlmostEqual(temporal_roll(points, ids, 100, 100), 0.0, places=4)

    def test_core_records_carry_the_four_aliases(self):
        from tobii_linux.face_tracking import frame_records
        ids = [(15, 17), (9, 3), (14, 23)]
        records = frame_records([(0.1, 0.2), (0.3, 0.4), (0.5, 0.6)], [0.9, 0.8, 0.7], ids)
        self.assertEqual((records[(2, 1)].x, records[(13, 17)].y), (0.1, 0.2))
        self.assertEqual(records[(14, 21)].quality, 0.8)
        self.assertEqual(records[(9, 12)].x, 0.5)
        self.assertTrue(all(record.present for record in records.values()))


@unittest.skipUnless(importlib.util.find_spec('openvino') and (MODELS/'ff').exists(),
                     'Optional OpenVINO or model assets are not installed')
class FaceTrackerTests(unittest.TestCase):
    def test_blank_frame_stays_searching(self):
        import numpy as np
        from tobii_linux.face_tracking import FaceTracker, SEARCHING
        tracker = FaceTracker(MODELS)
        result = tracker.process(np.zeros((480, 640), np.uint8))
        self.assertEqual(tracker.state, SEARCHING)
        self.assertIsNone(result.fit)


if __name__ == '__main__':
    unittest.main()
