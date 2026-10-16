use super::*;
use crate::test_util::MODELS;

fn layer_types(xml: &str) -> Vec<String> {
    let document = roxmltree::Document::parse(xml).unwrap();
    document
        .descendants()
        .filter(|n| n.has_tag_name("layer"))
        .map(|n| n.attribute("type").unwrap().to_owned())
        .collect()
}

#[test]
fn hash_mismatch_is_rejected_before_translation() {
    let target = option_env!("CARGO_TARGET_DIR").map_or_else(|| MODELS.join("../../target"), std::path::PathBuf::from);
    let directory = target.join(format!("tobii-inference-tests/hash-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("model.xml"), "<net version=\"4\"/>").unwrap();
    std::fs::write(directory.join("model.bin"), b"corrupted").unwrap();
    let manifest = format!(
        r#"{{"files": {{"model.xml": {{"sha256": "{}"}}, "model.bin": {{"sha256": "{}"}}}}}}"#,
        sha256_hex(b"<net version=\"4\"/>"),
        sha256_hex(b"")
    );
    std::fs::write(directory.join("manifest.json"), manifest).unwrap();
    let error = load(&directory).unwrap_err().to_string();
    std::fs::remove_dir_all(&directory).unwrap();
    assert_eq!(error, "Model hash mismatch: model.bin");
}

#[test]
fn missing_connection_is_rejected() {
    let xml = r#"<net version="6"><layers>
      <layer id="1" type="ReLU"><input><port id="0"><dim>1</dim></port></input>
      <output><port id="1"><dim>1</dim></port></output></layer>
      </layers></net>"#;
    assert_eq!(translate(xml, b"").unwrap_err().to_string(), "Unconnected input in 1");
}

#[test]
fn constant_blob_extent_is_rejected() {
    let xml = r#"<net version="6"><layers>
      <layer id="0" name="bad" type="Const" precision="FP32">
      <output><port id="0"><dim>2</dim></port></output>
      <blobs><custom offset="0" size="8"/></blobs></layer>
      </layers></net>"#;
    assert_eq!(translate(xml, &[0; 4]).unwrap_err().to_string(), "Invalid blob extent");
}

#[test]
fn other_versions_cycles_and_unknown_operations_are_rejected() {
    assert_eq!(translate(r#"<net version="10"/>"#, b"").unwrap_err().to_string(), "Expected researched IR4/IR6 graph");
    let cycle = r#"<net version="6"><layers>
      <layer id="1" type="ReLU"><input><port id="0"><dim>1</dim></port></input><output><port id="1"><dim>1</dim></port></output></layer>
      </layers><edges><edge from-layer="1" from-port="1" to-layer="1" to-port="0"/></edges></net>"#;
    assert_eq!(translate(cycle, b"").unwrap_err().to_string(), "Cyclic graph or missing edge source");
    let unknown = r#"<net version="6"><layers>
      <layer id="0" name="in" type="Input" precision="FP32"><output><port id="0"><dim>1</dim></port></output></layer>
      <layer id="1" type="Tile"><data axis="0"/><input><port id="0"><dim>1</dim></port></input><output><port id="1"><dim>1</dim></port></output></layer>
      </layers><edges><edge from-layer="0" from-port="0" to-layer="1" to-port="0"/></edges></net>"#;
    assert_eq!(translate(unknown, b"").unwrap_err().to_string(), "Unsupported operation: Tile {'axis': '0'}");
}

#[test]
fn declared_shapes_are_checked() {
    let xml = r#"<net version="6"><layers>
      <layer id="0" name="in" type="Input" precision="FP32"><output><port id="0"><dim>1</dim><dim>4</dim></port></output></layer>
      <layer id="1" name="r" type="Reshape"><data dim="2,2"/><input><port id="0"><dim>1</dim><dim>4</dim></port></input>
      <output><port id="1"><dim>4</dim><dim>1</dim></port></output></layer>
      </layers><edges><edge from-layer="0" from-port="0" to-layer="1" to-port="0"/></edges></net>"#;
    assert_eq!(translate(xml, b"").unwrap_err().to_string(), "Output shape mismatch at 1: [2, 2] != [4, 1]");
}

/// The committed networks translate to the opset1 graphs the Python opset loader built:
/// the operation counts of `openvino.serialize` on that loader's models, and the digests
/// of the translation, checked against it by comparing compiled outputs bit for bit.
#[test]
fn committed_networks_translate_to_the_python_graphs() {
    for (name, inputs, digest) in [
        ("alignpre", vec![1, 1, 32, 32], ALIGNPRE_DIGEST),
        ("fr", vec![1, 3, 128, 128], FR_DIGEST),
        ("ff", vec![1, 1, 192, 256], FF_DIGEST),
        ("d1", vec![1, 1, 96, 96], D1_DIGEST),
    ] {
        let model = load(&MODELS.join(name)).unwrap();
        assert_eq!(model.inputs, [inputs], "{name}");
        let mut combined = model.xml.clone().into_bytes();
        combined.extend(&model.weights);
        assert_eq!(sha256_hex(&combined), digest, "{name}");
    }
    let d1 = load(&MODELS.join("d1")).unwrap();
    let types = layer_types(&d1.xml);
    let count = |kind: &str| types.iter().filter(|t| *t == kind).count();
    assert_eq!(
        [
            count("Convolution"),
            count("Add"),
            count("ReLU"),
            count("MaxPool"),
            count("MatMul"),
            count("Sigmoid"),
            count("Result")
        ],
        [15, 17, 14, 4, 2, 1, 1]
    );
}

const ALIGNPRE_DIGEST: &str = "9efb95e22e2dcc971a74a1e6d95dc9019fd2a3221169745c5cbed3601917512c";
const FR_DIGEST: &str = "91de24f6cbf328054667ed0fcc6d69d45b60022c5addd908d66f6458b73692e5";
const FF_DIGEST: &str = "640973c7971bf7ce3dfc29e09804f42edcd60b619bc279d88bb640b3442e1315";
const D1_DIGEST: &str = "6d5a4abcb332194db096ca6a1951be08fa705e96ad629c6661b17c2450560716";
