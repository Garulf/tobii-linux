//! The D1 landmark network's crop, input tensor and output decoding.
//!
//! A caller-supplied face box becomes an image-to-crop similarity (0x1400dac40 fit, roll
//! compensation composed at 0x1400db330), the frame is warped into a 96×96 crop with the
//! fixed-point bilinear sampler of 0x1400b45b0, scaled to float32 (0x1402aa500) and run
//! through the network; its 57 offset pairs are decoded against the normalized initial
//! shape (0x1400ac560) and mapped back to the image through the inverse transform
//! (0x1400db010). Face detection, temporal roll and pose are separate.
//!
//! Matrix values are float32 like the runtime's; where the reconstruction computes in
//! float64 (the inverse, the warp's sampling grid, reductions) it does so in NumPy's order.

use std::path::Path;

use crate::error::{Result, value};
use crate::numeric::{dot, mean_columns, numpy_sum, pack_f32};
use crate::openvino::{Network, Runtime};

pub type Point = [f32; 2];
/// A 2×3 affine transform, rows of float32.
pub type Affine = [[f32; 3]; 2];

pub const CROP_SIZE: usize = 96;
pub const LANDMARKS: usize = 57;
pub const OUTPUTS: usize = 171;
/// The uint8 to float scale stored at runtime RVA 0x474aa0.
pub const PIXEL_SCALE: f32 = 0.003_921_568_859_368_563_f64 as f32;
const CROP_WIDTH: f32 = 96.0;
const ROLL_THRESHOLD_DEGREES: f32 = 5.0;
const DEGREES_TO_RADIANS: f32 = 0.017_453_294_247_388_84_f64 as f32;

pub const OUTPUT_MESSAGE: &str = "Expected 57 initial XY pairs and 171 D1 outputs";
pub const OUTPUT_DATA_MESSAGE: &str = "Unsupported crop or non-finite D1 data";
pub const REFERENCE_MESSAGE: &str = "Expected finite 57×2 D1 reference shape";
pub const POINT_SETS_MESSAGE: &str = "Expected matching finite N×2 point sets";
pub const INVERSE_MESSAGE: &str = "Expected finite N×2 points and 2×3 transform";
pub const COMPOSE_MESSAGE: &str = "Expected finite 2×3 transforms";
pub const TRANSFORM_MESSAGE: &str = "Expected finite 2×3 transform";
pub const CROP_MESSAGE: &str = "Expected finite 57×2 reference and positive integer face box";
pub const WARP_MESSAGE: &str = "Invalid contiguous grayscale image/crop dimensions";

fn finite(points: &[Point]) -> bool {
    points.iter().flatten().all(|v| v.is_finite())
}

fn finite_affine(transform: &Affine) -> bool {
    transform.iter().flatten().all(|v| v.is_finite())
}

/// The decoded network output: 57 crop points and 57 quality values.
#[derive(Debug, Clone, PartialEq)]
pub struct D1Output {
    pub crop_points: Vec<Point>,
    pub quality: Vec<f32>,
}

/// Decodes 57 XY offset pairs plus 57 quality values against the runtime-normalized
/// initial shape (not the raw init_shape.bin coordinates). The points are in the crop.
pub fn decode_output(values: &[f32], normalized_initial_shape: &[Point]) -> Result<D1Output> {
    if normalized_initial_shape.len() != LANDMARKS || values.len() != OUTPUTS {
        return value(OUTPUT_MESSAGE);
    }
    if !values.iter().all(|v| v.is_finite()) || !finite(normalized_initial_shape) {
        return value(OUTPUT_DATA_MESSAGE);
    }
    let offset = |v: f32| -> Result<f64> {
        let scaled = pack_f32(f64::from(v) * f64::from(CROP_WIDTH))?;
        Ok(f64::from(pack_f32(f64::from(scaled) * 0.5)?))
    };
    let crop_points = normalized_initial_shape
        .iter()
        .enumerate()
        .map(|(i, base)| {
            Ok([
                pack_f32(f64::from(base[0]) + offset(values[2 * i])?)?,
                pack_f32(f64::from(base[1]) + offset(values[2 * i + 1])?)?,
            ])
        })
        .collect::<Result<_>>()?;
    Ok(D1Output { crop_points, quality: values[2 * LANDMARKS..].to_vec() })
}

/// Image XY normalized by the image size, as the FDP writer stores them after the
/// inverse warp: each quotient computed in double and rounded to float32.
pub fn normalize_image_points(points: &[Point], image_width: f64, image_height: f64) -> Result<Vec<Point>> {
    if !(image_width > 0.0 && image_width <= 32768.0 && image_height > 0.0 && image_height <= 32768.0) {
        return value("Invalid image dimensions");
    }
    let result = points
        .iter()
        .map(|&[x, y]| Ok([pack_f32(f64::from(x) / image_width)?, pack_f32(f64::from(y) / image_height)?]))
        .collect::<Result<Vec<Point>>>()?;
    if !finite(&result) {
        return value("Non-finite image point");
    }
    Ok(result)
}

/// The network input of a contiguous 96×96 grayscale crop: each byte times the runtime's
/// 1/255 constant, in float32.
pub fn prepare_crop_tensor(pixels: &[u8]) -> Result<Vec<f32>> {
    if pixels.len() != CROP_SIZE * CROP_SIZE {
        return value("Expected exactly 9216 grayscale bytes");
    }
    Ok(pixels.iter().map(|&p| f32::from(p) * PIXEL_SCALE).collect())
}

/// Centers the reference shape on its per-axis means and scales its larger extent to half
/// the crop width (0x1400ac560).
pub fn normalize_reference_shape(points: &[Point]) -> Result<Vec<Point>> {
    if points.len() != LANDMARKS || !finite(points) {
        return value(REFERENCE_MESSAGE);
    }
    let (minimum, maximum) = bounds(points);
    let extent = (maximum[0] - minimum[0]).max(maximum[1] - minimum[1]);
    if extent <= 0.0 {
        return value("Degenerate reference shape");
    }
    let mean = mean_columns(points);
    let factor = (0.5 / extent) * CROP_WIDTH;
    let half = CROP_WIDTH * 0.5;
    Ok(points.iter().map(|p| [(p[0] - mean[0]) * factor + half, (p[1] - mean[1]) * factor + half]).collect())
}

fn bounds(points: &[Point]) -> (Point, Point) {
    let mut minimum = points[0];
    let mut maximum = points[0];
    for point in points {
        for axis in 0..2 {
            minimum[axis] = minimum[axis].min(point[axis]);
            maximum[axis] = maximum[axis].max(point[axis]);
        }
    }
    (minimum, maximum)
}

/// The source to target similarity of 0x1400dac40: scale and rotation from the centered
/// dot and cross products, translation between the means.
pub fn fit_similarity(source: &[Point], target: &[Point]) -> Result<Affine> {
    if source.len() != target.len() || source.len() < 2 || !finite(source) || !finite(target) {
        return value(POINT_SETS_MESSAGE);
    }
    let mean_source = mean_columns(source);
    let mean_target = mean_columns(target);
    let x: Vec<Point> = source.iter().map(|p| [p[0] - mean_source[0], p[1] - mean_source[1]]).collect();
    let y: Vec<Point> = target.iter().map(|p| [p[0] - mean_target[0], p[1] - mean_target[1]]).collect();
    let squares: Vec<f64> = x.iter().flatten().map(|&v| f64::from(v) * f64::from(v)).collect();
    let norm = numpy_sum(&squares).sqrt() as f32;
    if norm == 0.0 {
        return value("Degenerate source shape");
    }
    let reciprocal = 1.0 / (norm * norm);
    let products: Vec<f64> =
        x.iter().flatten().zip(y.iter().flatten()).map(|(&a, &b)| f64::from(a) * f64::from(b)).collect();
    let dot_product = numpy_sum(&products) as f32;
    let terms: Vec<f32> = x.iter().zip(&y).map(|(a, b)| a[0] * b[1] - a[1] * b[0]).collect();
    let blocks = terms.len() / 4 * 4;
    let mut cross = 0.0f32;
    for t in terms[..blocks].as_chunks::<4>().0 {
        cross = (((t[1] + t[0]) + cross) + t[2]) + t[3];
    }
    for &term in &terms[blocks..] {
        cross += term;
    }
    let a = dot_product * reciprocal;
    let b = cross * reciprocal;
    let linear = [[f64::from(a), f64::from(-b)], [f64::from(b), f64::from(a)]];
    let mean = mean_source.map(f64::from);
    let translation = |row: usize| mean_target[row] - dot(&linear[row], mean) as f32;
    Ok([[a, -b, translation(0)], [b, a, translation(1)]])
}

/// The inverse of the linear part in double precision and its offset, from a float32
/// transform whose determinant has been computed (by the caller's rule) and checked.
fn inverse(transform: &Affine, determinant: f64) -> ([[f64; 2]; 2], [f64; 2]) {
    let l = transform.map(|row| row.map(f64::from));
    let inverse = [[l[1][1] / determinant, -l[0][1] / determinant], [-l[1][0] / determinant, l[0][0] / determinant]];
    let shift = [l[0][2], l[1][2]];
    let offset = [0, 1].map(|j| dot(&[-inverse[j][0], -inverse[j][1]], shift));
    (inverse, offset)
}

/// Maps crop points back through a non-singular image to crop transform (0x1400db010).
/// The determinant is rounded to float32 as the original's float matrix branch does; the
/// inverse is computed in double and stored as float32 before the product.
pub fn inverse_transform_points(points: &[Point], transform: &Affine) -> Result<Vec<Point>> {
    if !finite(points) || !finite_affine(transform) {
        return value(INVERSE_MESSAGE);
    }
    let f = transform;
    let determinant = f64::from(f[0][0] * f[1][1] - f[0][1] * f[1][0]);
    if determinant == 0.0 {
        return value("Singular transform");
    }
    let (inverse, offset) = inverse(transform, determinant);
    let inverse = inverse.map(|row| row.map(|v| f64::from(v as f32)));
    let offset = offset.map(|v| f64::from(v as f32));
    Ok(points
        .iter()
        .map(|p| {
            let p = p.map(f64::from);
            [0, 1].map(|j| (dot(&p, inverse[j]) + offset[j]) as f32)
        })
        .collect())
}

/// `first @ second` as 3×3 homogeneous matrices in double, the order observed at 0x1400db330.
pub fn compose_affine(first: &Affine, second: &Affine) -> Result<Affine> {
    if !finite_affine(first) || !finite_affine(second) {
        return value(COMPOSE_MESSAGE);
    }
    let homogeneous = |m: &Affine| [m[0].map(f64::from), m[1].map(f64::from), [0.0, 0.0, 1.0]];
    let (a, b) = (homogeneous(first), homogeneous(second));
    Ok([0, 1].map(|i| [0, 1, 2].map(|j| dot(&a[i], b.iter().map(|row| row[j])) as f32)))
}

fn valid_size(size: usize) -> bool {
    (1..=32768).contains(&size)
}

/// The recovered grayscale affine crop: the image to crop transform inverted in double,
/// samples on a 1/32 pixel grid from separately rounded 10-bit row and column
/// contributions, bilinear weights, zero outside the image and rounded output.
pub fn warp_grayscale(
    pixels: &[u8],
    image_width: usize,
    image_height: usize,
    transform: &Affine,
    crop_width: usize,
    crop_height: usize,
) -> Result<Vec<u8>> {
    if ![image_width, image_height, crop_width, crop_height].into_iter().all(valid_size)
        || pixels.len() != image_width * image_height
    {
        return value(WARP_MESSAGE);
    }
    if !finite_affine(transform) {
        return value(TRANSFORM_MESSAGE);
    }
    let l = transform.map(|row| row.map(f64::from));
    let determinant = l[0][0] * l[1][1] - l[0][1] * l[1][0];
    if determinant == 0.0 {
        return value("Singular transform");
    }
    let (inverse, offset) = inverse(transform, determinant);
    let fixed = |v: f64| crate::numeric::cvttsd2si64((v * 1024.0).round_ties_even());
    let columns: Vec<[i64; 2]> = (0..crop_width).map(|x| [0, 1].map(|j| fixed(inverse[j][0] * x as f64))).collect();
    let rows: Vec<[i64; 2]> =
        (0..crop_height).map(|y| [0, 1].map(|j| fixed(inverse[j][1] * y as f64 + offset[j]))).collect();
    let (width, height) = (image_width as i64, image_height as i64);
    let sample = |x: i64, y: i64| -> i64 {
        if (0..width).contains(&x) && (0..height).contains(&y) {
            i64::from(pixels[(y * width + x) as usize])
        } else {
            0
        }
    };
    let mut output = Vec::with_capacity(crop_width * crop_height);
    for row in &rows {
        for column in &columns {
            let position = [0, 1].map(|j| column[j].wrapping_add(row[j]).wrapping_add(16) >> 5);
            let [ix, iy] = position.map(|p| (p >> 5).clamp(-32768, 32767));
            let [fx, fy] = position.map(|p| p & 31);
            let total = sample(ix, iy) * ((32 - fx) * (32 - fy))
                + sample(ix + 1, iy) * (fx * (32 - fy))
                + sample(ix, iy + 1) * ((32 - fx) * fy)
                + sample(ix + 1, iy + 1) * (fx * fy);
            output.push(((total + 512) >> 10).clamp(0, 255) as u8);
        }
    }
    Ok(output)
}

/// The image to 96×96 crop transform for a face box `(x, y, width, height)`: the reference
/// fitted into the box, then rotated about the crop center when roll compensation is on and
/// the roll exceeds the consumer's strict five-degree threshold.
pub fn crop_transform(
    normalized_reference: &[Point],
    bounding_box: [i64; 4],
    roll_degrees: f64,
    compensate_roll: bool,
) -> Result<Affine> {
    let [x, y, width, height] = bounding_box;
    if normalized_reference.len() != LANDMARKS
        || !finite(normalized_reference)
        || width.min(height) <= 0
        || !roll_degrees.is_finite()
    {
        return value(CROP_MESSAGE);
    }
    let (minimum, maximum) = bounds(normalized_reference);
    let extent = [maximum[0] - minimum[0], maximum[1] - minimum[1]];
    if extent.iter().any(|&e| e <= 0.0) {
        return value("Degenerate reference shape");
    }
    let scale = (height as f32 / extent[1] + width as f32 / extent[0]) * 0.5;
    let mut fitted: Vec<Point> = normalized_reference.iter().map(|p| [p[0] * scale, p[1] * scale]).collect();
    let (low, high) = bounds(&fitted);
    let center = [width as f32 * 0.5 + x as f32, height as f32 * 0.5 + y as f32];
    let shift = [0, 1].map(|axis| center[axis] - ((high[axis] - low[axis]) * 0.5 + low[axis]));
    for point in &mut fitted {
        point[0] += shift[0];
        point[1] += shift[1];
    }
    let transform = fit_similarity(&fitted, normalized_reference)?;
    let roll = roll_degrees as f32;
    if !(compensate_roll && roll.abs() > ROLL_THRESHOLD_DEGREES) {
        return Ok(transform);
    }
    let angle = roll * DEGREES_TO_RADIANS;
    if !angle.is_finite() {
        return value("math domain error");
    }
    let sine = f64::from(angle).sin() as f32;
    let cosine = f64::from(angle).cos() as f32;
    let half = CROP_WIDTH * 0.5;
    let (s, c) = (sine * half, cosine * half);
    let rotation = [[cosine, sine, (half - c) - s], [-sine, cosine, (s + half) - c]];
    compose_affine(&rotation, &transform)
}

/// Landmarks of one frame: image and image-normalized points, quality, and the image to
/// crop transform used.
#[derive(Debug, Clone, PartialEq)]
pub struct D1Landmarks {
    pub image_points: Vec<Point>,
    pub normalized_points: Vec<Point>,
    pub quality: Vec<f32>,
    pub image_to_crop: Affine,
}

/// The D1 network with its normalized reference shape, for caller-supplied grayscale
/// frames and face boxes.
#[derive(Debug)]
pub struct D1LandmarkModel {
    reference: Vec<Point>,
    network: Network,
}

impl D1LandmarkModel {
    /// The model directory's network and the raw init_shape.bin points.
    pub fn new(runtime: &Runtime, model_directory: &Path, raw_reference: &[Point]) -> Result<Self> {
        let reference = normalize_reference_shape(raw_reference)?;
        Ok(Self { reference, network: runtime.load(model_directory)? })
    }

    pub fn reference(&self) -> &[Point] {
        &self.reference
    }

    pub fn predict_frame(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
        bounding_box: [i64; 4],
        roll_degrees: f64,
        compensate_roll: bool,
    ) -> Result<D1Landmarks> {
        let transform = crop_transform(&self.reference, bounding_box, roll_degrees, compensate_roll)?;
        let crop = warp_grayscale(pixels, width, height, &transform, CROP_SIZE, CROP_SIZE)?;
        let output = self.network.infer(&prepare_crop_tensor(&crop)?)?;
        let decoded = decode_output(&output, &self.reference)?;
        let image_points = inverse_transform_points(&decoded.crop_points, &transform)?;
        let normalized_points = normalize_image_points(&image_points, width as f64, height as f64)?;
        Ok(D1Landmarks { image_points, normalized_points, quality: decoded.quality, image_to_crop: transform })
    }
}

#[cfg(test)]
mod tests;
