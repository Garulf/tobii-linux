//! The game DLL's legacy-module transform (0x18004ac80, 0x180048d30) run in emulation by
//! tools/sesp_oracle.py, and the double-precision model of tests/test_sesp.py.

use super::*;
use crate::sesp::{FrameDecoder, decode, head_pose};

const DISPLAY_AREAS: [[[f64; 3]; 3]; 4] = [
    [
        [-296.6200866699219, 326.0040588378906, 111.8187484741211],
        [300.3799133300781, 326.0040588378906, 111.8187484741211],
        [-296.6200866699219, 10.267330169677734, -3.100020170211792],
    ],
    [[-265.0, 320.0, 30.0], [265.0, 320.0, 30.0], [-265.0, 20.0, 0.0]],
    [[-350.0, 400.0, 0.0], [350.0, 400.0, 0.0], [-350.0, 10.0, 0.0]],
    [[-200.0, 250.0, 150.0], [240.0, 250.0, 150.0], [-200.0, 30.0, 20.0]],
];
const POSITIONS: [[f64; 3]; 4] = [[0.0, 0.0, 0.0], [0.0, 0.0, 600.0], [25.0, -40.0, 550.0], [-120.5, 80.25, 720.0]];
/// (module output, Python model output) per area and position, in that order.
const ORIGINAL: [([f64; 3], [f64; 3]); 16] = [
    (
        [-1.879913330078125, -168.1356964111328, -54.3593635559082],
        [-1.879913330078125, -168.1356964111328, -54.3593635559082],
    ),
    (
        [-1.879913330078125, 37.076385498046875, 509.4561767578125],
        [-1.879913330078125, 37.07637810707092, 509.4562063217163],
    ),
    (
        [23.120086669921875, -17.612335205078125, 476.15234375],
        [23.120086669921875, -17.61233276128769, 476.1523804664612],
    ),
    (
        [-122.37991333007812, 153.5291290283203, 594.7721557617188],
        [-122.37991333007812, 153.52912548184395, 594.7722053304315],
    ),
    ([0.0, -170.0, -15.0], [0.0, -170.0, -15.0]),
    ([0.0, -110.29776763916016, 582.0222778320312], [0.0, -110.29776901006699, 582.0223188400269]),
    ([25.0, -155.07443237304688, 536.2506103515625], [25.0, -155.0744428485632, 536.2506076693535]),
    ([-120.5, -18.505584716796875, 693.4415893554688], [-120.5, -18.50558766722679, 693.4416092131287]),
    ([0.0, -205.0, 0.0], [0.0, -205.0, 0.0]),
    ([0.0, -205.0, 600.0], [0.0, -205.0, 600.0]),
    ([25.0, -245.0, 550.0], [25.0, -245.0, 550.0]),
    ([-120.5, -124.75, 720.0], [-120.5, -124.75, 720.0]),
    ([-20.0, -140.0, -85.0], [-20.0, -140.0, -85.0]),
    ([-20.0, 165.23760986328125, 431.555908203125], [-20.0, 165.23760318756104, 431.55590534210205]),
    ([5.0, 105.36408996582031, 408.8587646484375], [5.0, 105.36407589912415, 408.8587534427643]),
    ([-140.5, 295.3744812011719, 494.04150390625], [-140.5, 295.3744761645794, 494.0415569841862]),
];

#[test]
fn model_reproduces_the_python_model_and_the_module() {
    let mut cases = ORIGINAL.iter();
    for [top_left, top_right, bottom_left] in DISPLAY_AREAS {
        let transform = LegacyPositionTransform::from_display_area(top_left, top_right, bottom_left);
        for position in POSITIONS {
            let (module, model) = cases.next().unwrap();
            let modelled = transform.apply(position);
            assert_eq!(&modelled, model);
            assert!(modelled.iter().zip(module).all(|(a, b)| (a - b).abs() < 1e-3));
            let restored = transform.apply(transform.invert(position));
            assert!(restored.iter().zip(position).all(|(a, b)| (a - b).abs() < 1e-3));
        }
    }
}

fn albedo() -> LegacyPositionTransform {
    let [top_left, top_right, bottom_left] = DISPLAY_AREAS[0];
    LegacyPositionTransform::from_display_area(top_left, top_right, bottom_left)
}

fn decoded(message: &[u8]) -> crate::sesp::HeadPose {
    let buffer = FrameDecoder::default().feed(message).unwrap().remove(0);
    let message = decode(&buffer).unwrap();
    head_pose(message.kind, message.payload.as_ref()).unwrap()
}

#[test]
fn head_pose_message_carries_the_inverse() {
    let sample = HeadPoseSample { valid: true, position_mm: [0.0, 0.0, 600.0], rotation_rad: [0.1, 0.2, 0.3] };
    let transform = albedo();
    let pose = decoded(&head_pose_message(4, &sample, 77, Some(&transform)));
    assert_eq!(pose.timestamp_us, 77);
    assert!(transform.apply(pose.position).iter().zip([0.0, 0.0, 600.0]).all(|(a, b)| (a - b).abs() < 1e-3));
    assert_eq!(decoded(&head_pose_message(4, &sample, 77, None)).position, [0.0, 0.0, 600.0]);
    let invalid = HeadPoseSample { valid: false, position_mm: [0.0; 3], rotation_rad: [0.0; 3] };
    let pose = decoded(&head_pose_message(5, &invalid, 1, None));
    assert_eq!((pose.position_validity, pose.rotation_validity), (0, [0, 0, 0]));
}
