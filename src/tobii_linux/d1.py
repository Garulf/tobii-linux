"""Recovered D1 crop and landmark component; pose/tracking remain separate.

The crop, tensor, decoding and inference are crates/tobii-inference/src/d1.rs; the network
runs on the OpenVINO C library the Rust core loads (docs/rust.md). Arrays are NumPy float32.
"""
from dataclasses import dataclass

from ._native import d1 as _d1


def _matrix(data, shape):
    import numpy as np
    return np.frombuffer(data, dtype=np.float32).reshape(shape).copy()


def _points(points):
    import numpy as np
    array = np.asarray(points, dtype=np.float32)
    return array.tobytes(), array.shape


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
    values = [float(v) for v in values]
    shape = [[float(x) for x in point] for point in normalized_initial_shape]
    points, quality = _d1.decode_output(values, shape, crop_width)
    return D1Output(tuple(points), tuple(quality))


def normalize_image_points(points, image_width, image_height):
    """Normalize image XY as the observed FDP writer does, after inverse warp."""
    return tuple(_d1.normalize_image_points(list(points), image_width, image_height))


def prepare_crop_tensor(pixels):
    """Convert a contiguous 96×96 grayscale byte crop for the OpenVINO runner.

This reproduces uint8-to-float normalization only (the stored float constant at
runtime RVA 0x474aa0). It does not locate, warp, rotate or pad a crop from a camera frame.
"""
    return _matrix(_d1.prepare_crop_tensor(pixels), (1, 1, 96, 96))


def normalize_reference_shape(points, crop_width=96):
    """Center on per-axis means, scale the larger extent to half the crop width.

Recovered from 0x1400ac560; checked numerically against the original routine.
"""
    return tuple(_d1.normalize_reference_shape(*_points(points), crop_width))


def fit_similarity(source, target):
    """Fit source→target 2×3 similarity using recovered 0x1400dac40 math.

Numerical agreement is checked against original code; reduction order can
change rounding. Degenerate/non-finite inputs are rejected by this API.
"""
    return _matrix(_d1.fit_similarity(*_points(source), *_points(target)), (2, 3))


def inverse_transform_points(points, transform):
    """Map crop coordinates back through a non-singular image→crop affine."""
    return _matrix(_d1.inverse_transform_points(*_points(points), *_points(transform)), (-1, 2))


def compose_affine(first, second):
    """Compose as first @ second, the order observed at 0x1400db330."""
    return _matrix(_d1.compose_affine(*_points(first), *_points(second)), (2, 3))


def warp_grayscale(pixels, image_width, image_height, transform, crop_width=96, crop_height=96):
    """Recovered D1 grayscale affine crop with fixed-point bilinear sampling.

Image→crop transform is inverted in double precision for resampling. Samples
use a 1/32-pixel grid, with separately rounded 10-bit row/column contributions,
constant-zero borders and rounded integer pixel output.
"""
    return _d1.warp_grayscale(pixels, image_width, image_height, *_points(transform), crop_width, crop_height)


def crop_transform(normalized_reference, bounding_box, roll_degrees=0.0, compensate_roll=True):
    """Build image→96×96 crop transform from an externally supplied face box.

The box is integer (x,y,width,height). Roll compensation follows the consumer's
strict >5-degree threshold. Face detection and temporal roll estimation are
not performed here.
"""
    data = _d1.crop_transform(*_points(normalized_reference), tuple(bounding_box), roll_degrees, compensate_roll)
    return _matrix(data, (2, 3))


@dataclass(frozen=True)
class D1Landmarks:
    image_points: tuple
    normalized_points: tuple
    quality: tuple
    image_to_crop: tuple


class D1LandmarkModel:
    """Linux CPU landmark component for caller-supplied grayscale face boxes.

Requires the OpenVINO C library (see docs/rust.md) and NumPy. Does not detect faces,
acquire USB frames, estimate pose, update temporal roll or publish tracking to games.
"""
    def __init__(self, model_directory, raw_reference):
        self._native = _d1.D1LandmarkModel(model_directory, *_points(raw_reference))
        self.reference = tuple(self._native.reference)

    def predict_frame(self, pixels, width, height, bounding_box, roll_degrees=0.0, compensate_roll=True):
        return landmarks_of(self._native.predict_frame(pixels, width, height, tuple(bounding_box), roll_degrees,
                                                       compensate_roll))


def landmarks_of(native):
    """D1Landmarks from the native (image, normalized, quality, image_to_crop) tuple."""
    image, normalized, quality, transform = native
    return D1Landmarks(tuple(image), tuple(normalized), tuple(quality), tuple(tuple(row) for row in transform))
