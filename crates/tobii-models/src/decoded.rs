//! `decoded-lbf/`: the fa/fc/lv LBF coefficients of a models/tobii-et5 layout expanded to
//! float32 files with their manifests, for inspection outside the readers.

use std::path::Path;

use crate::bytes::{f32_bytes, read_file, sha256_hex};
use crate::error::{Result, format};
use crate::json::Json;
use crate::lbf::{LbfModel, LvModel};
use crate::tree::FileTree;

pub const LAYOUT_ROOT: &str = "models/tobii-et5";
pub const LBF_MODELS: [&str; 2] = ["fa", "fc"];

fn file_entry(data: &[u8]) -> [(&'static str, Json); 2] {
    [("bytes", Json::int(data.len())), ("sha256", Json::str(sha256_hex(data)))]
}

/// `decoded-lbf/`: the expanded float32 coefficients, the runtime's int8 first matrices and
/// their manifests, read from an on-disk support directory.
pub fn decoded_lbf_tree(support: &Path) -> Result<FileTree> {
    let mut tree = FileTree::new();
    for name in LBF_MODELS {
        let relative = format!("bdtsdata/NN/{name}.lbf");
        let data = read_file(&support.join(&relative))?;
        let model = LbfModel::parse(&data)?;
        let mut matrices = vec![];
        for (i, stage) in model.stages.iter().enumerate() {
            for (j, matrix) in stage.iter().enumerate() {
                let mut item = vec![
                    ("stage", Json::int(i)),
                    ("matrix", Json::int(j)),
                    ("shape", Json::Array(vec![Json::int(matrix.inputs), Json::int(matrix.outputs)])),
                ];
                let mut outputs =
                    vec![("weights", f32_bytes(&matrix.weights), "f32"), ("biases", f32_bytes(&matrix.biases), "f32")];
                if j == 0 {
                    let (quantized, scales) = matrix.quantize()?;
                    outputs.push(("quantized_weights", quantized.iter().map(|&q| q as u8).collect(), "i8"));
                    outputs.push(("scales", f32_bytes(&scales), "f32"));
                }
                for (kind, bytes, suffix) in outputs {
                    let filename = format!("stage{i}-matrix{j}-{kind}.{suffix}");
                    let mut entry = vec![("file", Json::str(&filename))];
                    entry.extend(file_entry(&bytes));
                    item.push((kind, Json::object(entry)));
                    tree.insert(format!("decoded-lbf/{name}/{filename}"), bytes);
                }
                matrices.push(Json::object(item));
            }
        }
        let manifest = Json::object([
            ("source", Json::str(format!("{LAYOUT_ROOT}/support/{relative}"))),
            ("source_sha256", Json::str(sha256_hex(&data))),
            ("source_bytes", Json::int(model.source_bytes)),
            ("forest_end", Json::int(model.forest_end)),
            ("landmarks", Json::int(model.points.len())),
            ("trees", Json::int(model.tree_count)),
            ("matrices", Json::Array(matrices)),
            (
                "layout",
                Json::str(
                    "Little-endian float32 weights[input, output], biases[output]; \
                     decoded file values before runtime int8 quantization.",
                ),
            ),
        ]);
        tree.insert(format!("decoded-lbf/{name}/manifest.json"), manifest.to_file());
    }
    let relative = "bdtsdata/LBF/lv";
    let data = read_file(&support.join(relative))?;
    let lv = LvModel::parse(&data)?;
    let mut records = vec![];
    for (i, record) in lv.records.iter().enumerate() {
        let filename = format!("record{i}-weights.f32");
        let bytes = f32_bytes(&record.matrix.weights);
        let Some(forest) = &record.forest else {
            return format(format!("LV record {i} has no forest"));
        };
        let mut entry = vec![
            ("id", Json::Array(vec![Json::int(record.id.0), Json::int(record.id.1)])),
            ("shape", Json::Array(vec![Json::int(record.matrix.inputs), Json::int(record.matrix.outputs)])),
            ("trees", Json::int(forest.trees.len())),
            ("matrix_offset", Json::int(record.matrix_offset)),
            ("file", Json::str(&filename)),
        ];
        entry.extend(file_entry(&bytes));
        records.push(Json::object(entry));
        tree.insert(format!("decoded-lbf/lv/{filename}"), bytes);
    }
    let manifest = Json::object([
        ("source", Json::str(format!("{LAYOUT_ROOT}/support/{relative}"))),
        ("source_sha256", Json::str(sha256_hex(&data))),
        ("source_bytes", Json::int(lv.source_bytes)),
        ("records", Json::Array(records)),
        ("layout", Json::str("Little-endian float32 weights[input, output]; no stored biases.")),
    ]);
    tree.insert("decoded-lbf/lv/manifest.json", manifest.to_file());
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digests::expected_sha256;
    use crate::test_util::SUPPORT;

    #[test]
    fn decoded_lbf_matches_the_expected_digests() {
        if !SUPPORT.is_dir() {
            return;
        }
        let tree = decoded_lbf_tree(&SUPPORT).unwrap();
        assert_eq!(tree.len(), 80);
        for (path, data) in tree.iter() {
            assert_eq!(expected_sha256(path), Some(sha256_hex(data).as_str()), "{path}");
        }
    }
}
