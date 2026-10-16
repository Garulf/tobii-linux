//! tobii_linux.lbf_features: pixel-pair tree traversal.

use pyo3::prelude::*;
use tobii_headpose::lbf::{self, Image};
use tobii_models::lbf::{Forest, Tree};

use super::HeadPoseResult;
use crate::convert::bytes_like;

/// A forest from its Python tuple `(first, scale_bits, [(marker, word, depth, payload)])`.
pub fn forest(object: &Bound<'_, PyAny>) -> PyResult<Forest> {
    let forest = object;
    let first: i64 = forest.get_item(0)?.extract()?;
    let scale_bits: i64 = forest.get_item(1)?.extract()?;
    let mut trees = vec![];
    for tree in forest.get_item(2)?.try_iter()? {
        let tree = tree?;
        let depth: i64 = tree.get_item(2)?.extract()?;
        if !(0..=20).contains(&depth) {
            return Err(super::invalid("Invalid tree payload"));
        }
        trees.push(Tree {
            marker: tree.get_item(0)?.extract::<i64>()? as u32,
            word: tree.get_item(1)?.extract::<i64>()? as u32,
            depth: depth as u32,
            payload: bytes_like(&tree.get_item(3)?)?,
        });
    }
    Ok(Forest { first: first as i32, scale_bits: scale_bits as u32 as i32, trees })
}

pub fn image<'a>(pixels: &'a [u8], rows: i64, columns: i64, stride: i64) -> Image<'a> {
    Image { pixels, rows, columns, stride }
}

pub fn mirrored(axis1_sign: i64) -> PyResult<bool> {
    match axis1_sign {
        1 => Ok(false),
        -1 => Ok(true),
        _ => Err(super::invalid("Invalid image buffer or axis sign")),
    }
}

#[pyfunction]
#[pyo3(signature = (forest, pixels, rows, columns, stride, center0, center1, scale, axis1_sign=1))]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn forest_leaves(
    forest: &Bound<'_, PyAny>,
    pixels: &Bound<'_, PyAny>,
    rows: i64,
    columns: i64,
    stride: i64,
    center0: f64,
    center1: f64,
    scale: f64,
    axis1_sign: i64,
) -> PyResult<Vec<usize>> {
    let pixels = bytes_like(pixels)?;
    let image = image(&pixels, rows, columns, stride);
    let forest = self::forest(forest)?;
    lbf::forest_leaves(&forest, image, center0, center1, scale, mirrored(axis1_sign)?).py()
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(forest_leaves, m)?)
}
