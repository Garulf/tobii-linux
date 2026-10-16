//! Hand-transcribed vectors from the game DLL's serializer (tests/test_prp.py) and the
//! original serializer's output in emulation (tools/prp_encoder_oracle.py).

use super::*;
use crate::error::ServiceError;
use crate::test_util::{hex, to_hex};

fn gaze(timestamp: i64, valid: bool, x: f64, y: f64) -> GazePoint {
    GazePoint { timestamp, valid, x, y }
}

#[test]
fn gaze_word_order() {
    let wire = hex("18000000 44332211 88776655 01000000 0000003f 0000803e");
    let sample = gaze(0x1122334455667788, true, 0.5, 0.25);
    assert_eq!(decode_gaze_body(&wire).unwrap(), sample);
    assert_eq!(encode_gaze_body(&sample), wire);
}

#[test]
fn head_independent_rotation_validity() {
    let wire = hex("34000000 ffffffff feffffff 01000000 0000803f 00000040 00004040 \
                    01000000 00000000 01000000 000080be 0000003f 000080bf");
    let sample = HeadPose {
        timestamp: -2,
        position_valid: true,
        position: [1.0, 2.0, 3.0],
        rotation_valid: [true, false, true],
        rotation: [-0.25, 0.5, -1.0],
    };
    assert_eq!(decode_head_body(&wire).unwrap(), sample);
    assert_eq!(encode_head_body(&sample), wire);
}

#[test]
fn envelope_vector_and_corruption() {
    let wire = hex("54505250 04000000 50505250 00000000");
    assert_eq!(encode_envelope(&[0; 4], DEFAULT_MAX_PAYLOAD).unwrap(), wire);
    assert_eq!(decode_envelope(&wire, DEFAULT_MAX_PAYLOAD).unwrap(), &[0; 4]);
    let mut bad_magic = wire.clone();
    bad_magic[0] = b'X';
    let mut bad_check = wire.clone();
    bad_check[8..12].fill(0);
    let longer = [wire.as_slice(), b"x"].concat();
    for bad in [&wire[..11], &wire[..wire.len() - 1], &longer, &bad_magic, &bad_check] {
        assert!(matches!(decode_envelope(bad, DEFAULT_MAX_PAYLOAD), Err(ServiceError::Prp(_))));
    }
    assert!(decode_envelope(&wire, 3).is_err());
    assert!(encode_envelope(&[0; 4], 3).is_err());
}

#[test]
fn body_validation() {
    for (length, decode) in [
        (GAZE_BODY_SIZE, (|body: &[u8]| decode_gaze_body(body).map(|_| ())) as fn(&[u8]) -> Result<()>),
        (HEAD_BODY_SIZE, |body: &[u8]| decode_head_body(body).map(|_| ())),
    ] {
        let mut short = (length as u32).to_le_bytes().to_vec();
        short.resize(length - 1, 0);
        for body in [Vec::new(), vec![0; length], short] {
            assert!(decode(&body).is_err());
        }
    }
    let mut bad = encode_gaze_body(&gaze(0, true, 0.0, 0.0));
    bad[12] = 2;
    assert_eq!(decode_gaze_body(&bad), Err(ServiceError::Prp("noncanonical PRP validity field".into())));
}

#[test]
fn complete_gaze_packet_from_original_encoder() {
    let wire = hex("54505250 3c000000 68505250 0c000000 2a000000 01000000 \
                    04000000 08000000 03000000 04000000 18000000 44332211 \
                    88776655 01000000 0000003f 0000803e 04000000 00000000");
    let sample = StreamSample::Gaze(gaze(0x1122334455667788, true, 0.5, 0.25));
    assert_eq!(encode_stream_packet(&sample, 42, false).unwrap(), wire);
}

/// Bodies and packets of the original serializer (0x180052e60, 0x18004f730).
#[test]
fn original_serializer_cases() {
    let cases = [
        (gaze(0x1122334455667788, true, 0.5, 0.25), "180000004433221188776655010000000000003f0000803e"),
        (gaze(-2, false, -1.0, 0.0), "18000000fffffffffeffffff00000000000080bf00000000"),
    ];
    for responsive in [false, true] {
        let stream = if responsive { "1d" } else { "03" };
        for (sample, body) in cases {
            assert_eq!(to_hex(&encode_gaze_body(&sample)), body);
            let packet = format!(
                "545052503c000000685052500c0000002a000000010000000400000008000000{stream}00000004000000{body}0400000000000000"
            );
            assert_eq!(to_hex(&encode_stream_packet(&StreamSample::Gaze(sample), 42, responsive).unwrap()), packet);
        }
    }
    let heads = [
        (
            HeadPose {
                timestamp: -2,
                position_valid: true,
                position: [1.0, 2.0, 3.0],
                rotation_valid: [true, false, true],
                rotation: [-0.25, 0.5, -1.0],
            },
            "34000000fffffffffeffffff010000000000803f0000004000004040010000000000000001000000000080be0000003f000080bf",
        ),
        (
            HeadPose {
                timestamp: 0x1122334455667788,
                position_valid: false,
                position: [0.0, -1.0, 9.0],
                rotation_valid: [false, true, false],
                rotation: [0.0, 0.0, 0.0],
            },
            "3400000044332211887766550000000000000000000080bf00001041000000000100000000000000000000000000000000000000",
        ),
    ];
    for (head, body) in heads {
        assert_eq!(to_hex(&encode_head_body(&head)), body);
        let packet = format!(
            "54505250580000000c5052500c000000ffffffff0100000004000000080000000200000004000000{body}0400000000000000"
        );
        assert_eq!(to_hex(&encode_stream_packet(&StreamSample::Head(head), 0xFFFF_FFFF, false).unwrap()), packet);
        assert!(encode_stream_packet(&StreamSample::Head(head), 0, true).is_err());
    }
}
