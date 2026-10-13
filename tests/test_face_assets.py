from pathlib import Path
import struct
import tempfile
import unittest
from tobii_linux.face_assets import read_shapes, read_landmark_ids, read_fdp, read_mesh, landmark_vertices
from tobii_linux.lbf import read_lbf, read_lv, quantize_matrix

SUPPORT = Path(__file__).resolve().parents[1] / 'models/tobii-et5/support'


class FaceAssetsTests(unittest.TestCase):
    @unittest.skipUnless(SUPPORT.is_dir(), 'Research assets are excluded from the Nix package')
    def test_installed_landmarks_resolve_to_mesh(self):
        mesh = read_mesh(SUPPORT / 'jk_300.wfm')
        ids = read_landmark_ids(SUPPORT / 'bdtsdata/NN/vnn_nir/landmarks.bin')
        mapped = landmark_vertices(mesh, read_fdp(SUPPORT / 'jk_300_rigid.fdp'), ids)
        self.assertEqual((len(mesh.vertices), len(mesh.triangles), len(mapped)), (357, 640, 57))
        self.assertEqual(read_shapes(SUPPORT / 'bdtsdata/NN/is.bin', 3)[0], (9, 35, 2))

    def test_shape_rejects_nonfinite_and_bad_extent(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'shape'
            for data in (struct.pack('<IIff', 1, 2, float('nan'), 0), struct.pack('<II', 57, 2)):
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    read_shapes(path)

    def test_landmarks_reject_duplicates(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'ids'
            path.write_bytes(struct.pack('<IIIII', 2, 1, 2, 1, 2))
            with self.assertRaises(ValueError):
                read_landmark_ids(path)

    @unittest.skipUnless(SUPPORT.is_dir(), 'Research assets are excluded from the Nix package')
    def test_lbf_installed_extents_and_topology(self):
        for name, trees, end, features in [('fa', 1980, 145093, 7920), ('fc', 1260, 92525, 5040)]:
            model = read_lbf(SUPPORT / f'bdtsdata/NN/{name}.lbf')
            self.assertEqual((model['tree_count'], model['forest_end']), (trees, end))
            self.assertEqual(model['stages'][0][0]['shape'], (features, 32))
            self.assertEqual(model['stages'][-1][-1]['shape'], (112, 70))

    @unittest.skipUnless(SUPPORT.is_dir(), 'Research assets are excluded from the Nix package')
    def test_lbf_rejects_truncation_trailing_bytes_and_other_layout(self):
        original = (SUPPORT / 'bdtsdata/NN/fc.lbf').read_bytes()
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'model'
            for data in (original[:100], original + b'!', b'\x03' + original[1:]):
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    read_lbf(path)
        with self.assertRaises(ValueError):
            read_lbf(SUPPORT / 'bdtsdata/LBF/lv')

    @unittest.skipUnless(SUPPORT.is_dir(), 'Research assets are excluded from the Nix package')
    def test_lv_layout_and_boundaries(self):
        path = SUPPORT / 'bdtsdata/LBF/lv'
        model = read_lv(path)
        self.assertEqual(len(model['records']), 13)
        self.assertEqual(sum(len(r['forest'][2]) for r in model['records']), 650)
        self.assertTrue(all(r['matrix']['shape'] == (6400, 1) for r in model['records']))
        self.assertTrue(all(r['matrix']['biases'] == bytes(4) for r in model['records']))
        with tempfile.TemporaryDirectory() as d:
            malformed = Path(d) / 'lv'
            for data in (path.read_bytes()[:-1], path.read_bytes() + b'!'):
                malformed.write_bytes(data)
                with self.assertRaises(ValueError):
                    read_lv(malformed)

    def test_quantization_uses_codebook_range_and_truncates(self):
        matrix = {'shape': (2, 2), 'weights': struct.pack('<ffff', 0.25, -0.3, -0.25, 0.3),
                  'codebook_max_abs': (0.5, 1.5)}
        weights, scales = quantize_matrix(matrix)
        self.assertEqual(struct.unpack('<ff', scales), (254.0, 84.0))
        self.assertEqual(struct.unpack('<bbbb', weights), (63, -25, -63, 25))
        with self.assertRaises(ValueError):
            quantize_matrix({**matrix, 'codebook_max_abs': (0.0, 1.5)})
