//! Original-code vectors of tests/test_gaze.py: the platmod records receive_gaze_data made of
//! one live 0x500 message, the service callbacks' stream data for them and the game DLL's
//! packets (version 0 and 0x7fffffff), all from tools/gaze_oracle.py; and packets captured
//! from the real ET5 service with tools/prp_real_service_probe.py.

use tobii_protocol::gaze::{GazeDataRecord, OpennessRecord, PairRecord, PointRecord};

use super::*;
use crate::error::ServiceError;
use crate::test_util::hex;

/// (name, platmod record, operation, stream, native stream data)
const ORIGINAL: [(&str, &str, u32, u32, &str); 6] = [
    ("gaze_point", "4950988cd100000001000000c2e0283f3d4f393f", 1, 3, "4950988cd100000001000000c2e0283f3d4f393f"),
    (
        "responsive_gaze_point",
        "4950988cd10000000100000082872e3fce58343f",
        1,
        29,
        "4950988cd10000000100000082872e3fce58343f",
    ),
    (
        "eye_openness",
        "4950988cd100000000000000000080bf00000000000080bf",
        1,
        33,
        "4950988cd100000000000000000080bf00000000000080bf",
    ),
    (
        "gaze_origin",
        "4950988cd100000001000000912899c2d0ea944154f02e4401000000f84437c11021bf4172662f44",
        1,
        4,
        "4950988cd100000001000000912899c2d0ea944154f02e4401000000f84437c11021bf4172662f44",
    ),
    (
        "gaze_data",
        concat!(
            "4950988cd1000000010000000d6695c27ecf56c2f8913344010000003665233f256e193fcff6253f01000000e7c8c942",
            "0177c3429ce0e54126752a3f4815393f01000000ccd496c2e1415cc269a7344401000000c0055e4001000000d83019c1",
            "998045c27674344401000000f276043f0243173ff2aa253f0100000028c4ef42a4d4da4285f20342de99323f549c2f3f",
            "0100000090662bc128a64ac29085354401000000a0e48040",
        ),
        1,
        6,
        concat!(
            "4950988cd1000000010000000d6695c27ecf56c2f8913344010000003665233f256e193fcff6253f01000000e7c8c942",
            "0177c3429ce0e54126752a3f4815393f01000000ccd496c2e1415cc269a7344401000000c0055e4001000000d83019c1",
            "998045c27674344401000000f276043f0243173ff2aa253f0100000028c4ef42a4d4da4285f20342de99323f549c2f3f",
            "0100000090662bc128a64ac29085354401000000a0e48040",
        ),
    ),
    (
        "user_position_guide",
        "4950988cd1000000010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f7d63c83e",
        11,
        2,
        "4950988cd1000000010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f7d63c83e",
    ),
];

/// (name, extended, packet)
const ORIGINAL_PACKETS: [(&str, bool, &str); 10] = [
    (
        "gaze_point",
        false,
        concat!(
            "545052503c000000685052500c00000000000000010000000400000008000000030000000400000018000000d1000000",
            "4950988c01000000c2e0283f3d4f393f0400000000000000",
        ),
    ),
    (
        "gaze_point",
        true,
        concat!(
            "545052503c000000685052500c00000000000000010000000400000008000000030000000400000018000000d1000000",
            "4950988c01000000c2e0283f3d4f393f0400000000000000",
        ),
    ),
    (
        "responsive_gaze_point",
        false,
        concat!(
            "545052503c000000685052500c000000000000000100000004000000080000001d0000000400000018000000d1000000",
            "4950988c0100000082872e3fce58343f0400000000000000",
        ),
    ),
    (
        "responsive_gaze_point",
        true,
        concat!(
            "545052503c000000685052500c000000000000000100000004000000080000001d0000000400000018000000d1000000",
            "4950988c0100000082872e3fce58343f0400000000000000",
        ),
    ),
    (
        "gaze_origin",
        false,
        concat!(
            "5450525050000000045052500c0000000000000001000000040000000800000004000000040000002c000000d1000000",
            "4950988c01000000912899c2d0ea944154f02e4401000000f84437c11021bf4172662f440400000000000000",
        ),
    ),
    (
        "gaze_origin",
        true,
        concat!(
            "5450525050000000045052500c0000000000000001000000040000000800000004000000040000002c000000d1000000",
            "4950988c01000000912899c2d0ea944154f02e4401000000f84437c11021bf4172662f440400000000000000",
        ),
    ),
    (
        "gaze_data",
        false,
        concat!(
            "54505250c80000009c5052500c000000000000000100000004000000080000000600000004000000a4000000d1000000",
            "4950988c010000000d6695c27ecf56c2f89133443665233f256e193fcff6253f01000000e7c8c9420177c3429ce0e541",
            "26752a3f4815393f01000000ccd496c2e1415cc269a7344401000000c0055e4001000000d83019c1998045c276743444",
            "f276043f0243173ff2aa253f0100000028c4ef42a4d4da4285f20342de99323f549c2f3f0100000090662bc128a64ac2",
            "9085354401000000a0e480400400000000000000",
        ),
    ),
    (
        "gaze_data",
        true,
        concat!(
            "54505250d0000000845052500c000000000000000100000004000000080000000600000004000000ac000000d1000000",
            "4950988c010000000d6695c27ecf56c2f89133443665233f256e193fcff6253f01000000e7c8c9420177c3429ce0e541",
            "26752a3f4815393f01000000ccd496c2e1415cc269a7344401000000c0055e4001000000d83019c1998045c276743444",
            "f276043f0243173ff2aa253f0100000028c4ef42a4d4da4285f20342de99323f549c2f3f0100000090662bc128a64ac2",
            "9085354401000000a0e4804001000000010000000400000000000000",
        ),
    ),
    (
        "user_position_guide",
        false,
        concat!(
            "5450525050000000045052500c000000000000000b000000040000000800000002000000040000002c000000d1000000",
            "4950988c010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f7d63c83e0400000000000000",
        ),
    ),
    (
        "user_position_guide",
        true,
        concat!(
            "5450525050000000045052500c000000000000000b000000040000000800000002000000040000002c000000d1000000",
            "4950988c010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f7d63c83e0400000000000000",
        ),
    ),
];

const REAL_GAZE_POINT_VALID: &str = concat!(
    "545052503c000000685052500c00000000000000010000000400000008000000030000000400000018000000d1000000",
    "934429a4010000004a79083f06544e3e0400000000000000",
);
const REAL_GAZE_POINT_INVALID: &str = concat!(
    "545052503c000000685052500c00000000000000010000000400000008000000030000000400000018000000d1000000",
    "93b82fa400000000000080bf000080bf0400000000000000",
);
const REAL_RESPONSIVE_VALID: &str = concat!(
    "545052503c000000685052500c000000000000000100000004000000080000001d0000000400000018000000d1000000",
    "934429a4010000001953093f84685c3e0400000000000000",
);
const REAL_GAZE_ORIGIN_VALID: &str = concat!(
    "5450525050000000045052500c0000000000000001000000040000000800000004000000040000002c000000d1000000",
    "934429a401000000f3ca32c3e049144189ea234401000000bd00e4c2406bb8410b2f26440400000000000000",
);
const REAL_GAZE_ORIGIN_INVALID: &str = concat!(
    "5450525050000000045052500c0000000000000001000000040000000800000004000000040000002c000000d1000000",
    "93b82fa400000000000000000000000000000000000000000000000000000000000000000400000000000000",
);

fn platmod(name: &str, packed: &[u8]) -> PlatmodRecord {
    match name {
        "gaze_point" => PlatmodRecord::GazePoint(PointRecord::unpack(packed).unwrap()),
        "responsive_gaze_point" => PlatmodRecord::ResponsiveGazePoint(PointRecord::unpack(packed).unwrap()),
        "gaze_origin" => PlatmodRecord::GazeOrigin(PairRecord::unpack(packed).unwrap()),
        "gaze_data" => PlatmodRecord::GazeData(Box::new(GazeDataRecord::unpack(packed).unwrap())),
        "eye_openness" => PlatmodRecord::EyeOpenness(OpennessRecord::unpack(packed).unwrap()),
        "user_position_guide" => PlatmodRecord::UserPositionGuide(PairRecord::unpack(packed).unwrap()),
        other => panic!("unknown record {other}"),
    }
}

fn original_record(name: &str) -> PrpRecord {
    let (_, packed, ..) = ORIGINAL.iter().find(|entry| entry.0 == name).unwrap();
    prp_record(&platmod(name, &hex(packed)))
}

#[test]
fn prp_records_match_the_service_callbacks() {
    for (name, packed, kind, stream, native) in ORIGINAL {
        let record = prp_record(&platmod(name, &hex(packed)));
        assert_eq!(record, PrpRecord { kind, stream, native: hex(native) }, "{name}");
    }
}

#[test]
fn packets_match_the_game_dll_serializer() {
    for (name, extended, packet) in ORIGINAL_PACKETS {
        assert_eq!(stream_packet(&original_record(name), 0, extended).unwrap(), Some(hex(packet)), "{name}");
    }
}

#[test]
fn eye_openness_has_no_serializer_case() {
    assert_eq!(stream_packet(&original_record("eye_openness"), 0, true).unwrap(), None);
    let unknown = PrpRecord { kind: OPERATION_COMPOUND_STREAM, stream: 9, native: vec![0; 40] };
    assert_eq!(stream_body(&unknown, true).unwrap(), None);
}

#[test]
fn validity_other_than_one_is_dropped_by_the_service() {
    let pair =
        PairRecord { timestamp: 1, left_valid: 2, left: [1.0, 2.0, 3.0], right_valid: 1, right: [4.0, 5.0, 6.0] };
    let record = prp_record(&PlatmodRecord::GazeOrigin(pair));
    assert_eq!((record.native[8], record.native[24]), (0, 1));
}

#[test]
fn malformed_native_data_is_refused() {
    let record = PrpRecord { kind: OPERATION_STREAM, stream: PRP_GAZE_POINT, native: vec![0; 19] };
    assert!(matches!(stream_body(&record, true), Err(ServiceError::Truncated(_))));
}

fn body(packet: &str, length: usize) -> Vec<u8> {
    let payload = crate::prp::decode_envelope(&hex(packet), crate::prp::DEFAULT_MAX_PAYLOAD).unwrap().to_vec();
    payload[28..28 + length].to_vec()
}

#[test]
fn real_gaze_points_re_encode_identically() {
    for (packet, responsive) in
        [(REAL_GAZE_POINT_VALID, false), (REAL_GAZE_POINT_INVALID, false), (REAL_RESPONSIVE_VALID, true)]
    {
        let point = crate::prp::decode_gaze_body(&body(packet, 24)).unwrap();
        let record =
            PointRecord { timestamp: point.timestamp, valid: u32::from(point.valid), point: [point.x, point.y] };
        let platmod =
            if responsive { PlatmodRecord::ResponsiveGazePoint(record) } else { PlatmodRecord::GazePoint(record) };
        assert_eq!(stream_packet(&prp_record(&platmod), 0, true).unwrap(), Some(hex(packet)));
    }
    let invalid = crate::prp::decode_gaze_body(&body(REAL_GAZE_POINT_INVALID, 24)).unwrap();
    assert_eq!((invalid.valid, invalid.x, invalid.y), (false, -1.0, -1.0));
}

#[test]
fn real_gaze_origins_re_encode_identically() {
    for packet in [REAL_GAZE_ORIGIN_VALID, REAL_GAZE_ORIGIN_INVALID] {
        let data = body(packet, 44);
        let word = |index: usize| u32::from_le_bytes(data[4 * index..4 * index + 4].try_into().unwrap());
        let float = |index: usize| f64::from(f32::from_bits(word(index)));
        let pair = PairRecord {
            timestamp: crate::prp::timestamp(word(1), word(2)),
            left_valid: word(3),
            left: [float(4), float(5), float(6)],
            right_valid: word(7),
            right: [float(8), float(9), float(10)],
        };
        assert_eq!(stream_packet(&prp_record(&PlatmodRecord::GazeOrigin(pair)), 0, true).unwrap(), Some(hex(packet)));
    }
}
