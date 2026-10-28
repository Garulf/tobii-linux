import hashlib
import json
from pathlib import Path
import unittest
from tobii_linux.model_digests import EXPECTED_SHA256

MODELS = Path(__file__).resolve().parents[1] / 'models/tobii-et5'


@unittest.skipUnless(MODELS.is_dir(), 'models/tobii-et5 is absent')
class LocalModelsTests(unittest.TestCase):
    def test_present_files_match_the_expected_digests(self):
        for relative, digest in EXPECTED_SHA256.items():
            path = MODELS / relative
            if path.is_file():
                self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), digest, relative)

    @unittest.skipUnless((MODELS / 'catalog.json').is_file(), 'catalog.json is absent')
    def test_catalog_lists_the_expected_digests(self):
        catalog = json.loads((MODELS / 'catalog.json').read_bytes())
        for name, model in catalog['models'].items():
            for filename, entry in model['files'].items():
                self.assertEqual(EXPECTED_SHA256[f'{name}/{filename}'], entry['sha256'])


if __name__ == '__main__':
    unittest.main()
