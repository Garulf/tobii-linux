//! Recovered LBF stages: pixel-pair tree traversal, feature gathering and the stage
//! predictor, with explicit buffer geometry. Not a camera tracker.

use tobii_models::lbf::{Forest, LbfModel, Matrix};

use crate::error::{Result, invalid};

/// An 8-bit image buffer indexed `axis0 * stride + axis1`.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    pub pixels: &'a [u8],
    pub rows: i64,
    pub columns: i64,
    pub stride: i64,
}

impl Image<'_> {
    fn check(&self) -> Result<()> {
        if !(1..=32768).contains(&self.rows) || !(1..=self.stride).contains(&self.columns) || self.stride > 32768 {
            return invalid("Invalid image dimensions");
        }
        if (self.pixels.len() as i64) < (self.rows - 1) * self.stride + self.columns {
            return invalid("Invalid image buffer or axis sign");
        }
        Ok(())
    }
}

/// Native `cvttss2si` of a value first rounded to float32.
fn truncated(value: f64) -> i64 {
    (value as f32).trunc() as i64
}

/// Per-tree leaf indices using zero-initialized native thresholds.
///
/// Axes follow buffer indexing. Runtime crop/axis mapping and any later threshold mutation
/// are not established. Inputs are bounded to avoid native int32 overflow behaviour outside
/// observed image dimensions. `mirrored` negates the axis-1 offsets.
pub fn forest_leaves(
    forest: &Forest,
    image: Image<'_>,
    center0: f64,
    center1: f64,
    scale: f64,
    mirrored: bool,
) -> Result<Vec<usize>> {
    image.check()?;
    if [center0, center1, scale].iter().any(|x| !x.is_finite() || x.abs() > 32768.0) || scale < 0.0 {
        return invalid("Unsupported center/scale");
    }
    let (c0, c1, distance) = (truncated(center0), truncated(center1), truncated(scale));
    let sign = if mirrored { -1 } else { 1 };
    let pixel = |row: i64, column: i64| {
        let row = row.clamp(0, image.rows - 1);
        let column = column.clamp(0, image.columns - 1);
        i32::from(image.pixels[(row * image.stride + column) as usize])
    };
    let mut leaves = Vec::with_capacity(forest.trees.len());
    for tree in &forest.trees {
        if tree.depth > 20 || tree.payload.len() != 4 << tree.depth {
            return invalid("Invalid tree payload");
        }
        let mut node = 0usize;
        for _ in 0..tree.depth {
            let offsets = &tree.payload[node * 4..node * 4 + 4];
            let [a0, a1, b0, b1] = [0, 1, 2, 3].map(|i| i64::from(offsets[i] as i8));
            let a = pixel(c0 + ((a0 * distance) >> 8), c1 + ((a1 * distance * sign) >> 8));
            let b = pixel(c0 + ((b0 * distance) >> 8), c1 + ((b1 * distance * sign) >> 8));
            node = 2 * node + if a - b > 0 { 2 } else { 1 };
        }
        leaves.push(node + 1 - (1 << tree.depth));
    }
    Ok(leaves)
}

/// The sparse feature indices of one stage and its scale, 0x1400debc0.
///
/// `centers` are `(axis0, axis1)` per model point; points without forests are skipped.
pub fn gather_features(
    model: &LbfModel,
    stage: usize,
    image: Image<'_>,
    centers: &[[f64; 2]],
    scale: f32,
    mirrored: bool,
) -> Result<(Vec<usize>, f32)> {
    if centers.len() != model.points.len() || stage >= model.stages.len() {
        return invalid("Landmark count or stage mismatch");
    }
    let Some(first) = model.points.iter().find(|p| !p.submodels.is_empty()) else {
        return invalid("No landmark has LBF forests");
    };
    let forest_at = |forests: &[Forest]| -> Result<Forest> {
        forests.get(stage).cloned().ok_or_else(|| crate::error::Error::Value("Missing stage forest".into()))
    };
    let stage_scale = scale * forest_at(&first.submodels)?.scale_factor();
    let mut features = vec![];
    let mut base = 0;
    for (point, center) in model.points.iter().zip(centers) {
        if point.submodels.is_empty() {
            continue;
        }
        let forest = forest_at(&point.submodels)?;
        let leaves = forest_leaves(&forest, image, center[0], center[1], f64::from(stage_scale), mirrored)?;
        for (leaf, tree) in leaves.iter().zip(&forest.trees) {
            features.push(base + leaf);
            base += 1 << tree.depth;
        }
    }
    if model.stages[stage].first().map(|m| m.inputs) != Some(base) {
        return invalid("Feature extent does not match predictor");
    }
    Ok((features, stage_scale))
}

/// One stage: a sparse int8 first matrix, tanh hidden activations and float32 dense
/// matrices with the native multiply/add order.
pub fn predict_stage(matrices: &[Matrix], features: &[usize]) -> Result<Vec<f32>> {
    let Some(first) = matrices.first() else {
        return invalid("Stage has no matrices");
    };
    if features.iter().any(|&f| f >= first.inputs) {
        return invalid("Feature index outside predictor");
    }
    let (weights, scales) = first.quantize()?;
    let outputs = first.outputs;
    let mut values: Vec<f32> = (0..outputs)
        .map(|o| {
            let sum = features.iter().fold(0i32, |total, &f| total.wrapping_add(i32::from(weights[f * outputs + o])));
            sum as f32 / scales[o] + first.biases[o]
        })
        .collect();
    for matrix in &matrices[1..] {
        if values.len() != matrix.inputs {
            return invalid("Disconnected stage matrices");
        }
        let activations: Vec<f32> = values.iter().map(|v| v.tanh()).collect();
        let mut output = matrix.biases.clone();
        for (i, activation) in activations.iter().enumerate() {
            for (o, value) in output.iter_mut().enumerate() {
                *value += matrix.weights[i * matrix.outputs + o] * activation;
            }
        }
        values = output;
    }
    if !values.iter().all(|v| v.is_finite()) {
        return invalid("Non-finite predictor result");
    }
    Ok(values)
}

/// All stages from caller-supplied initial landmarks `(axis0, axis1)` and scale.
pub fn refine_landmarks(
    model: &LbfModel,
    image: Image<'_>,
    centers: &[[f64; 2]],
    scale: f32,
    mirrored: bool,
) -> Result<Vec<[f32; 2]>> {
    let mut current: Vec<[f32; 2]> = centers.iter().map(|c| c.map(|v| v as f32)).collect();
    let sign = if mirrored { -1.0 } else { 1.0 };
    for (stage, matrices) in model.stages.iter().enumerate() {
        let centers: Vec<[f64; 2]> = current.iter().map(|c| c.map(f64::from)).collect();
        let (features, stage_scale) = gather_features(model, stage, image, &centers, scale, mirrored)?;
        let delta = predict_stage(matrices, &features)?;
        if delta.len() != 2 * current.len() {
            return invalid("Landmark output count mismatch");
        }
        for (i, [a, b]) in current.iter_mut().enumerate() {
            *a += delta[2 * i] * stage_scale;
            *b += (delta[2 * i + 1] * sign) * stage_scale;
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tobii_models::lbf::Tree;

    fn forest(offsets: [i8; 4]) -> Forest {
        let mut payload: Vec<u8> = offsets.iter().map(|&o| o as u8).collect();
        payload.extend([0; 4]);
        Forest { first: 0, scale_bits: 0, trees: vec![Tree { marker: 0, word: 0, depth: 1, payload }] }
    }

    fn image(pixels: &[u8], rows: i64, columns: i64) -> Image<'_> {
        Image { pixels, rows, columns, stride: columns }
    }

    #[test]
    fn pixel_difference_strict_branch_and_axis_mirror() {
        let tree = forest([0, -1, 0, 1]);
        let pixels = [9, 5, 1];
        assert_eq!(forest_leaves(&tree, image(&pixels, 1, 3), 0.0, 1.0, 256.0, false).unwrap(), [1]);
        assert_eq!(forest_leaves(&tree, image(&pixels, 1, 3), 0.0, 1.0, 256.0, true).unwrap(), [0]);
        assert_eq!(forest_leaves(&tree, image(&pixels, 1, 3), 0.0, 1.0, 0.0, false).unwrap(), [0]);
    }

    #[test]
    fn clamps_negative_offsets_and_rejects_short_buffer() {
        let tree = forest([-128, -128, 127, 127]);
        assert_eq!(forest_leaves(&tree, image(&[0, 1, 2, 3], 2, 2), 0.0, 0.0, 32.0, false).unwrap(), [0]);
        assert!(forest_leaves(&tree, image(&[0, 1, 2], 2, 2), 0.0, 0.0, 32.0, false).is_err());
    }
}
