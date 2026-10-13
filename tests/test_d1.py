import unittest
import importlib.util
from tobii_linux.d1 import decode_output, normalize_image_points


class D1OutputTests(unittest.TestCase):
    def test_offsets_are_relative_to_reference_not_absolute(self):
        reference=[(40.0,50.0)]*57
        values=[0.0]*171
        values[0:2]=[0.5,-0.5]
        values[114:]=[0.75]*57
        result=decode_output(values,reference)
        self.assertEqual(result.crop_points[0],(64.0,26.0))
        self.assertEqual(result.crop_points[1],reference[1])
        self.assertEqual(result.quality,(0.75,)*57)

    def test_rejects_wrong_size_and_nonfinite(self):
        for values in ([0.0]*170,[float('nan')]*171):
            with self.assertRaises(ValueError):decode_output(values,[(0,0)]*57)

    def test_normalized_axes_use_image_width_and_height(self):
        self.assertEqual(normalize_image_points([(320,120)],640,480),((0.5,0.25),))


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class D1GeometryTests(unittest.TestCase):
    def test_similarity_recovers_known_rotation_scale_translation(self):
        import numpy as np
        from tobii_linux.d1 import fit_similarity, inverse_transform_points
        source=np.array([[0,0],[1,0],[1,2],[3,-1]],dtype=np.float32)
        transform=np.array([[0.7,-0.4,90],[0.4,0.7,15]],dtype=np.float32)
        target=source @ transform[:,:2].T+transform[:,2]
        np.testing.assert_allclose(fit_similarity(source,target),transform,atol=1e-5)
        np.testing.assert_allclose(inverse_transform_points(target,transform),source,atol=1e-5)

    def test_composition_order_is_observable(self):
        from tobii_linux.d1 import compose_affine
        import numpy as np
        scale=[[2,0,0],[0,2,0]];translate=[[1,0,3],[0,1,-4]]
        np.testing.assert_array_equal(compose_affine(scale,translate),[[2,0,6],[0,2,-8]])

    def test_reference_center_and_extent(self):
        import numpy as np
        from tobii_linux.d1 import normalize_reference_shape
        points=np.array([(i,2*i+30) for i in range(57)],dtype=np.float32)
        result=np.array(normalize_reference_shape(points))
        np.testing.assert_allclose(result.mean(axis=0),[48,48],atol=1e-5)
        self.assertAlmostEqual(float(np.ptp(result,axis=0).max()),48,places=5)

    def test_degenerate_geometry_is_rejected(self):
        from tobii_linux.d1 import fit_similarity, normalize_reference_shape, inverse_transform_points
        for operation in (lambda:fit_similarity([[1,1]]*3,[[0,0]]*3),
                          lambda:normalize_reference_shape([[1,1]]*57),
                          lambda:inverse_transform_points([[0,0]],[[0,0,0],[0,0,0]])):
            with self.assertRaises(ValueError):operation()


@unittest.skipUnless(importlib.util.find_spec('numpy'), 'Optional NumPy is not installed')
class D1QualityTests(unittest.TestCase):
    def test_alias_scores_are_included_in_fdp_order(self):
        from tobii_linux.d1_quality import D1_IDS, d1_quality_score
        ids = sorted(D1_IDS)
        values = [0] * 57
        values[ids.index((15,17))] = 1
        self.assertAlmostEqual(d1_quality_score(values, ids), 3/61, places=7)

    def test_quality_thresholds_are_strict(self):
        import numpy as np
        from tobii_linux.d1_quality import passes_d1_quality
        for mode, threshold in ((0,.5),(1,.5),(2,.35)):
            self.assertFalse(passes_d1_quality(threshold,mode))
            self.assertTrue(passes_d1_quality(np.nextafter(np.float32(threshold),np.float32(1)),mode))
        self.assertFalse(passes_d1_quality(float('nan'),2))
        self.assertFalse(passes_d1_quality(1,3))
