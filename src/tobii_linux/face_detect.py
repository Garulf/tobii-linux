"""Recovered face finder (VisionSDK detector 0x14007be00) for grayscale frames.

The letterbox, priors, decoding, suppression and network are
crates/tobii-inference/src/face_detect.rs; images are NumPy uint8 arrays.
"""
from dataclasses import dataclass

from ._native import face_detect as _face

INPUT_WIDTH = 256
INPUT_HEIGHT = 192
STRIDES = (8.0, 16.0, 32.0, 64.0)
ANCHOR_SIZES = ((10.0, 16.0, 24.0), (32.0, 48.0), (64.0, 96.0), (128.0, 192.0, 256.0))
CENTER_VARIANCE = 0.10000000149011612
SIZE_VARIANCE = 0.20000000298023224
NMS_OVERLAP = 0.4000000059604645
PI = 3.1415927410125732
HALF_PI = 1.5707963705062866
DEFAULT_SENSITIVITY = 0.75
DEFAULT_MIN_FACE = 0.15000000596046448
DEFAULT_MAX_FACE = 1.0
MIN_SCORE = 0.019999999552965164
RESIZE_COEFFICIENT_BITS = 11


@dataclass(frozen=True)
class Letterbox:
    offset_x: int
    offset_y: int
    width: int
    height: int
    scale: float


@dataclass(frozen=True)
class Detection:
    """Square face box in frame pixels plus the network's extra outputs."""
    x1: float
    y1: float
    x2: float
    y2: float
    score: float
    angles: tuple

    def rectangle(self):
        """Integer (x, y, side, side) as passed to the D1 landmark crop."""
        return (int(self.x1), int(self.y1), int(self.x2 - self.x1), int(self.x2 - self.x1))


def _image(frame):
    import numpy as np
    return np.asarray(frame, dtype=np.uint8)


def _layout(layout):
    return (layout.offset_x, layout.offset_y, layout.width, layout.height, layout.scale)


def _detections(found):
    return [Detection(*box[:5], tuple(box[5])) for box in found]


def _region(region):
    return None if region is None else tuple(region)


def resize_bilinear(image, width, height):
    """8-bit bilinear resize of 0x1400b4940 (mode 1), fixed-point 11-bit taps."""
    import numpy as np
    source = _image(image)
    data = _face.resize_bilinear(source.tobytes(), source.shape, width, height)
    return np.frombuffer(data, dtype=np.uint8).reshape(height, width).copy()


def letterbox_layout(frame_width, frame_height):
    return Letterbox(*_face.letterbox_layout(frame_width, frame_height))


def letterbox(frame):
    """Zero-padded 192x256 network image and its layout."""
    import numpy as np
    pixels = _image(frame)
    data, layout = _face.letterbox(pixels.tobytes(), pixels.shape)
    return np.frombuffer(data, dtype=np.uint8).reshape(INPUT_HEIGHT, INPUT_WIDTH).copy(), Letterbox(*layout)


def anchors(width=INPUT_WIDTH, height=INPUT_HEIGHT):
    """Normalised (cx, cy, w, h) priors in level, row, column, size order (0x14007b260)."""
    import numpy as np
    return np.frombuffer(_face.anchors(width, height), dtype=np.float32).reshape(-1, 4).copy()


def decode(output, priors, score_threshold, width=INPUT_WIDTH, height=INPUT_HEIGHT):
    """Candidate boxes in network pixels (0x14007bab0): x1, y1, x2, y2, score, angles."""
    import numpy as np
    values = np.asarray(output, dtype=np.float32).reshape(-1)
    priors = np.asarray(priors, dtype=np.float32)
    found = _face.decode(values.tobytes(), priors.tobytes(), score_threshold, width, height)
    f = np.float32
    return [(f(x1), f(y1), f(x2), f(y2), f(score), tuple(f(a) for a in angles))
            for x1, y1, x2, y2, score, angles in found]


def suppress(boxes):
    """Greedy suppression of 0x14007af20 on score-sorted boxes, inclusive pixel areas."""
    boxes = list(boxes)
    return [boxes[index] for index in _face.suppress([tuple(box[:4]) for box in boxes])]


def to_frame(boxes, layout, frame_width, frame_height, min_face=DEFAULT_MIN_FACE,
             max_face=DEFAULT_MAX_FACE, region=None):
    """Square frame-pixel detections, filtered by size fraction and region centre."""
    boxes = [(*box[:5], tuple(box[5])) for box in boxes]
    return _detections(_face.to_frame(boxes, _layout(layout), frame_width, frame_height,
                                      min_face, max_face, _region(region)))


def score_threshold(sensitivity=DEFAULT_SENSITIVITY):
    import numpy as np
    return np.float32(_face.score_threshold(sensitivity))


def detect_faces(frame, run_network, sensitivity=DEFAULT_SENSITIVITY, min_face=DEFAULT_MIN_FACE,
                 max_face=DEFAULT_MAX_FACE, region=None):
    """Full 0x14007be00 path; `run_network` maps the 192x256 uint8 image to raw outputs.

Detections are returned widest first, as the tracker uses the first one.
Stable sorts may order exact score or width ties differently from the
original's unstable sort.
"""
    import numpy as np
    pixels = _image(frame)
    image, layout = letterbox(pixels)
    output = np.asarray(run_network(image), dtype=np.float32).reshape(-1)
    found = _face.detections(output.tobytes(), _layout(layout), pixels.shape[1], pixels.shape[0],
                             sensitivity, min_face, max_face, _region(region))
    return _detections(found)


class FaceFinder:
    """The `ff` model on the Rust core's OpenVINO, with the shared 1/255 input scaling."""
    def __init__(self, model_directory):
        self._native = _face.FaceFinder(model_directory)

    def __call__(self, image):
        import numpy as np
        return np.frombuffer(self._native.run(_image(image).tobytes()), dtype=np.float32).copy()

    def detect(self, frame, sensitivity=DEFAULT_SENSITIVITY, min_face=DEFAULT_MIN_FACE,
               max_face=DEFAULT_MAX_FACE, region=None):
        pixels = _image(frame)
        return _detections(self._native.detect(pixels.tobytes(), pixels.shape, sensitivity,
                                               min_face, max_face, _region(region)))
