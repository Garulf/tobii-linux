import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

from tobii_linux.legacy_ir import load_model


@unittest.skipUnless(importlib.util.find_spec('openvino') and importlib.util.find_spec('numpy'),
                     'Optional inference dependencies are not installed')
class LegacyIRTests(unittest.TestCase):
    def fixture(self, directory, xml, weights=b''):
        target = Path(directory)
        files = {'model.xml': xml.encode(), 'model.bin': weights}
        for name, value in files.items():
            (target / name).write_bytes(value)
        (target / 'manifest.json').write_text(json.dumps({'files': {
            name: {'sha256': hashlib.sha256(value).hexdigest()} for name, value in files.items()}}))
        return target

    def test_hash_mismatch_rejected_before_loading(self):
        with tempfile.TemporaryDirectory() as directory:
            target = self.fixture(directory, '<net version="4"/>')
            (target / 'model.bin').write_bytes(b'corrupted')
            with self.assertRaisesRegex(ValueError, 'hash mismatch'):
                load_model(target)

    def test_missing_connection_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            target = self.fixture(directory, '''<net version="6"><layers>
              <layer id="1" type="ReLU"><input><port id="0"><dim>1</dim></port></input>
              <output><port id="1"><dim>1</dim></port></output></layer>
              </layers></net>''')
            with self.assertRaisesRegex(ValueError, 'Unconnected input'):
                load_model(target)

    def test_constant_blob_extent_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            target = self.fixture(directory, '''<net version="6"><layers>
              <layer id="0" name="bad" type="Const" precision="FP32">
              <output><port id="0"><dim>2</dim></port></output>
              <blobs><custom offset="0" size="8"/></blobs></layer>
              </layers></net>''', b'\0'*4)
            with self.assertRaisesRegex(ValueError, 'blob extent'):
                load_model(target)


if __name__ == '__main__':
    unittest.main()
