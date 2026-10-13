"""Recovered D1 crop and landmark component; pose/tracking remain separate."""
from dataclasses import dataclass
import math
import struct


def _f32(value):
    return struct.unpack('<f', struct.pack('<f', value))[0]


@dataclass(frozen=True)
class D1Output:
    crop_points: tuple
    quality: tuple


def decode_output(values, normalized_initial_shape, crop_width=96):
    """Decode 57 XY offset pairs plus 57 quality values.

The caller must provide the runtime-normalized initial shape, NOT raw
init_shape.bin coordinates. Inverse crop transformation is still required to
obtain image coordinates. No quality threshold or pose is inferred here.
"""
    values = tuple(_f32(float(v)) for v in values)
    shape = tuple(tuple(_f32(float(x)) for x in point) for point in normalized_initial_shape)
    if len(shape) != 57 or any(len(p) != 2 for p in shape) or len(values) != 171:
        raise ValueError('Expected 57 initial XY pairs and 171 D1 outputs')
    if crop_width != 96 or not all(math.isfinite(v) for v in values) or not all(math.isfinite(x) for p in shape for x in p):
        raise ValueError('Unsupported crop or non-finite D1 data')
    points = tuple(tuple(_f32(base + _f32(_f32(values[2*i+j] * crop_width) * 0.5))
                         for j, base in enumerate(point)) for i, point in enumerate(shape))
    return D1Output(points, values[114:])


def normalize_image_points(points, image_width, image_height):
    """Normalize image XY as the observed FDP writer does, after inverse warp."""
    if not 0 < image_width <= 32768 or not 0 < image_height <= 32768:
        raise ValueError('Invalid image dimensions')
    result = tuple((_f32(_f32(x) / image_width), _f32(_f32(y) / image_height)) for x, y in points)
    if not all(math.isfinite(x) for p in result for x in p):
        raise ValueError('Non-finite image point')
    return result


def prepare_crop_tensor(pixels):
    """Convert a contiguous 96×96 grayscale byte crop for the OpenVINO runner.

This reproduces uint8-to-float normalization only. It does not locate, warp,
rotate or pad a crop from a camera frame.
"""
    import numpy as np
    if len(pixels) != 96 * 96:
        raise ValueError('Expected exactly 9216 grayscale bytes')
    source = np.frombuffer(pixels, dtype=np.uint8)
    # Exact stored float constant at runtime RVA 0x474aa0.
    factor = np.float32(0.003921568859368563)
    return (source.astype(np.float32) * factor).reshape(1, 1, 96, 96)


def normalize_reference_shape(points, crop_width=96):
    """Center on per-axis means, scale the larger extent to half the crop width.

Recovered from 0x1400ac560. Float reduction/vectorization may produce small
rounding differences; checked numerically against the original routine.
"""
    import numpy as np
    shape = np.asarray(points, dtype=np.float32)
    if shape.shape != (57, 2) or crop_width != 96 or not np.isfinite(shape).all():
        raise ValueError('Expected finite 57×2 D1 reference shape')
    extent = np.max(np.max(shape, axis=0) - np.min(shape, axis=0))
    if extent <= 0:
        raise ValueError('Degenerate reference shape')
    centered = shape - shape.mean(axis=0, dtype=np.float32)
    factor = np.float32(np.float32(0.5) / extent) * np.float32(crop_width)
    result = centered * factor + np.float32(crop_width * 0.5)
    return tuple(tuple(float(x) for x in point) for point in result)


def fit_similarity(source, target):
    """Fit source→target 2×3 similarity using recovered 0x1400dac40 math.

Numerical agreement is checked against original code; reduction order can
change rounding. Degenerate/non-finite inputs are rejected by this API.
"""
    import numpy as np
    source = np.asarray(source, dtype=np.float32)
    target = np.asarray(target, dtype=np.float32)
    if (source.ndim != 2 or source.shape[1] != 2 or source.shape != target.shape
            or len(source) < 2 or not np.isfinite(source).all() or not np.isfinite(target).all()):
        raise ValueError('Expected matching finite N×2 point sets')
    mean_source = source.mean(axis=0, dtype=np.float32)
    mean_target = target.mean(axis=0, dtype=np.float32)
    x, y = source - mean_source, target - mean_target
    norm = np.float32(np.sqrt(np.sum(x.astype(np.float64) ** 2)))
    if norm == 0:
        raise ValueError('Degenerate source shape')
    reciprocal = np.float32(1) / np.float32(norm * norm)
    dot = np.float32(np.sum(x.astype(np.float64) * y.astype(np.float64)))
    cross_terms = x[:, 0] * y[:, 1] - x[:, 1] * y[:, 0]
    cross = np.float32(0)
    n = len(x) // 4 * 4
    for i in range(0, n, 4):
        cross = np.float32(np.float32(np.float32(cross_terms[i+1] + cross_terms[i]) + cross)
                           + cross_terms[i+2]) + cross_terms[i+3]
    for term in cross_terms[n:]:
        cross = np.float32(cross + term)
    a, b = np.float32(dot * reciprocal), np.float32(cross * reciprocal)
    linear = np.array([[a, -b], [b, a]], dtype=np.float32)
    translation = mean_target - (linear.astype(np.float64) @ mean_source.astype(np.float64)).astype(np.float32)
    return np.column_stack((linear, translation))


def inverse_transform_points(points, transform):
    """Map crop coordinates back through a non-singular image→crop affine."""
    import numpy as np
    points = np.asarray(points, dtype=np.float32)
    transform = np.asarray(transform, dtype=np.float32)
    if (points.ndim != 2 or points.shape[1] != 2 or transform.shape != (2, 3)
            or not np.isfinite(points).all() or not np.isfinite(transform).all()):
        raise ValueError('Expected finite N×2 points and 2×3 transform')
    linear = transform[:, :2].astype(np.float64)
    # The original float matrix branch rounds both products and subtraction
    # in float32, then uses double precision for reciprocal and coefficients.
    f = transform[:, :2]
    determinant = float(np.float32(f[0, 0]*f[1, 1] - f[0, 1]*f[1, 0]))
    if determinant == 0:
        raise ValueError('Singular transform')
    inverse = np.array([[linear[1, 1], -linear[0, 1]], [-linear[1, 0], linear[0, 0]]]) / determinant
    offset = -inverse @ transform[:, 2].astype(np.float64)
    # Original inverse is written into a float32 matrix before GEMM.
    inverse = inverse.astype(np.float32).astype(np.float64)
    offset = offset.astype(np.float32).astype(np.float64)
    return (points.astype(np.float64) @ inverse.T + offset).astype(np.float32)


def compose_affine(first, second):
    """Compose as first @ second, the order observed at 0x1400db330."""
    import numpy as np
    first, second = np.asarray(first, dtype=np.float32), np.asarray(second, dtype=np.float32)
    if first.shape != (2, 3) or second.shape != (2, 3) or not np.isfinite(first).all() or not np.isfinite(second).all():
        raise ValueError('Expected finite 2×3 transforms')
    a, b = np.eye(3), np.eye(3)
    a[:2], b[:2] = first, second
    return (a @ b)[:2].astype(np.float32)


def warp_grayscale(pixels, image_width, image_height, transform, crop_width=96, crop_height=96):
    """Recovered D1 grayscale affine crop with fixed-point bilinear sampling.

Image→crop transform is inverted in double precision for resampling. Samples
use a 1/32-pixel grid, with separately rounded 10-bit row/column contributions,
constant-zero borders and rounded integer pixel output.
"""
    import numpy as np
    if (not 0 < image_width <= 32768 or not 0 < image_height <= 32768
            or not 0 < crop_width <= 32768 or not 0 < crop_height <= 32768
            or len(pixels) != image_width * image_height):
        raise ValueError('Invalid contiguous grayscale image/crop dimensions')
    t = np.asarray(transform, dtype=np.float32)
    if t.shape != (2, 3) or not np.isfinite(t).all():
        raise ValueError('Expected finite 2×3 transform')
    linear = t[:, :2].astype(np.float64)
    determinant = linear[0, 0]*linear[1, 1] - linear[0, 1]*linear[1, 0]
    if determinant == 0:
        raise ValueError('Singular transform')
    inverse = np.array([[linear[1, 1], -linear[0, 1]], [-linear[1, 0], linear[0, 0]]]) / determinant
    offset = -inverse @ t[:, 2].astype(np.float64)
    x, y = np.arange(crop_width), np.arange(crop_height)[:, None]
    fixed = [((np.rint(inverse[j, 0]*x*1024).astype(np.int64)
               + np.rint((inverse[j, 1]*y+offset[j])*1024).astype(np.int64) + 16) >> 5)
             for j in range(2)]
    ix, iy = (np.clip(f >> 5, -32768, 32767) for f in fixed)
    fx, fy = (f & 31 for f in fixed)
    source = np.frombuffer(pixels, dtype=np.uint8).reshape(image_height, image_width)
    output = np.zeros((crop_height, crop_width), dtype=np.int64)
    for dx, dy, weight in ((0,0,(32-fx)*(32-fy)), (1,0,fx*(32-fy)),
                           (0,1,(32-fx)*fy), (1,1,fx*fy)):
        sx, sy = ix+dx, iy+dy
        valid = (sx >= 0) & (sx < image_width) & (sy >= 0) & (sy < image_height)
        sample = source[np.clip(sy,0,image_height-1), np.clip(sx,0,image_width-1)]
        output += np.where(valid, sample.astype(np.int64)*weight, 0)
    return np.clip((output+512) >> 10, 0, 255).astype(np.uint8).tobytes()


def crop_transform(normalized_reference, bounding_box, roll_degrees=0.0, compensate_roll=True):
    """Build image→96×96 crop transform from an externally supplied face box.

The box is integer (x,y,width,height). Roll compensation follows the consumer's
strict >5-degree threshold. Face detection and temporal roll estimation are
not performed here.
"""
    import numpy as np
    reference = np.asarray(normalized_reference, dtype=np.float32)
    box = tuple(bounding_box)
    if (reference.shape != (57, 2) or not np.isfinite(reference).all()
            or len(box) != 4 or any(not isinstance(x, (int, np.integer)) for x in box)
            or min(box[2:]) <= 0 or not math.isfinite(roll_degrees)):
        raise ValueError('Expected finite 57×2 reference and positive integer face box')
    minimum, maximum = reference.min(axis=0), reference.max(axis=0)
    extent = maximum - minimum
    if np.any(extent <= 0):
        raise ValueError('Degenerate reference shape')
    scale = np.float32(np.float32(box[3]) / extent[1] + np.float32(box[2]) / extent[0]) * np.float32(0.5)
    fitted = reference * scale
    low = fitted.min(axis=0)
    span = fitted.max(axis=0) - low
    center = np.asarray(box[2:], dtype=np.float32) * np.float32(0.5) + np.asarray(box[:2], dtype=np.float32)
    fitted += center - (span * np.float32(0.5) + low)
    transform = fit_similarity(fitted, reference)
    roll = np.float32(roll_degrees)
    if compensate_roll and abs(roll) > 5:
        angle = np.float32(roll * np.float32(0.01745329424738884))
        sine, cosine = np.float32(math.sin(float(angle))), np.float32(math.cos(float(angle)))
        s, c = sine * np.float32(48), cosine * np.float32(48)
        rotation = np.array([[cosine, sine, np.float32(np.float32(48)-c)-s],
                             [-sine, cosine, np.float32(s+np.float32(48))-c]], dtype=np.float32)
        transform = compose_affine(rotation, transform)
    return transform


@dataclass(frozen=True)
class D1Landmarks:
    image_points: tuple
    normalized_points: tuple
    quality: tuple
    image_to_crop: tuple


class D1LandmarkModel:
    """Linux CPU landmark component for caller-supplied grayscale face boxes.

Requires optional OpenVINO/NumPy. Does not detect faces, acquire USB frames,
estimate pose, update temporal roll or publish tracking to games.
"""
    def __init__(self, model_directory, raw_reference):
        import openvino as ov
        from .legacy_ir import load_model
        self.reference = normalize_reference_shape(raw_reference)
        self.compiled = ov.Core().compile_model(
            load_model(model_directory), 'CPU',
            {'INFERENCE_PRECISION_HINT': 'f32', 'INFERENCE_NUM_THREADS': 2})

    def predict_frame(self, pixels, width, height, bounding_box, roll_degrees=0.0, compensate_roll=True):
        transform = crop_transform(self.reference, bounding_box, roll_degrees, compensate_roll)
        crop = warp_grayscale(pixels, width, height, transform)
        output = self.compiled([prepare_crop_tensor(crop)])[0].reshape(-1)
        decoded = decode_output(output, self.reference)
        image = inverse_transform_points(decoded.crop_points, transform)
        image_points = tuple(tuple(float(x) for x in p) for p in image)
        return D1Landmarks(image_points, normalize_image_points(image_points, width, height),
                           decoded.quality, tuple(tuple(float(x) for x in p) for p in transform))
