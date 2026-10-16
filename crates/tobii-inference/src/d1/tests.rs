use super::*;
use crate::test_util::{MODELS, runtime};

fn installed_shape() -> Vec<Point> {
    let shapes = tobii_models::Shapes::read(&MODELS.join("support/bdtsdata/NN/vnn_nir/init_shape.bin"), 2).unwrap();
    shapes.values.as_chunks::<2>().0.to_vec()
}

fn ramp() -> Vec<u8> {
    (0..120).flat_map(|y| (0..160).map(move |x| ((x + 3 * y) % 256) as u8)).collect()
}

fn hex(data: &[u8]) -> String {
    crate::legacy_ir::sha256_hex(data)
}

fn affine_bits(transform: &Affine) -> [u32; 6] {
    let [[a, b, c], [d, e, f]] = *transform;
    [a, b, c, d, e, f].map(f32::to_bits)
}

#[test]
fn offsets_are_relative_to_reference_not_absolute() {
    let reference = vec![[40.0, 50.0]; 57];
    let mut values = vec![0.0f32; 171];
    values[0] = 0.5;
    values[1] = -0.5;
    values[114..].fill(0.75);
    let result = decode_output(&values, &reference).unwrap();
    assert_eq!(result.crop_points[0], [64.0, 26.0]);
    assert_eq!(result.crop_points[1], reference[1]);
    assert_eq!(result.quality, vec![0.75; 57]);
}

#[test]
fn rejects_wrong_size_and_nonfinite() {
    assert_eq!(decode_output(&[0.0; 170], &[[0.0; 2]; 57]).unwrap_err().to_string(), OUTPUT_MESSAGE);
    assert_eq!(decode_output(&[f32::NAN; 171], &[[0.0; 2]; 57]).unwrap_err().to_string(), OUTPUT_DATA_MESSAGE);
    assert!(matches!(decode_output(&[f32::MAX; 171], &[[0.0; 2]; 57]), Err(crate::Error::Overflow(_))));
}

#[test]
fn normalized_axes_use_image_width_and_height() {
    assert_eq!(normalize_image_points(&[[320.0, 120.0]], 640.0, 480.0).unwrap(), [[0.5, 0.25]]);
    assert!(normalize_image_points(&[], 0.0, 480.0).is_err());
}

#[test]
fn similarity_recovers_known_rotation_scale_translation() {
    let source = [[0.0, 0.0], [1.0, 0.0], [1.0, 2.0], [3.0, -1.0]];
    let transform: Affine = [[0.7, -0.4, 90.0], [0.4, 0.7, 15.0]];
    let target: Vec<Point> = source
        .iter()
        .map(|p| [0, 1].map(|r| transform[r][0] * p[0] + transform[r][1] * p[1] + transform[r][2]))
        .collect();
    let fitted = fit_similarity(&source, &target).unwrap();
    for (row, expected) in fitted.iter().zip(&transform) {
        for (a, b) in row.iter().zip(expected) {
            assert!((a - b).abs() < 1e-5, "{fitted:?}");
        }
    }
    let back = inverse_transform_points(&target, &transform).unwrap();
    for (a, b) in back.iter().flatten().zip(source.iter().flatten()) {
        assert!((a - b).abs() < 1e-5);
    }
}

#[test]
fn composition_order_is_observable() {
    let scale = [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
    let translate = [[1.0, 0.0, 3.0], [0.0, 1.0, -4.0]];
    assert_eq!(compose_affine(&scale, &translate).unwrap(), [[2.0, 0.0, 6.0], [0.0, 2.0, -8.0]]);
}

#[test]
fn reference_center_and_extent() {
    let points: Vec<Point> = (0..57).map(|i| [i as f32, (2 * i + 30) as f32]).collect();
    let result = normalize_reference_shape(&points).unwrap();
    let mean = crate::numeric::mean_columns(&result);
    assert!((mean[0] - 48.0).abs() < 1e-5 && (mean[1] - 48.0).abs() < 1e-5);
    let (low, high) = bounds(&result);
    assert!(((high[1] - low[1]) - 48.0).abs() < 1e-5);
}

#[test]
fn degenerate_geometry_is_rejected() {
    assert!(fit_similarity(&[[1.0, 1.0]; 3], &[[0.0, 0.0]; 3]).is_err());
    assert!(normalize_reference_shape(&[[1.0, 1.0]; 57]).is_err());
    assert!(inverse_transform_points(&[[0.0, 0.0]], &[[0.0; 3]; 2]).is_err());
    assert!(warp_grayscale(&[0; 4], 2, 2, &[[0.0; 3]; 2], 96, 96).is_err());
    assert!(crop_transform(&installed_shape(), [0, 0, 0, 10], 0.0, true).is_err());
}

/// The crop tensor of tools/d1_input_oracle.py's inputs equals the original conversion
/// 0x1402aa500 byte for byte: these are the digests of the original's float output.
#[test]
fn crop_tensor_matches_original_conversion() {
    let pattern: Vec<u8> = (0..9216u32).map(|i| ((i * 73 + i * i / 7) & 255) as u8).collect();
    for (pixels, digest) in [
        (vec![0u8; 9216], INPUT_ZERO_DIGEST),
        ((0..9216u32).map(|i| (i % 256) as u8).collect(), INPUT_RAMP_DIGEST),
        (pattern, INPUT_PATTERN_DIGEST),
    ] {
        let tensor = prepare_crop_tensor(&pixels).unwrap();
        let bytes: Vec<u8> = tensor.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(hex(&bytes), digest);
    }
}

/// The warp of tools/d1_warp_oracle.py's ramp image and fixtures; the oracle shows these
/// crops equal the original warp 0x1400b45b0 byte for byte.
#[test]
fn warp_matches_original_fixtures() {
    let pixels = ramp();
    for (transform, digest) in WARP_FIXTURES {
        let crop = warp_grayscale(&pixels, 160, 120, transform, 96, 96).unwrap();
        assert_eq!(hex(&crop), *digest, "{transform:?}");
    }
}

/// The original D1 preprocessing 0x14007dc50 (tools/d1_crop_oracle.py) on the ramp image:
/// its crop exactly, its transform within the oracle's tolerance, and the transform bits
/// the NumPy reconstruction produced.
#[test]
fn crop_matches_original_preprocessing() {
    let reference = normalize_reference_shape(&installed_shape()).unwrap();
    let bytes: Vec<u8> = reference.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(hex(&bytes), REFERENCE_DIGEST);
    let pixels = ramp();
    for (bounding_box, roll, enabled, original, reconstructed, crop_digest) in CROP_CASES {
        let transform = crop_transform(&reference, *bounding_box, *roll, *enabled).unwrap();
        assert_eq!(affine_bits(&transform), *reconstructed, "{bounding_box:?} {roll}");
        for (got, original) in transform.iter().flatten().zip(original) {
            let original = f32::from_bits(*original);
            assert!((got - original).abs() <= 2e-5 + 1e-6 * original.abs());
        }
        let crop = warp_grayscale(&pixels, 160, 120, &transform, 96, 96).unwrap();
        assert_eq!(hex(&crop), *crop_digest);
    }
}

#[test]
fn landmark_model_runs_supplied_boxes() {
    let Some(runtime) = runtime() else { return };
    let model = D1LandmarkModel::new(&runtime, &MODELS.join("d1"), &installed_shape()).unwrap();
    for roll in [0.0, 20.0, -32.0] {
        let result = model.predict_frame(&ramp(), 160, 120, [30, 20, 65, 80], roll, true).unwrap();
        assert_eq!(result.image_points.len(), 57);
        assert!(result.quality.iter().all(|q| (0.0..=1.0).contains(q)));
        for (normalized, image) in result.normalized_points.iter().zip(&result.image_points) {
            assert_eq!(normalized[0], (f64::from(image[0]) / 160.0) as f32);
        }
    }
}

const INPUT_ZERO_DIGEST: &str = "1c0273095382988333e2f2b5ae487cea460737ed9be65cbad9c5de537f95bf75";
const INPUT_RAMP_DIGEST: &str = "fcea581c5dae05cde04b0e262a831205742d9137ee351b3f617a1987cbb5b827";
const INPUT_PATTERN_DIGEST: &str = "2e3a92c002d13334eba0e806e6ea94b7b570eb04f8ecad39def9e2a9a449deb1";
const REFERENCE_DIGEST: &str = "9099d2b24ffd912cc54207f04aab348f2dbf09876969d3ed5f01b0108063a583";
const WARP_FIXTURES: &[(Affine, &str)] = &[
    ([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], "cd1df2543652e2c04aa4bf2b311a10fcd9ef94ee3b9c822e43c31bd42495bbcd"),
    ([[1.0, 0.0, -15.0], [0.0, 1.0, -21.0]], "f0a6fdeb09595f7797875d99cfc85acceaf5c72d9703f47dbcfa0ac85b888a81"),
    ([[1.0, 0.0, -0.37], [0.0, 1.0, -0.61]], "d48d62491ca19f6ce18fb0ad35ddfd10677f1cfbc56f76ccf2894b0d80fe86d0"),
    ([[1.0, 0.0, 30.25], [0.0, 1.0, 10.75]], "fafd0b15fea93477cd7c960337d440029f636b506e0fb1547c03c52a76fb4d04"),
    ([[0.7, -0.4, 25.0], [0.4, 0.7, -15.0]], "4ac121a3e46a0b491644915daad5b48bc78245f24c9bef5d8efb61ff6e90de20"),
    ([[0.3, 0.05, 20.0], [-0.05, 0.3, 15.0]], "d6f5e0263c0e06f304b27bb4cfd2de12b154d6938e1f65859e0f7a39f70a6b77"),
];
type CropCase = ([i64; 4], f64, bool, [u32; 6], [u32; 6], &'static str);
#[rustfmt::skip]
const CROP_CASES: &[CropCase] = &[
    ([30, 20, 65, 80], 0.0, true, [0x3f1d_5cd8, 0x31a8_c756, 0x4119_4d50, 0xb1a8_c756, 0x3f1d_5cd8, 0x415c_be3c], [0x3f1d_5cd8, 0x31a8_c756, 0x4119_4d50, 0xb1a8_c756, 0x3f1d_5cd8, 0x415c_be3c], "ec1b52cfa4dfcd5b52c964225517c83cd53a0d577a4842158e7147f1e7e1b5d0"),
    ([0, 0, 160, 120], 5.0, true, [0x3e9f_d741, 0xb0e3_650b, 0x41b8_32ee, 0x30e3_650b, 0x3e9f_d741, 0x41ff_9368], [0x3e9f_d741, 0xb0e3_650b, 0x41b8_32ee, 0x30e3_650b, 0x3e9f_d741, 0x41ff_9368], "d3deeb4bfc6cf1a90cfc0e33ced3347dc1ecb61ca513c0ae8eb4584c0525396c"),
    ([-15, 10, 80, 90], -5.0, true, [0x3f05_beab, 0xb142_cbf1, 0x420b_c184, 0x3142_cbf1, 0x3f05_beab, 0x41af_8d83], [0x3f05_beab, 0xb08a_0570, 0x420b_c184, 0x308a_0570, 0x3f05_beab, 0x41af_8d83], "e91bf3c5b557f7ddb290ee06a50d8c6039d6cadc7bf7f28fb591b7e93105a836"),
    ([30, 20, 65, 80], 20.0, true, [0x3f13_df5f, 0x3e57_48eb, 0x3e4c_cb00, 0xbe57_48eb, 0x3f13_df5f, 0x41e7_fe30], [0x3f13_df5f, 0x3e57_48eb, 0x3e4c_caf4, 0xbe57_48eb, 0x3f13_df5f, 0x41e7_fe30], "2d6d31593c472469c72d23316ba6a02387723abace125fb3ab22cce467f81d66"),
    ([30, 20, 65, 80], -32.0, true, [0x3f05_737c, 0xbea6_c76f, 0x4206_2d59, 0x3ea6_c76f, 0x3f05_737c, 0xbfae_b950], [0x3f05_737c, 0xbea6_c76f, 0x4206_2d59, 0x3ea6_c76f, 0x3f05_737c, 0xbfae_b948], "f852cefecda6d489d9dda85e7a3897cbb2859fc84ea6c3442ab9ee37b36faeaa"),
    ([30, 20, 65, 80], 20.0, false, [0x3f1d_5cd8, 0x31a8_c756, 0x4119_4d50, 0xb1a8_c756, 0x3f1d_5cd8, 0x415c_be3c], [0x3f1d_5cd8, 0x31a8_c756, 0x4119_4d50, 0xb1a8_c756, 0x3f1d_5cd8, 0x415c_be3c], "ec1b52cfa4dfcd5b52c964225517c83cd53a0d577a4842158e7147f1e7e1b5d0"),
];
