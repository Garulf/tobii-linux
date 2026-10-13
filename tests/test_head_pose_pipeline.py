import importlib.util
import math
import struct
import unittest

ALBEDO = ((-296.62, 326.0, 111.82), (300.38, 326.0, 111.82), (-296.62, 10.27, -3.10))


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class HeadPosePipelineTests(unittest.TestCase):
    def output(self):
        from tobii_linux.head_pose_pipeline import DisplayArea, HeadPoseOutput
        return HeadPoseOutput((0, 0, 0), (0, 0, 0), DisplayArea(*ALBEDO))

    def test_missing_fit_gives_invalid_zero_sample(self):
        sample = self.output()(None, 42)
        timestamp, validity, *rest = struct.unpack('<qi3fiii3f', sample.encode())
        self.assertEqual((timestamp, validity), (42, 0))
        self.assertEqual(rest[3:6], [0, 0, 0])

    def test_sample_layout_is_48_bytes_with_set_flags(self):
        sample = self.output()((0, math.pi, 0, 0, 0, 0.6), 1000)
        values = struct.unpack('<qi3fiii3f', sample.encode())
        self.assertEqual(len(sample.encode()), 48)
        self.assertEqual(values[1], 1)
        self.assertEqual(values[5:8], (1, 1, 1))

    def test_frontal_head_reports_display_tilt_and_distance(self):
        sample = self.output()((0, math.pi, 0, 0, 0, 0.6), 1000)
        self.assertAlmostEqual(abs(sample.rotation_rad[0]), math.radians(20), places=2)
        self.assertGreater(sample.position_mm[2], 500)


if __name__ == '__main__':
    unittest.main()
