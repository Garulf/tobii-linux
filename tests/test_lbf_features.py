import struct
import unittest
from tobii_linux.lbf_features import forest_leaves


class ForestFeaturesTests(unittest.TestCase):
    def test_pixel_difference_strict_branch_and_axis_mirror(self):
        tree = (0, 0, 1, struct.pack('<bbbb', 0, -1, 0, 1) + bytes(4))
        forest = (0, 0, [tree])
        pixels = bytes([9, 5, 1])
        self.assertEqual(forest_leaves(forest, pixels, 1, 3, 3, 0, 1, 256), (1,))
        self.assertEqual(forest_leaves(forest, pixels, 1, 3, 3, 0, 1, 256, -1), (0,))
        self.assertEqual(forest_leaves(forest, pixels, 1, 3, 3, 0, 1, 0), (0,))

    def test_clamps_negative_offsets_and_rejects_short_buffer(self):
        forest = (0, 0, [(0, 0, 1, struct.pack('<bbbb', -128, -128, 127, 127) + bytes(4))])
        self.assertEqual(forest_leaves(forest, bytes([0, 1, 2, 3]), 2, 2, 2, 0, 0, 32), (0,))
        with self.assertRaises(ValueError):
            forest_leaves(forest, bytes(3), 2, 2, 2, 0, 0, 32)
