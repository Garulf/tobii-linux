import subprocess
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DRAW = ROOT / "tools" / "draw_tray_icons.py"
TRAY = ROOT / "crates" / "tobii-experience" / "assets" / "tray"


def chunks(png):
    position = 8
    while position < len(png):
        length = int.from_bytes(png[position:position + 4], "big")
        yield png[position + 4:position + 8], png[position + 8:position + 8 + length]
        position += 12 + length


def image(path):
    """The header and filtered scanlines of a PNG, whichever zlib packed them."""
    parts = list(chunks(path.read_bytes()))
    header = next(data for kind, data in parts if kind == b"IHDR")
    return header, zlib.decompress(b"".join(data for kind, data in parts if kind == b"IDAT"))


class DrawTrayIconsTest(unittest.TestCase):
    def test_committed_icons_are_the_drawing(self):
        with tempfile.TemporaryDirectory() as output:
            subprocess.run([sys.executable, str(DRAW), output], check=True, capture_output=True)
            drawn = sorted(path.name for path in Path(output).iterdir())
            self.assertEqual(drawn, sorted(path.name for path in TRAY.glob("*.png")))
            for name in drawn:
                with self.subTest(name):
                    self.assertEqual(image(Path(output) / name), image(TRAY / name))


if __name__ == "__main__":
    unittest.main()
