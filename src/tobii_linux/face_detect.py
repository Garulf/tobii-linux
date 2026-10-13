"""Recovered face finder (VisionSDK detector 0x14007be00) for grayscale frames."""
from dataclasses import dataclass
import functools
import math

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


def _f32(value):
    import numpy as np
    return np.float32(value)


def _round_even(value):
    return int(round(float(value)))


@functools.lru_cache(maxsize=64)
def _resize_taps(source, target):
    import numpy as np
    scale = 1.0 / (target / source)
    indices, weights = [], []
    for position in range(target):
        exact = np.float32((position + 0.5) * scale - 0.5)
        index = math.floor(exact)
        fraction = np.float32(exact - np.float32(index))
        if index < 0:
            index, fraction = 0, np.float32(0)
        if index >= source - 1:
            index, fraction = source - 1, np.float32(0)
        first = int(np.rint((np.float32(1) - fraction) * np.float32(1 << RESIZE_COEFFICIENT_BITS)))
        indices.append(index)
        weights.append((first, (1 << RESIZE_COEFFICIENT_BITS) - first))
    return indices, weights


def resize_bilinear(image, width, height):
    """8-bit bilinear resize of 0x1400b4940 (mode 1), fixed-point 11-bit taps."""
    import numpy as np
    source = np.asarray(image, dtype=np.uint8)
    if source.ndim != 2 or min(source.shape) < 1 or width < 1 or height < 1:
        raise ValueError('Expected a nonempty 2D grayscale image and positive size')
    rows, columns = source.shape
    x_index, x_weight = (np.asarray(v) for v in _resize_taps(columns, width))
    y_index, y_weight = (np.asarray(v) for v in _resize_taps(rows, height))
    pixels = source.astype(np.int64)
    x_next = np.minimum(x_index + 1, columns - 1)
    horizontal = pixels[:, x_index] * x_weight[:, 0] + pixels[:, x_next] * x_weight[:, 1]
    y_next = np.minimum(y_index + 1, rows - 1)
    vertical = (horizontal[y_index] * y_weight[:, :1] + horizontal[y_next] * y_weight[:, 1:]
                + (1 << (2*RESIZE_COEFFICIENT_BITS - 1))) >> (2*RESIZE_COEFFICIENT_BITS)
    return np.clip(vertical, 0, 255).astype(np.uint8)


def letterbox_layout(frame_width, frame_height):
    width, height = _f32(frame_width), _f32(frame_height)
    if width / height <= _f32(4.0) / _f32(3.0):
        scale = height * _f32(0.0052083334885537624)
        inner = _round_even(_f32(192.0) / (height / width))
        return Letterbox((INPUT_WIDTH - inner) // 2, 0, inner, INPUT_HEIGHT, float(scale))
    scale = width * _f32(0.00390625)
    inner = _round_even(_f32(256.0) / (width / height))
    return Letterbox(0, (INPUT_HEIGHT - inner) // 2, INPUT_WIDTH, inner, float(scale))


def letterbox(frame):
    """Zero-padded 192x256 network image and its layout."""
    import numpy as np
    pixels = np.asarray(frame, dtype=np.uint8)
    layout = letterbox_layout(pixels.shape[1], pixels.shape[0])
    image = np.zeros((INPUT_HEIGHT, INPUT_WIDTH), dtype=np.uint8)
    image[layout.offset_y:layout.offset_y + layout.height,
          layout.offset_x:layout.offset_x + layout.width] = resize_bilinear(pixels, layout.width, layout.height)
    return image, layout


def anchors(width=INPUT_WIDTH, height=INPUT_HEIGHT):
    """Normalised (cx, cy, w, h) priors in level, row, column, size order (0x14007b260)."""
    import numpy as np
    one = _f32(1.0)
    result = []
    for stride, sizes in zip(STRIDES, ANCHOR_SIZES):
        stride = _f32(stride)
        rows = math.ceil(_f32(height) * (one / stride))
        columns = math.ceil(_f32(width) * (one / stride))
        for row in range(rows):
            for column in range(columns):
                center_x = (_f32(column) + _f32(0.5)) * stride * (one / _f32(width))
                center_y = (_f32(row) + _f32(0.5)) * stride * (one / _f32(height))
                for size in sizes:
                    result.append((center_x, center_y, _f32(size) * (one / _f32(width)),
                                   (one / _f32(height)) * _f32(size)))
    return np.asarray(result, dtype=np.float32)


def decode(output, priors, score_threshold, width=INPUT_WIDTH, height=INPUT_HEIGHT):
    """Candidate boxes in network pixels (0x14007bab0): x1, y1, x2, y2, score, angles."""
    import numpy as np
    values = np.asarray(output, dtype=np.float32).reshape(-1)
    count = len(priors)
    if values.size != count * 9:
        raise ValueError('Output size does not match the anchor count')
    offsets = values[:4*count].reshape(count, 4)
    scores = values[4*count:6*count].reshape(count, 2)[:, 1]
    extra = values[6*count:].reshape(count, 3)
    image_width, image_height = _f32(width), _f32(height)
    center_variance, size_variance, half = _f32(CENTER_VARIANCE), _f32(SIZE_VARIANCE), _f32(0.5)
    pi, half_pi = _f32(PI), _f32(HALF_PI)
    boxes = []
    for index in np.flatnonzero(scores > _f32(score_threshold)):
        prior, offset = priors[index], offsets[index]
        center_x = offset[0] * center_variance * prior[2] + prior[0]
        center_y = offset[1] * center_variance * prior[3] + prior[1]
        half_width = _f32(math.exp(offset[2] * size_variance)) * prior[2] * half
        half_height = _f32(math.exp(offset[3] * size_variance)) * prior[3] * half
        x1 = max((center_x - half_width) * image_width, _f32(0))
        y1 = max((center_y - half_height) * image_height, _f32(0))
        x2 = min((half_width + center_x) * image_width, image_width)
        y2 = min((half_height + center_y) * image_height, image_height)
        angles = (extra[index, 0] * pi - half_pi, half_pi - extra[index, 1] * pi, extra[index, 2] * pi - half_pi)
        boxes.append((x1, y1, x2, y2, scores[index], angles))
    return boxes


def suppress(boxes):
    """Greedy suppression of 0x14007af20 on score-sorted boxes, inclusive pixel areas."""
    one, limit = _f32(1.0), _f32(NMS_OVERLAP)
    kept = list(boxes)
    areas = [((b[3] - b[1]) + one) * ((b[2] - b[0]) + one) for b in kept]
    first = 0
    while first < len(kept):
        other = first + 1
        while other < len(kept):
            a, b = kept[first], kept[other]
            overlap_w = max((min(a[2], b[2]) - max(a[0], b[0])) + one, _f32(0))
            overlap_h = max((min(a[3], b[3]) - max(a[1], b[1])) + one, _f32(0))
            shared = overlap_h * overlap_w
            if shared / ((areas[other] + areas[first]) - shared) < limit:
                other += 1
            else:
                del kept[other], areas[other]
        first += 1
    return kept


def to_frame(boxes, layout, frame_width, frame_height, min_face=DEFAULT_MIN_FACE,
             max_face=DEFAULT_MAX_FACE, region=None):
    """Square frame-pixel detections, filtered by size fraction and region centre."""
    scale, half, quarter = _f32(layout.scale), _f32(0.5), _f32(0.25)
    shortest = _f32(min(frame_width, frame_height))
    lowest = min(max(_f32(min_face), _f32(0)), _f32(1))
    highest = max(_f32(min_face), min(_f32(max_face), _f32(1)))
    result = []
    for x1, y1, x2, y2, score, angles in boxes:
        x1, x2 = x1 - _f32(layout.offset_x), x2 - _f32(layout.offset_x)
        y1, y2 = y1 - _f32(layout.offset_y), y2 - _f32(layout.offset_y)
        radius = _round_even(((y2*scale - y1*scale) + (x2*scale - x1*scale)) * quarter)
        left = _f32(_round_even((x2 + x1) * scale * half) - radius)
        top = _f32(_round_even((y2 + y1) * scale * half) - radius)
        side = _f32(radius * 2)
        if not shortest * lowest < side < shortest * highest:
            continue
        if region is not None:
            center_x = _round_even((side + left + left) * half)
            center_y = _round_even((side + top + top) * half)
            rx, ry, rw, rh = region
            if not (rx < center_x < rx + rw and ry < center_y < ry + rh):
                continue
        result.append(Detection(float(left), float(top), float(side + left), float(side + top),
                                float(score), tuple(float(a) for a in angles)))
    return result


def score_threshold(sensitivity=DEFAULT_SENSITIVITY):
    return max(_f32(MIN_SCORE), min(_f32(1.0), _f32(1.0) - _f32(sensitivity)))


def detect_faces(frame, run_network, sensitivity=DEFAULT_SENSITIVITY, min_face=DEFAULT_MIN_FACE,
                 max_face=DEFAULT_MAX_FACE, region=None):
    """Full 0x14007be00 path; `run_network` maps the 192x256 uint8 image to raw outputs.

Detections are returned widest first, as the tracker uses the first one.
Python's stable sorts may order exact score or width ties differently from the
original's unstable sort.
"""
    import numpy as np
    pixels = np.asarray(frame, dtype=np.uint8)
    image, layout = letterbox(pixels)
    candidates = decode(run_network(image), anchors(), score_threshold(sensitivity))
    candidates.sort(key=lambda box: -box[4])
    kept = suppress(candidates)
    kept.sort(key=lambda box: -(box[2] - box[0]))
    return to_frame(kept, layout, pixels.shape[1], pixels.shape[0], min_face, max_face, region)


class FaceFinder:
    """OpenVINO runner for the `ff` model with the shared 1/255 input scaling."""
    def __init__(self, model_directory):
        import openvino as ov
        from .legacy_ir import load_model
        self.compiled = ov.Core().compile_model(
            load_model(model_directory), 'CPU',
            {'INFERENCE_PRECISION_HINT': 'f32', 'INFERENCE_NUM_THREADS': 2})

    def __call__(self, image):
        import numpy as np
        factor = np.float32(0.003921568859368563)
        tensor = (np.asarray(image, dtype=np.uint8).astype(np.float32) * factor).reshape(1, 1, 192, 256)
        return self.compiled([tensor])[0].reshape(-1)

    def detect(self, frame, **options):
        return detect_faces(frame, self, **options)
