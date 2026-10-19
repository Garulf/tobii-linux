#!/usr/bin/env python3
"""Draw the experience app's tray icons.

The icons are this project's own artwork, not Tobii's: an eye in blue for Enabled, the same
eye in grey with a slash for Disabled, the blue eye with an amber dot for GetStarted, and a
ring of fading dots for Progress at the sizes a tray draws. Shapes are sampled 8 x 8 per pixel
and written as RGBA PNGs with the standard library only, so every run gives the same pixels
(tests/test_draw_tray_icons.py compares them with the committed icons).

    python3 tools/draw_tray_icons.py [output directory]

Output goes to crates/tobii-experience/assets/tray by default.
"""

import hashlib
import math
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT = ROOT / 'crates' / 'tobii-experience' / 'assets' / 'tray'

STATUS_SIZE = 33
PROGRESS_SIZES = (16, 24, 32, 48, 64)
SAMPLES = 8

BLUE = (0x2E, 0x9C, 0xDB)
GREY = (0x9A, 0xA0, 0xA6)
PUPIL = (0x14, 0x2B, 0x40)
AMBER = (0xF5, 0xA6, 0x23)
OUTLINE = (0x33, 0x33, 0x33)
WHITE = (0xFF, 0xFF, 0xFF)


def lens(x, y, half_width, half_height):
    """Inside the almond made by two circular arcs through (+-half_width, 0)."""
    radius = (half_width ** 2 + half_height ** 2) / (2 * half_height)
    return (math.hypot(x, y - (half_height - radius)) <= radius
            and math.hypot(x, y + (half_height - radius)) <= radius)


def disc(x, y, cx, cy, r):
    return math.hypot(x - cx, y - cy) <= r


def segment_distance(x, y, x0, y0, x1, y1):
    dx, dy = x1 - x0, y1 - y0
    t = max(0.0, min(1.0, ((x - x0) * dx + (y - y0) * dy) / (dx * dx + dy * dy)))
    return math.hypot(x - (x0 + t * dx), y - (y0 + t * dy))


def eye_layers(colour):
    """Paint order for an eye centred in the unit square [-1, 1]: outline ring, iris, pupil."""
    def outline(x, y):
        return lens(x, y, 0.92, 0.56) and not lens(x, y, 0.74, 0.40)

    return [
        (outline, colour),
        (lambda x, y: disc(x, y, 0, 0, 0.34), colour),
        (lambda x, y: disc(x, y, 0, 0, 0.15), PUPIL),
        (lambda x, y: disc(x, y, 0.09, -0.09, 0.06), WHITE),
    ]


def enabled():
    return eye_layers(BLUE)


def disabled():
    def slash_gap(x, y):
        return segment_distance(x, y, -0.78, -0.78, 0.78, 0.78) <= 0.17

    def slash(x, y):
        return segment_distance(x, y, -0.78, -0.78, 0.78, 0.78) <= 0.08

    layers = [(lambda x, y, inside=inside: inside(x, y) and not slash_gap(x, y), colour)
              for inside, colour in eye_layers(GREY)]
    return layers + [(slash, GREY)]


def get_started():
    return eye_layers(BLUE) + [
        (lambda x, y: disc(x, y, 0.66, 0.66, 0.32), OUTLINE),
        (lambda x, y: disc(x, y, 0.66, 0.66, 0.24), AMBER),
    ]


def progress():
    dots = 8
    layers = []
    for index in range(dots):
        angle = -math.pi / 2 - 2 * math.pi * index / dots
        level = round(0xFF - (0xFF - 0x50) * index / (dots - 1))
        cx, cy = round(0.66 * math.cos(angle), 9), round(0.66 * math.sin(angle), 9)
        layers.append((lambda x, y, cx=cx, cy=cy: disc(x, y, cx, cy, 0.17), (level, level, level)))
    return layers


def render(layers, size):
    """RGBA rows with straight alpha: per pixel coverage of the topmost layer per sample."""
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            red = green = blue = covered = 0
            for sy in range(SAMPLES):
                for sx in range(SAMPLES):
                    x = ((px + (sx + 0.5) / SAMPLES) / size) * 2 - 1
                    y = ((py + (sy + 0.5) / SAMPLES) / size) * 2 - 1
                    for inside, colour in reversed(layers):
                        if inside(x, y):
                            red, green, blue = red + colour[0], green + colour[1], blue + colour[2]
                            covered += 1
                            break
            if covered:
                row += bytes((round(red / covered), round(green / covered), round(blue / covered),
                              round(255 * covered / SAMPLES ** 2)))
            else:
                row += b'\xff\xff\xff\x00'
        rows.append(bytes(row))
    return rows


def png(rows, size):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

    raw = b''.join(b'\x00' + row for row in rows)
    header = struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', header) + chunk(b'IDAT', zlib.compress(raw, 9))
            + chunk(b'IEND', b''))


def icons():
    yield 'Enabled.png', enabled(), STATUS_SIZE
    yield 'Disabled.png', disabled(), STATUS_SIZE
    yield 'GetStarted.png', get_started(), STATUS_SIZE
    for size in PROGRESS_SIZES:
        yield f'Progress-{size}.png', progress(), size


def main(argv):
    output = Path(argv[1]) if len(argv) > 1 else DEFAULT_OUTPUT
    output.mkdir(parents=True, exist_ok=True)
    for name, layers, size in icons():
        data = png(render(layers, size), size)
        (output / name).write_bytes(data)
        print(f'{hashlib.sha256(data).hexdigest()}  {name}')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
