//! The face finder of the VisionSDK detector 0x14007be00 for grayscale frames.
//!
//! The frame is letterboxed into the network's 256×192 input with the 8-bit bilinear
//! resize of 0x1400b4940, the network's box offsets are decoded against the priors of
//! 0x14007b260 (0x14007bab0), suppressed greedily by overlap (0x14007af20) and mapped to
//! square frame boxes filtered by size and region. All of it is float32 arithmetic in the
//! original's order; Python's `max`, `min` and `round` semantics are kept where the
//! reconstruction used them.

use std::cmp::Ordering;
use std::path::Path;

use crate::d1::PIXEL_SCALE;
use crate::error::{Result, value};
use crate::numeric::{py_exp, py_max, py_min, py_round};
use crate::openvino::{Network, Runtime};

pub const INPUT_WIDTH: usize = 256;
pub const INPUT_HEIGHT: usize = 192;
const STRIDES: [f32; 4] = [8.0, 16.0, 32.0, 64.0];
const ANCHOR_SIZES: [&[f32]; 4] = [&[10.0, 16.0, 24.0], &[32.0, 48.0], &[64.0, 96.0], &[128.0, 192.0, 256.0]];
const CENTER_VARIANCE: f32 = 0.100_000_001_490_116_12_f64 as f32;
const SIZE_VARIANCE: f32 = 0.200_000_002_980_232_24_f64 as f32;
const NMS_OVERLAP: f32 = 0.400_000_005_960_464_5_f64 as f32;
const PI: f32 = std::f32::consts::PI;
const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
const MIN_SCORE: f32 = 0.019_999_999_552_965_164_f64 as f32;
const HEIGHT_SCALE: f32 = 0.005_208_333_488_553_762_4_f64 as f32;
const WIDTH_SCALE: f32 = 0.003_906_25;
const RESIZE_COEFFICIENT_BITS: u32 = 11;
pub const DEFAULT_SENSITIVITY: f64 = 0.75;
pub const DEFAULT_MIN_FACE: f64 = 0.150_000_005_960_464_48;
pub const DEFAULT_MAX_FACE: f64 = 1.0;

pub const RESIZE_MESSAGE: &str = "Expected a nonempty 2D grayscale image and positive size";

/// Placement of the resized frame in the network input, and the frame pixels per input pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Letterbox {
    pub offset_x: i64,
    pub offset_y: i64,
    pub width: i64,
    pub height: i64,
    pub scale: f32,
}

/// A decoded box in network pixels with its score and the network's three angle outputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub score: f32,
    pub angles: [f32; 3],
}

impl Candidate {
    fn corners(&self) -> [f32; 4] {
        [self.x1, self.y1, self.x2, self.y2]
    }
}

/// A square face box in frame pixels plus the network's extra outputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detection {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub score: f32,
    pub angles: [f32; 3],
}

impl Detection {
    /// Integer `(x, y, side, side)` as passed to the D1 landmark crop.
    pub fn rectangle(&self) -> [i64; 4] {
        let side = (f64::from(self.x2) - f64::from(self.x1)) as i64;
        [self.x1 as i64, self.y1 as i64, side, side]
    }
}

/// Detector settings: score sensitivity, accepted face size as a fraction of the shorter
/// frame side, and an optional `(x, y, width, height)` region the box centre must lie in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectOptions {
    pub sensitivity: f64,
    pub min_face: f64,
    pub max_face: f64,
    pub region: Option<[f64; 4]>,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self { sensitivity: DEFAULT_SENSITIVITY, min_face: DEFAULT_MIN_FACE, max_face: DEFAULT_MAX_FACE, region: None }
    }
}

/// Source index and fixed-point weights of every output position along one axis.
fn resize_taps(source: usize, target: usize) -> Vec<(usize, i64, i64)> {
    let scale = 1.0 / (target as f64 / source as f64);
    let one = 1i64 << RESIZE_COEFFICIENT_BITS;
    (0..target)
        .map(|position| {
            let exact = ((position as f64 + 0.5) * scale - 0.5) as f32;
            let floor = f64::from(exact).floor() as i64;
            let (index, fraction) = if floor < 0 {
                (0, 0.0)
            } else if floor >= source as i64 - 1 {
                (source as i64 - 1, 0.0)
            } else {
                (floor, exact - floor as f32)
            };
            let first = ((1.0 - fraction) * one as f32).round_ties_even() as i64;
            (index as usize, first, one - first)
        })
        .collect()
}

/// The 8-bit bilinear resize of 0x1400b4940 (mode 1) with 11-bit fixed-point taps.
pub fn resize_bilinear(pixels: &[u8], columns: usize, rows: usize, width: usize, height: usize) -> Result<Vec<u8>> {
    if columns == 0 || rows == 0 || width == 0 || height == 0 || pixels.len() != columns * rows {
        return value(RESIZE_MESSAGE);
    }
    let x_taps = resize_taps(columns, width);
    let y_taps = resize_taps(rows, height);
    let horizontal: Vec<i64> = pixels
        .chunks_exact(columns)
        .flat_map(|row| {
            x_taps.iter().map(move |&(x, first, second)| {
                i64::from(row[x]) * first + i64::from(row[(x + 1).min(columns - 1)]) * second
            })
        })
        .collect();
    let rounding = 1i64 << (2 * RESIZE_COEFFICIENT_BITS - 1);
    let mut output = Vec::with_capacity(width * height);
    for &(y, first, second) in &y_taps {
        let next = (y + 1).min(rows - 1);
        for column in 0..width {
            let value = horizontal[y * width + column] * first + horizontal[next * width + column] * second + rounding;
            output.push((value >> (2 * RESIZE_COEFFICIENT_BITS)).clamp(0, 255) as u8);
        }
    }
    Ok(output)
}

pub fn letterbox_layout(frame_width: usize, frame_height: usize) -> Result<Letterbox> {
    let (width, height) = (frame_width as f32, frame_height as f32);
    if width / height <= 4.0 / 3.0 {
        let inner = py_round(f64::from(INPUT_HEIGHT as f32 / (height / width)))?;
        Ok(Letterbox {
            offset_x: (INPUT_WIDTH as i64 - inner).div_euclid(2),
            offset_y: 0,
            width: inner,
            height: INPUT_HEIGHT as i64,
            scale: height * HEIGHT_SCALE,
        })
    } else {
        let inner = py_round(f64::from(INPUT_WIDTH as f32 / (width / height)))?;
        Ok(Letterbox {
            offset_x: 0,
            offset_y: (INPUT_HEIGHT as i64 - inner).div_euclid(2),
            width: INPUT_WIDTH as i64,
            height: inner,
            scale: width * WIDTH_SCALE,
        })
    }
}

/// The zero-padded 256×192 network image of a `columns`×`rows` frame, and its layout.
pub fn letterbox(pixels: &[u8], columns: usize, rows: usize) -> Result<(Vec<u8>, Letterbox)> {
    let layout = letterbox_layout(columns, rows)?;
    let size = |v: i64| usize::try_from(v).unwrap_or(0);
    let (width, height) = (size(layout.width), size(layout.height));
    let resized = resize_bilinear(pixels, columns, rows, width, height)?;
    let mut image = vec![0u8; INPUT_WIDTH * INPUT_HEIGHT];
    let (left, top) = (size(layout.offset_x), size(layout.offset_y));
    for (row, line) in resized.chunks_exact(width).enumerate() {
        let start = (top + row) * INPUT_WIDTH + left;
        image[start..start + width].copy_from_slice(line);
    }
    Ok((image, layout))
}

/// Normalised `(cx, cy, w, h)` priors in level, row, column, size order (0x14007b260).
pub fn anchors(width: usize, height: usize) -> Vec<[f32; 4]> {
    let (image_width, image_height) = (width as f32, height as f32);
    let mut priors = Vec::new();
    for (stride, sizes) in STRIDES.into_iter().zip(ANCHOR_SIZES) {
        let rows = (image_height * (1.0 / stride)).ceil() as usize;
        let columns = (image_width * (1.0 / stride)).ceil() as usize;
        for row in 0..rows {
            for column in 0..columns {
                let center_x = (column as f32 + 0.5) * stride * (1.0 / image_width);
                let center_y = (row as f32 + 0.5) * stride * (1.0 / image_height);
                for &size in sizes {
                    priors.push([center_x, center_y, size * (1.0 / image_width), (1.0 / image_height) * size]);
                }
            }
        }
    }
    priors
}

/// Candidate boxes in network pixels (0x14007bab0) for every prior scoring above the threshold.
pub fn decode(
    output: &[f32],
    priors: &[[f32; 4]],
    score_threshold: f32,
    width: usize,
    height: usize,
) -> Result<Vec<Candidate>> {
    let count = priors.len();
    if output.len() != count * 9 {
        return value("Output size does not match the anchor count");
    }
    let (image_width, image_height) = (width as f32, height as f32);
    let mut boxes = Vec::new();
    for (index, prior) in priors.iter().enumerate() {
        let score = output[4 * count + 2 * index + 1];
        if score.partial_cmp(&score_threshold) != Some(Ordering::Greater) {
            continue;
        }
        let offset = &output[4 * index..4 * index + 4];
        let extra = &output[6 * count + 3 * index..6 * count + 3 * index + 3];
        let center_x = offset[0] * CENTER_VARIANCE * prior[2] + prior[0];
        let center_y = offset[1] * CENTER_VARIANCE * prior[3] + prior[1];
        let grow = |v: f32| -> Result<f32> { Ok(py_exp(f64::from(v * SIZE_VARIANCE))? as f32) };
        let half_width = grow(offset[2])? * prior[2] * 0.5;
        let half_height = grow(offset[3])? * prior[3] * 0.5;
        boxes.push(Candidate {
            x1: py_max((center_x - half_width) * image_width, 0.0),
            y1: py_max((center_y - half_height) * image_height, 0.0),
            x2: py_min((half_width + center_x) * image_width, image_width),
            y2: py_min((half_height + center_y) * image_height, image_height),
            score,
            angles: [extra[0] * PI - HALF_PI, HALF_PI - extra[1] * PI, extra[2] * PI - HALF_PI],
        });
    }
    Ok(boxes)
}

/// Greedy suppression of 0x14007af20 over score-sorted `[x1, y1, x2, y2]` boxes with
/// inclusive pixel areas; the indices of the boxes kept, in order.
pub fn suppress(boxes: &[[f32; 4]]) -> Vec<usize> {
    let mut kept: Vec<usize> = (0..boxes.len()).collect();
    let mut areas: Vec<f32> = boxes.iter().map(|b| ((b[3] - b[1]) + 1.0) * ((b[2] - b[0]) + 1.0)).collect();
    let mut first = 0;
    while first < kept.len() {
        let mut other = first + 1;
        while other < kept.len() {
            let (a, b) = (boxes[kept[first]], boxes[kept[other]]);
            let overlap_w = py_max((py_min(a[2], b[2]) - py_max(a[0], b[0])) + 1.0, 0.0);
            let overlap_h = py_max((py_min(a[3], b[3]) - py_max(a[1], b[1])) + 1.0, 0.0);
            let shared = overlap_h * overlap_w;
            if shared / ((areas[other] + areas[first]) - shared) < NMS_OVERLAP {
                other += 1;
            } else {
                kept.remove(other);
                areas.remove(other);
            }
        }
        first += 1;
    }
    kept
}

/// Square frame-pixel detections, filtered by size fraction and region centre.
pub fn to_frame(
    boxes: &[Candidate],
    layout: &Letterbox,
    frame_width: usize,
    frame_height: usize,
    options: &DetectOptions,
) -> Result<Vec<Detection>> {
    let scale = layout.scale;
    let shortest = frame_width.min(frame_height) as f32;
    let min_face = options.min_face as f32;
    let lowest = py_min(py_max(min_face, 0.0), 1.0);
    let highest = py_max(min_face, py_min(options.max_face as f32, 1.0));
    let mut result = Vec::new();
    for b in boxes {
        let (x1, x2) = (b.x1 - layout.offset_x as f32, b.x2 - layout.offset_x as f32);
        let (y1, y2) = (b.y1 - layout.offset_y as f32, b.y2 - layout.offset_y as f32);
        let radius = py_round(f64::from(((y2 * scale - y1 * scale) + (x2 * scale - x1 * scale)) * 0.25))?;
        let left = (py_round(f64::from((x2 + x1) * scale * 0.5))? - radius) as f32;
        let top = (py_round(f64::from((y2 + y1) * scale * 0.5))? - radius) as f32;
        let side = (radius * 2) as f32;
        if !(shortest * lowest < side && side < shortest * highest) {
            continue;
        }
        if let Some([rx, ry, rw, rh]) = options.region {
            let center_x = py_round(f64::from((side + left + left) * 0.5))? as f64;
            let center_y = py_round(f64::from((side + top + top) * 0.5))? as f64;
            if !(rx < center_x && center_x < rx + rw && ry < center_y && center_y < ry + rh) {
                continue;
            }
        }
        result.push(Detection { x1: left, y1: top, x2: side + left, y2: side + top, score: b.score, angles: b.angles });
    }
    Ok(result)
}

pub fn score_threshold(sensitivity: f64) -> f32 {
    py_max(MIN_SCORE, py_min(1.0, 1.0 - sensitivity as f32))
}

/// Ascending order of a float32 sort key, -0 equal to +0 as Python compares them.
fn ascending(a: f32, b: f32) -> Ordering {
    (a + 0.0).total_cmp(&(b + 0.0))
}

/// The detector after the network (0x14007be00): decoding, score order, suppression, then
/// widest first, as the tracker uses the first detection. Sorts are stable; the original's
/// unstable sort may order exact ties differently.
pub fn detections(
    output: &[f32],
    layout: &Letterbox,
    frame_width: usize,
    frame_height: usize,
    options: &DetectOptions,
) -> Result<Vec<Detection>> {
    let priors = anchors(INPUT_WIDTH, INPUT_HEIGHT);
    let mut candidates = decode(output, &priors, score_threshold(options.sensitivity), INPUT_WIDTH, INPUT_HEIGHT)?;
    candidates.sort_by(|a, b| ascending(-a.score, -b.score));
    let corners: Vec<[f32; 4]> = candidates.iter().map(Candidate::corners).collect();
    let mut kept: Vec<Candidate> = suppress(&corners).into_iter().map(|i| candidates[i]).collect();
    kept.sort_by(|a, b| ascending(-(a.x2 - a.x1), -(b.x2 - b.x1)));
    to_frame(&kept, layout, frame_width, frame_height, options)
}

/// The network input of a letterboxed image: each byte times the runtime's 1/255 constant.
pub fn input_tensor(image: &[u8]) -> Result<Vec<f32>> {
    if image.len() != INPUT_WIDTH * INPUT_HEIGHT {
        return value("Expected a 256×192 grayscale network image");
    }
    Ok(image.iter().map(|&p| f32::from(p) * PIXEL_SCALE).collect())
}

/// The `ff` network.
#[derive(Debug)]
pub struct FaceFinder {
    network: Network,
}

impl FaceFinder {
    pub fn new(runtime: &Runtime, model_directory: &Path) -> Result<Self> {
        Ok(Self { network: runtime.load(model_directory)? })
    }

    /// The raw network outputs for a letterboxed 256×192 image.
    pub fn run(&self, image: &[u8]) -> Result<Vec<f32>> {
        self.network.infer(&input_tensor(image)?)
    }

    /// Faces in a `columns`×`rows` grayscale frame, widest first.
    pub fn detect(
        &self,
        pixels: &[u8],
        columns: usize,
        rows: usize,
        options: &DetectOptions,
    ) -> Result<Vec<Detection>> {
        let (image, layout) = letterbox(pixels, columns, rows)?;
        detections(&self.run(&image)?, &layout, columns, rows, options)
    }
}

#[cfg(test)]
mod tests;
