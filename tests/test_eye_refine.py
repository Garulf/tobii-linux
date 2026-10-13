import importlib.util
from pathlib import Path
import unittest

SUPPORT = Path(__file__).resolve().parents[1] / 'models/tobii-et5/support'


def shifted(calls):
    def refine(name, xs, ys, scale, mirrored, roi):
        calls.append((name, mirrored, float(scale), roi))
        return [x + 4 for x in xs], [y + 2 for y in ys]
    return refine


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class EyeRefineTests(unittest.TestCase):
    def test_model_selection_thresholds(self):
        import math
        from tobii_linux.eye_refine import select_lbf_model
        self.assertEqual(select_lbf_model(0), ('fa', False))
        self.assertEqual(select_lbf_model(15), ('fc', False))
        self.assertEqual(select_lbf_model(15.001), ('fc', True))
        self.assertEqual(select_lbf_model(-20), ('fc', False))
        self.assertEqual(select_lbf_model(math.nan), ('fa', False))

    def test_roi_guard(self):
        from tobii_linux.eye_refine import roi_inside
        self.assertTrue(roi_inside(240, 200, None))
        self.assertTrue(roi_inside(240, 200, (40, 20, 200, 180)))
        self.assertFalse(roi_inside(240, 200, (41, 20, 200, 180)))
        self.assertFalse(roi_inside(240, 200, (0, -1, 10, 10)))

    def test_mirror_map_pairs_sides(self):
        from tobii_linux.eye_refine import MIRRORED_INDEX, mirrored_index
        self.assertEqual(mirrored_index(3, 7), 12)
        self.assertEqual(mirrored_index(3, 8), 11)
        self.assertEqual(mirrored_index(9, 15), 15)
        self.assertEqual(mirrored_index(5, 1), -1)
        for group, table in MIRRORED_INDEX.items():
            for index, mirrored in enumerate(table, 1):
                if mirrored != -1:
                    self.assertEqual(table[mirrored - 1], index)
        with self.assertRaises(ValueError):
            mirrored_index(3, 15)

    def test_group_10_writes_and_roi_failure(self):
        from tobii_linux.eye_refine import refine_eye_landmarks
        ids = [(3, 7), (10, 1), (10, 2)]
        records = {(3, 7): (.1, .2), (3, 12): (.3, .4), (10, 1): (.5, .5), (10, 2): (.25, .75)}
        calls = []
        valid, written = refine_eye_landmarks(ids, records, 200, 100, 80, 0, shifted(calls))
        self.assertTrue(valid)
        self.assertEqual(calls, [('fa', False, 80.0, None)])
        self.assertEqual(sorted(written), [(10, 1), (10, 2)])
        import numpy as np
        np.testing.assert_array_equal(np.asarray(written[(10, 1)], dtype=np.float32),
                                      np.array([.52, .52, 1000, 1, 1, -1], dtype=np.float32))
        valid, written = refine_eye_landmarks(ids, records, 200, 100, 80, 30, shifted(calls))
        self.assertEqual(calls[-1][:2], ('fc', True))
        self.assertAlmostEqual(float(written[(10, 2)][0]), .27, places=6)
        self.assertAlmostEqual(float(written[(10, 1)][0]), .52, places=6)
        calls.clear()
        self.assertEqual(refine_eye_landmarks(ids, records, 200, 100, 80, 0, shifted(calls), (150, 0, 60, 50)),
                         (False, {}))
        self.assertEqual(calls, [])

    def test_tracker_yaw_is_zero_for_neutral_pose(self):
        from tobii_linux.eye_refine import tracker_yaw_degrees
        from tobii_linux.pose_temporal import FLOAT_PI
        self.assertAlmostEqual(float(tracker_yaw_degrees([0, FLOAT_PI, 0, 0, 0, .65])), 0, places=4)

    @unittest.skipUnless(SUPPORT.exists(), 'Private installed model assets are absent')
    def test_installed_lbf_tables_write_no_records(self):
        from tobii_linux.eye_refine import refine_eye_landmarks
        from tobii_linux.lbf import read_lbf
        for name in ('fa', 'fc'):
            ids = [tuple(p['id']) for p in read_lbf(SUPPORT / f'bdtsdata/NN/{name}.lbf')['points']]
            self.assertNotIn(10, {group for group, _ in ids})
            records = {}
            for group, index in ids:
                records[(group, index)] = (.5, .5)
            valid, written = refine_eye_landmarks(ids, records, 240, 240, 90, 0, shifted([]))
            self.assertTrue(valid)
            self.assertEqual(written, {})


if __name__ == '__main__':
    unittest.main()
