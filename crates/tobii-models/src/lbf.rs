//! The observed version-4 `NN/fa.lbf`, `NN/fc.lbf` and `LBF/lv` layouts.
//!
//! Based on the installed runtime's file readers; not a landmark predictor. Tree payloads
//! keep their original bytes. Coefficient matrices keep the decoded float32 file values
//! bit for bit, before the runtime's int8 quantization of each stage's first matrix.

use std::path::Path;

use crate::bytes::{f32_at, i32_at, read_file, u32_at};
use crate::error::{Result, format};

const VERSION: u8 = 4;
const TREE_MARKERS: [u32; 2] = [0xb00b5, 0xc00b5];
const MAX_TREE_DEPTH: u32 = 20;
const CODEBOOK: usize = 16;
const MAX_MATRIX: usize = 10_000_000;
const NN_MATRICES_PER_STAGE: i32 = 3;

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        if size > self.data.len() - self.offset {
            return format(format!("Truncated LBF at {}", self.offset));
        }
        let start = self.offset;
        self.offset += size;
        Ok(&self.data[start..self.offset])
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32_at(self.take(4)?, 0).unwrap())
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32_at(self.take(4)?, 0).unwrap())
    }

    fn count(&mut self, maximum: i32) -> Result<usize> {
        let value = self.i32()?;
        if !(1..=maximum).contains(&value) {
            return format("Unsupported LBF count");
        }
        Ok(value as usize)
    }

    fn floats(&mut self, count: usize) -> Result<Vec<f32>> {
        let data = self.take(count * 4)?;
        let values: Vec<f32> = (0..count).map(|i| f32_at(data, 4 * i).unwrap()).collect();
        if !values.iter().all(|v| v.is_finite()) {
            return format("Non-finite LBF value");
        }
        Ok(values)
    }

    fn end(&self) -> Result<()> {
        if self.offset != self.data.len() {
            return format(format!("Unexpected trailing LBF bytes: {}", self.data.len() - self.offset));
        }
        Ok(())
    }
}

/// One pixel-pair tree: `4 * 2**depth` payload bytes of signed offsets `(a0, a1, b0, b1)`
/// per node. `word` repeats `marker` unless the marker is one of the two tagged forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub marker: u32,
    pub word: u32,
    pub depth: u32,
    pub payload: Vec<u8>,
}

/// A forest's leading word, the float32 bits of its stage scale factor (read as int32),
/// and its trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forest {
    pub first: i32,
    pub scale_bits: i32,
    pub trees: Vec<Tree>,
}

impl Forest {
    pub fn scale_factor(&self) -> f32 {
        f32::from_bits(self.scale_bits as u32)
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let first = reader.i32()?;
        let count = reader.count(10000)?;
        let scale_bits = reader.i32()?;
        let mut trees = Vec::with_capacity(count);
        for _ in 0..count {
            let marker = reader.u32()?;
            let word = if TREE_MARKERS.contains(&marker) { reader.u32()? } else { marker };
            let depth = reader.u32()?;
            if depth > MAX_TREE_DEPTH {
                return format("Unsupported tree extent");
            }
            let payload = reader.take(4 << depth)?.to_vec();
            trees.push(Tree { marker, word, depth, payload });
        }
        Ok(Self { first, scale_bits, trees })
    }
}

/// A coefficient matrix with `weights[input * outputs + output]` and one bias per output
/// (zeros when the file stores none). Compressed matrices also keep each output's largest
/// absolute codebook entry, which sets the int8 scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    pub inputs: usize,
    pub outputs: usize,
    pub biases: Vec<f32>,
    pub weights: Vec<f32>,
    pub has_bias: bool,
    pub codebook_max_abs: Option<Vec<f32>>,
}

impl Matrix {
    fn read(reader: &mut Reader<'_>, compressed: bool, has_bias: bool) -> Result<Self> {
        let (inputs, outputs) = (reader.count(100000)?, reader.count(100000)?);
        if inputs * outputs > MAX_MATRIX {
            return format("Excessive matrix extent");
        }
        if !compressed {
            let biases = if has_bias { reader.floats(outputs)? } else { vec![0.0; outputs] };
            let weights = reader.floats(inputs * outputs)?;
            return Ok(Self { inputs, outputs, biases, weights, has_bias, codebook_max_abs: None });
        }
        let mut maxima = Vec::with_capacity(outputs);
        let mut biases = vec![0.0; outputs];
        let mut weights = vec![0.0; inputs * outputs];
        let values = inputs + usize::from(has_bias);
        for column in 0..outputs {
            let codebook = reader.floats(CODEBOOK)?;
            maxima.push(codebook.iter().map(|v| v.abs()).fold(0.0, f32::max));
            let packed = reader.take(values.div_ceil(2))?;
            for j in 0..values {
                let value = codebook[usize::from((packed[j / 2] >> ((j % 2) * 4)) & 15)];
                match j.checked_sub(usize::from(has_bias)) {
                    None => biases[column] = value,
                    Some(input) => weights[input * outputs + column] = value,
                }
            }
        }
        Ok(Self { inputs, outputs, biases, weights, has_bias, codebook_max_abs: Some(maxima) })
    }

    pub fn shape(&self) -> (usize, usize) {
        (self.inputs, self.outputs)
    }

    /// The runtime's first-matrix int8 branch (0x1400ddbb0) for finite positive ranges:
    /// `(weights, per-output scales)`.
    ///
    /// The scale uses the entire codebook, including entries no weight uses. Division and
    /// multiplication each round to float32 before truncation toward zero, as the scalar
    /// SSE path does; values are range-checked before the exact `as` conversions.
    pub fn quantize(&self) -> Result<(Vec<i8>, Vec<f32>)> {
        let Some(maxima) = &self.codebook_max_abs else {
            return format("Quantization requires nonzero finite codebooks");
        };
        if maxima.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return format("Quantization requires nonzero finite codebooks");
        }
        let scales: Vec<f32> = maxima.iter().map(|max| (127.0f32 / max).trunc()).collect();
        if scales.iter().any(|s| !s.is_finite()) {
            return format("Quantization out of observed range");
        }
        let mut quantized = Vec::with_capacity(self.weights.len());
        for (i, weight) in self.weights.iter().enumerate() {
            let value = (weight * scales[i % self.outputs]).trunc();
            if !(-127.0..=127.0).contains(&value) {
                return format("Quantization out of observed range");
            }
            quantized.push(value as i8);
        }
        Ok((quantized, scales))
    }
}

/// One landmark of an NN model: identifier, presence, initial coordinates and one forest
/// per stage when present.
#[derive(Debug, Clone, PartialEq)]
pub struct LbfPoint {
    pub id: (i32, i32),
    pub xy: [f32; 2],
    pub submodels: Vec<Forest>,
}

/// `NN/fa.lbf` or `NN/fc.lbf`, readers 0x1400ac7d0, 0x1400de8d0 and 0x1400dd1b0.
#[derive(Debug, Clone, PartialEq)]
pub struct LbfModel {
    pub version: u8,
    pub points: Vec<LbfPoint>,
    pub tree_count: usize,
    pub forest_end: usize,
    pub stages: Vec<Vec<Matrix>>,
    pub source_bytes: usize,
}

fn presence(value: i32, message: &str) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => format(message),
    }
}

impl LbfModel {
    /// Only the observed version-4 layout with three matrices per stage, requiring EOF.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = Reader { data, offset: 0 };
        let version = reader.u8()?;
        if version != VERSION {
            return format("Only observed version-4 LBF is supported");
        }
        let landmarks = reader.count(1000)?;
        let stage_count = reader.count(100)?;
        let matrix_count = reader.count(100)?;
        if matrix_count != NN_MATRICES_PER_STAGE as usize {
            return format("Unsupported LBF layout (this reader excludes LBF/lv)");
        }
        let mut points = Vec::with_capacity(landmarks);
        let mut tree_count = 0;
        for _ in 0..landmarks {
            let id = (reader.i32()?, reader.i32()?);
            let present = presence(reader.i32()?, "Invalid LBF presence flag")?;
            let xy = reader.floats(2)?;
            let mut submodels = vec![];
            if present {
                for _ in 0..reader.count(100)? {
                    let forest = Forest::read(&mut reader)?;
                    tree_count += forest.trees.len();
                    submodels.push(forest);
                }
            }
            points.push(LbfPoint { id, xy: [xy[0], xy[1]], submodels });
        }
        let forest_end = reader.offset;
        let mut stages = Vec::with_capacity(stage_count);
        for _ in 0..stage_count {
            let matrices = (0..matrix_count).map(|i| Matrix::read(&mut reader, i == 0, true)).collect::<Result<_>>()?;
            stages.push(matrices);
        }
        reader.end()?;
        Ok(Self { version, points, tree_count, forest_end, stages, source_bytes: data.len() })
    }

    pub fn read(path: &Path) -> Result<Self> {
        Self::parse(&read_file(path)?)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LvRecord {
    pub id: (i32, i32),
    pub forest: Option<Forest>,
    pub matrix_offset: usize,
    pub matrix: Matrix,
}

/// `LBF/lv`, native loader 0x1400aadc0: one forest and one bias-free compressed matrix
/// per record.
#[derive(Debug, Clone, PartialEq)]
pub struct LvModel {
    pub version: u8,
    pub records: Vec<LvRecord>,
    pub source_bytes: usize,
}

impl LvModel {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = Reader { data, offset: 0 };
        let version = reader.u8()?;
        if version != VERSION {
            return format("Only observed version-4 LV is supported");
        }
        let count = reader.count(1000)?;
        let mut records = Vec::with_capacity(count);
        for _ in 0..count {
            let id = (reader.i32()?, reader.i32()?);
            let present = presence(reader.i32()?, "Invalid LV presence flag")?;
            let forest = if present { Some(Forest::read(&mut reader)?) } else { None };
            let matrix_offset = reader.offset;
            let matrix = Matrix::read(&mut reader, true, false)?;
            records.push(LvRecord { id, forest, matrix_offset, matrix });
        }
        reader.end()?;
        Ok(Self { version, records, source_bytes: data.len() })
    }

    pub fn read(path: &Path) -> Result<Self> {
        Self::parse(&read_file(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{SUPPORT, support_file};

    fn compressed_matrix(weights: [f32; 4], maxima: [f32; 2]) -> Matrix {
        Matrix {
            inputs: 2,
            outputs: 2,
            biases: vec![0.0; 2],
            weights: weights.to_vec(),
            has_bias: true,
            codebook_max_abs: Some(maxima.to_vec()),
        }
    }

    #[test]
    fn quantization_uses_codebook_range_and_truncates() {
        let (weights, scales) = compressed_matrix([0.25, -0.3, -0.25, 0.3], [0.5, 1.5]).quantize().unwrap();
        assert_eq!(scales, [254.0, 84.0]);
        assert_eq!(weights, [63, -25, -63, 25]);
        assert!(compressed_matrix([0.0; 4], [0.0, 1.5]).quantize().is_err());
    }

    #[test]
    fn compressed_matrix_unpacks_low_nibble_first() {
        let mut data = vec![];
        for value in [2i32, 1] {
            data.extend(value.to_le_bytes());
        }
        let codebook: Vec<f32> = (0..16).map(|i| i as f32 - 8.0).collect();
        data.extend(codebook.iter().flat_map(|v| v.to_le_bytes()));
        data.extend([0x2F, 0x05]);
        let mut reader = Reader { data: &data, offset: 0 };
        let matrix = Matrix::read(&mut reader, true, true).unwrap();
        assert_eq!((matrix.biases, matrix.weights), (vec![7.0], vec![-6.0, -3.0]));
        assert_eq!(matrix.codebook_max_abs, Some(vec![8.0]));
        reader.end().unwrap();
    }

    #[test]
    fn installed_extents_and_topology() {
        if !SUPPORT.is_dir() {
            return;
        }
        for (name, trees, end, features) in [("fa", 1980, 145093, 7920), ("fc", 1260, 92525, 5040)] {
            let model = LbfModel::read(&support_file(&format!("bdtsdata/NN/{name}.lbf"))).unwrap();
            assert_eq!((model.tree_count, model.forest_end), (trees, end));
            assert_eq!(model.stages[0][0].shape(), (features, 32));
            assert_eq!(model.stages.last().unwrap().last().unwrap().shape(), (112, 70));
            let factors: Vec<f32> = model
                .points
                .iter()
                .find(|p| !p.submodels.is_empty())
                .unwrap()
                .submodels
                .iter()
                .map(Forest::scale_factor)
                .collect();
            assert_eq!(factors, [0.4, 0.26, 0.16, 0.1]);
        }
    }

    #[test]
    fn rejects_truncation_trailing_bytes_and_other_layout() {
        if !SUPPORT.is_dir() {
            return;
        }
        let original = std::fs::read(support_file("bdtsdata/NN/fc.lbf")).unwrap();
        let trailing = [original.as_slice(), b"!"].concat();
        let other_version = [&[3u8][..], &original[1..]].concat();
        for data in [&original[..100], &trailing, &other_version] {
            assert!(LbfModel::parse(data).is_err());
        }
        assert!(LbfModel::read(&support_file("bdtsdata/LBF/lv")).is_err());
    }

    #[test]
    fn lv_layout_and_boundaries() {
        if !SUPPORT.is_dir() {
            return;
        }
        let data = std::fs::read(support_file("bdtsdata/LBF/lv")).unwrap();
        let model = LvModel::parse(&data).unwrap();
        assert_eq!(model.records.len(), 13);
        assert_eq!(model.records.iter().map(|r| r.forest.as_ref().unwrap().trees.len()).sum::<usize>(), 650);
        assert!(model.records.iter().all(|r| r.matrix.shape() == (6400, 1) && r.matrix.biases == [0.0]));
        assert!(LvModel::parse(&data[..data.len() - 1]).is_err());
        assert!(LvModel::parse(&[data.as_slice(), b"!"].concat()).is_err());
    }
}
