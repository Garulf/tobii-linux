import importlib.util
import unittest


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class CoordinateFilterTests(unittest.TestCase):
    def run_frames(self, landmark_filter, frames, gains, period=30, start=0):
        outputs = []
        for frame, values in enumerate(frames):
            landmark_filter.begin_frame()
            outputs.append(landmark_filter.smooth_coordinates(values, gains, start + period*frame))
        return outputs

    def test_reset_clears_history_and_rewinds_rings(self):
        from tobii_linux.landmark_smoothing import CLEARED_TIME, LandmarkFilter
        landmark_filter = LandmarkFilter(channels=4)
        self.run_frames(landmark_filter, [[0.1, 0.2]]*3, [1.0, 1.0])
        landmark_filter.reset()
        self.assertTrue((landmark_filter.times == CLEARED_TIME).all())
        self.assertTrue((landmark_filter.position == 0).all())
        self.assertFalse(landmark_filter.initialised.any())

    def test_fewer_than_five_samples_pass_through(self):
        import numpy as np
        from tobii_linux.landmark_smoothing import LandmarkFilter
        frames = [[0.1*f, 0.5 + 0.01*f*f] for f in range(4)]
        outputs = self.run_frames(LandmarkFilter(channels=4), frames, [1.0, 1.0])
        for values, output in zip(frames, outputs):
            self.assertEqual([float(v) for v in output], [float(np.float32(v)) for v in values])

    def test_constant_signal_is_unchanged(self):
        from tobii_linux.landmark_smoothing import LandmarkFilter
        outputs = self.run_frames(LandmarkFilter(channels=4), [[0.375, 0.625]]*12, [1.1, 0.9])
        for output in outputs:
            self.assertAlmostEqual(float(output[0]), 0.375, places=6)
            self.assertAlmostEqual(float(output[1]), 0.625, places=6)

    def test_noise_free_ramp_follows_newest_sample(self):
        from tobii_linux.landmark_smoothing import LandmarkFilter
        frames = [[0.2 + 0.01*f] for f in range(12)]
        outputs = self.run_frames(LandmarkFilter(channels=2), frames, [1.0])
        self.assertAlmostEqual(float(outputs[-1][0]), 0.31, places=5)

    def test_noisy_samples_are_pulled_towards_the_fit(self):
        from tobii_linux.landmark_smoothing import LandmarkFilter
        frames = [[0.5 + (0.01 if f % 2 else -0.01)] for f in range(12)]
        outputs = self.run_frames(LandmarkFilter(channels=2), frames, [1.0])
        self.assertLess(abs(float(outputs[-1][0]) - 0.5), 0.01)

    def test_window_drops_samples_older_than_310_ms(self):
        from tobii_linux.landmark_smoothing import LandmarkFilter
        landmark_filter = LandmarkFilter(channels=2)
        self.run_frames(landmark_filter, [[0.1 + 0.01*f] for f in range(8)], [1.0])
        landmark_filter.begin_frame()
        output = landmark_filter.smooth_coordinates([0.9], [1.0], 30*7 + 400)
        self.assertAlmostEqual(float(output[0]), 0.9, places=7)

    def test_negative_gain_skips_value_and_channel(self):
        from tobii_linux.landmark_smoothing import LandmarkFilter
        landmark_filter = LandmarkFilter(channels=4)
        landmark_filter.begin_frame()
        output = landmark_filter.smooth_coordinates([0.1, 0.7, 0.3], [1.0, -1.0, 1.0], 0)
        self.assertEqual(landmark_filter.cursor, 2)
        self.assertAlmostEqual(float(output[1]), 0.7, places=7)

    def test_prediction_rows_for_constant_and_line(self):
        import numpy as np
        from tobii_linux.landmark_smoothing import index_prediction_row
        np.testing.assert_allclose(index_prediction_row(5, 0), [0.2]*5, rtol=1e-6)
        np.testing.assert_allclose(index_prediction_row(5, 1), [-0.2, 0.0, 0.2, 0.4, 0.6], atol=1e-6)


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class ScalarFilterTests(unittest.TestCase):
    def test_first_310_ms_pass_through_then_smooths(self):
        import numpy as np
        from tobii_linux.landmark_smoothing import LandmarkFilter
        landmark_filter = LandmarkFilter(channels=2)
        outputs = []
        for frame in range(16):
            landmark_filter.begin_frame()
            value = 0.8 if frame % 2 else 0.6
            outputs.append(float(landmark_filter.smooth_scalar(value, 30*frame, 0.5, 0.05, 1.0, 0.5)))
        self.assertEqual(outputs[10], float(np.float32(0.6)))
        self.assertTrue(0.6 < outputs[11] < 0.8)
        self.assertTrue(landmark_filter.initialised[0])


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class RecordSmoothingTests(unittest.TestCase):
    def test_gain_scales_with_quality_except_group_15(self):
        from tobii_linux.landmark_smoothing import coordinate_gain
        self.assertAlmostEqual(float(coordinate_gain(4, 0.5, 1.1)), 1.2, places=6)
        self.assertAlmostEqual(float(coordinate_gain(4, -1.0, 1.1)), 1.5, places=6)
        self.assertAlmostEqual(float(coordinate_gain(15, 0.5, 1.2)), 1.2, places=6)

    def test_tracker_table_uses_configured_columns(self):
        from tobii_linux.landmark_smoothing import table_entries
        entries = table_entries((1.1, 0.9, 1.1, 1.1, 1.1, 1.2, 0.8))
        self.assertEqual(len(entries), 13)
        self.assertEqual(entries[0][:3], (4, 1, 6))
        self.assertAlmostEqual(entries[2][3], 0.9, places=6)
        self.assertEqual(entries[-1][:3], (15, 1, 17))

    def test_flip_only_touches_present_records(self):
        from tobii_linux.landmark_smoothing import LandmarkRecord, flip_y
        records = {(3, 1): LandmarkRecord(0.2, 0.25, True, 0.9), (3, 2): LandmarkRecord(0.2, 0.25, False, -1.0)}
        flip_y(records)
        self.assertEqual(records[(3, 1)].y, 0.75)
        self.assertEqual(records[(3, 2)].y, 0.25)

    def test_channels_cover_table_then_present_qualities(self):
        from tobii_linux.landmark_smoothing import LandmarkFilter, LandmarkRecord, smooth_records
        records = {(4, i): LandmarkRecord(0.1*i, 0.5, True, 0.8) for i in range(1, 7)}
        records[(9, 3)] = LandmarkRecord(0.4, 0.4, True, -1.0)
        landmark_filter = LandmarkFilter()
        landmark_filter.begin_frame()
        smooth_records(landmark_filter, records, 0, ((4, 1, 6, 1.1), (14, 1, 2, 1.1)))
        self.assertEqual(landmark_filter.cursor, 2*8 + 6)
        self.assertFalse(records[(14, 1)].present)


if __name__ == '__main__':
    unittest.main()
