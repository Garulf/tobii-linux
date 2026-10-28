from pathlib import Path
import unittest
import unittest.mock
from tobii_linux.model_paths import default_models_dir


class DefaultModelsDirTests(unittest.TestCase):
    def test_follows_xdg_data_home(self):
        with unittest.mock.patch.dict('os.environ', {'XDG_DATA_HOME': '/data/share'}):
            self.assertEqual(default_models_dir(), Path('/data/share/tobii-linux/models'))

    def test_falls_back_to_local_share(self):
        with unittest.mock.patch.dict('os.environ', {'XDG_DATA_HOME': '', 'HOME': '/home/u'}):
            self.assertEqual(default_models_dir(), Path('/home/u/.local/share/tobii-linux/models'))


if __name__ == '__main__':
    unittest.main()
