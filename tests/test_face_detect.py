import importlib.util
import unittest


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class FaceDetectTests(unittest.TestCase):
    def test_anchor_count_matches_network_output(self):
        from tobii_linux.face_detect import anchors
        priors = anchors()
        self.assertEqual(priors.shape, (2820, 4))
        self.assertEqual(priors.shape[0] * 9, 25380)

    def test_letterbox_layouts(self):
        from tobii_linux.face_detect import letterbox_layout
        def placement(width, height):
            layout = letterbox_layout(width, height)
            return layout.offset_x, layout.offset_y, layout.width, layout.height
        self.assertEqual(placement(640, 480), (0, 0, 256, 192))
        self.assertEqual(placement(1280, 720), (0, 24, 256, 144))
        self.assertEqual(placement(480, 640), (56, 0, 144, 192))

    def test_resize_identity_and_constant(self):
        import numpy as np
        from tobii_linux.face_detect import resize_bilinear
        image = np.arange(48, dtype=np.uint8).reshape(6, 8)
        np.testing.assert_array_equal(resize_bilinear(image, 8, 6), image)
        np.testing.assert_array_equal(resize_bilinear(np.full((5, 7), 93, np.uint8), 13, 11), 93)

    def test_suppression_keeps_highest_of_overlapping_pair(self):
        import numpy as np
        from tobii_linux.face_detect import suppress
        f = np.float32
        boxes = [(f(10), f(10), f(50), f(50), f(0.9), ()), (f(12), f(12), f(52), f(52), f(0.8), ()),
                 (f(100), f(100), f(140), f(140), f(0.7), ())]
        self.assertEqual([b[4] for b in suppress(boxes)], [f(0.9), f(0.7)])

    def test_single_strong_anchor_becomes_square_frame_box(self):
        import numpy as np
        from tobii_linux.face_detect import anchors, detect_faces
        count = len(anchors())
        output = np.zeros(count * 9, dtype=np.float32)
        anchor = 2304 + (5*16 + 8)*2
        output[4*count + 2*anchor + 1] = 0.95
        found = detect_faces(np.zeros((480, 640), np.uint8), lambda image: output)
        self.assertEqual(len(found), 1)
        face = found[0]
        self.assertAlmostEqual(face.x2 - face.x1, face.y2 - face.y1)
        x, y, side, _ = face.rectangle()
        self.assertGreater(side, 0)


if __name__ == '__main__':
    unittest.main()
