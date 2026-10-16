use super::build::*;
use super::*;
use crate::test_util::hex;

/// One live 0x500 message recorded on Linux; `ORIGINAL_PLATMOD` is what the original
/// runtime code made of it in emulation (tools/gaze_oracle.py).
const DEVICE_MESSAGE_PAYLOAD: &str = concat!(
    "0000050000000400270bb8050000000400020bb90200000004000000010600000008000000d18c985049050000000400",
    "020bb9020000000400000011020000000400000004050000000400020bb9020000000400000014020000000403426327",
    "050000000400020bb902000000040000001b020000000400000001050000000400020bb902000000040000001c050000",
    "000400021f400400000008000002a3830800000400000008000002e53cf40000050000000400020bb902000000040000",
    "0016020000000400000001050000000400020bb9020000000400000002050000000400031f410400000008fffed533e6",
    "0000000400000008ffff2930820000000400000008000b391f80000000050000000400020bb902000000040000002102",
    "0000000400000001050000000400020bb9020000000400000022050000000400031f410400000008fffecdaede000000",
    "040000000800004a75680000000400000008000aef0540000000050000000400020bb902000000040000000305000000",
    "0400031f4104000000080000028d94d80000040000000800000265b8940000040000000800000297db3c000005000000",
    "0400020bb902000000040000001d020000000400000001050000000400020bb902000000040000000405000000040003",
    "1f41040000000800019391ce0000000400000008000186ee020000000400000008000072f04e00000005000000040002",
    "0bb9020000000400000005050000000400021f400400000008000002a9d49800000400000008000002e4552000000500",
    "00000400020bb9020000000400000006030000000400037817050000000400020bb90200000004000000070200000004",
    "00000000050000000400020bb9020000000400000017050000000400031f410400000008fffed2566800000004000000",
    "08ffff23be1f0000000400000008000b4a7690000000050000000400020bb9020000000400000019050000000400021f",
    "400400000008fffffc00000000000400000008fffffc0000000000050000000400020bb9020000000400000015020000",
    "000400000001050000000400020bb9020000000400000008050000000400031f410400000008ffffd9b3ca0000000400",
    "000008ffff3a7f670000000400000008000b474760000000050000000400020bb9020000000400000023020000000400",
    "000001050000000400020bb9020000000400000024050000000400031f410400000008ffffd22ec20000000400000008",
    "00005f90880000000400000008000af66720000000050000000400020bb9020000000400000009050000000400031f41",
    "040000000800000211dbc8000004000000080000025d0c080000040000000800000296abc80000050000000400020bb9",
    "02000000040000001e020000000400000001050000000400020bb902000000040000000a050000000400031f41040000",
    "00080001df885000000004000000080001b5a9480000000400000008000083f285000000050000000400020bb9020000",
    "00040000000b050000000400021f400400000008000002ca677800000400000008000002be7150000005000000040002",
    "0bb902000000040000000c030000000400040725050000000400020bb902000000040000000d02000000040000000005",
    "0000000400020bb9020000000400000018050000000400031f410400000008ffffd5265c0000000400000008ffff3559",
    "d80000000400000008000b585900000000050000000400020bb902000000040000001a050000000400021f4004000000",
    "08fffffc00000000000400000008fffffc0000000000050000000400020bb902000000040000001f0200000004000000",
    "01050000000400020bb9020000000400000020050000000400021f400400000008000002ba1e08000004000000080000",
    "02d163380000050000000400020bb9020000000400000025050000000400031f41040000000800000295dd1c00000400",
    "0000080000026de6540000040000000800000198bf440000050000000400020bb9020000000400000026020000000400",
    "000001050000000400020bb9020000000400000027050000000400031f4104000000080000020fb9cc00000400000008",
    "000002633ed40000040000000800000190c6fa0000050000000400020bb9020000000400000028020000000400000001",
    "050000000400020bb90200000004000000290300000004ffff0000050000000400020bb902000000040000002a020000",
    "000400000000050000000400020bb902000000040000002b0300000004ffff0000050000000400020bb9020000000400",
    "00002c020000000400000000",
);

const ORIGINAL_PLATMOD: [(&str, &str); 6] = [
    ("gaze_point", "4950988cd100000001000000c2e0283f3d4f393f"),
    ("responsive_gaze_point", "4950988cd10000000100000082872e3fce58343f"),
    ("eye_openness", "4950988cd100000000000000000080bf00000000000080bf"),
    ("gaze_origin", "4950988cd100000001000000912899c2d0ea944154f02e4401000000f84437c11021bf4172662f44"),
    (
        "gaze_data",
        concat!(
            "4950988cd1000000010000000d6695c27ecf56c2f8913344010000003665233f256e193fcff6253f01000000e7c8c942",
            "0177c3429ce0e54126752a3f4815393f01000000ccd496c2e1415cc269a7344401000000c0055e4001000000d83019c1",
            "998045c27674344401000000f276043f0243173ff2aa253f0100000028c4ef42a4d4da4285f20342de99323f549c2f3f",
            "0100000090662bc128a64ac29085354401000000a0e48040",
        ),
    ),
    ("user_position_guide", "4950988cd1000000010000004777253f95791b3fa25fcc3e0100000073ee033fb5cf183f7d63c83e"),
];

const CAPTURED_PAYLOADS: &str = include_str!("../../../../tests/data/gaze-payloads.json");

fn device() -> DeviceGaze {
    decode_gaze_message(&hex(DEVICE_MESSAGE_PAYLOAD)).unwrap().unwrap()
}

fn columns() -> Vec<(u32, Vec<Field>)> {
    stream_columns(&hex(DEVICE_MESSAGE_PAYLOAD)).unwrap().into_iter().map(|c| (c.column_id, c.value)).collect()
}

fn changed(update: Vec<(u32, Option<Vec<Field>>)>) -> Vec<u8> {
    let mut columns = columns();
    for (id, value) in update {
        match (columns.iter().position(|(c, _)| *c == id), value) {
            (Some(index), Some(value)) => columns[index].1 = value,
            (Some(index), None) => {
                columns.remove(index);
            }
            (None, Some(value)) => columns.push((id, value)),
            (None, None) => {}
        }
    }
    columns_payload(&columns).unwrap()
}

fn frame(payload: &[u8]) -> GazeFrame {
    GazeFrame::new(decode_gaze_message(payload).unwrap().unwrap())
}

#[test]
fn platmod_records_match_receive_gaze_data() {
    let records = platmod_records(&device(), false);
    let expected: Vec<(&str, Vec<u8>)> = ORIGINAL_PLATMOD.iter().map(|(name, data)| (*name, hex(data))).collect();
    assert_eq!(records, expected);
    assert_eq!(platmod_records(&device(), true), [expected[5].clone()]);
}

#[test]
fn records_unpack_to_the_values_they_pack() {
    for (name, data) in ORIGINAL_PLATMOD {
        let data = hex(data);
        let repacked = match name {
            "gaze_point" | "responsive_gaze_point" => PointRecord::unpack(&data).unwrap().pack(),
            "eye_openness" => OpennessRecord::unpack(&data).unwrap().pack(),
            "gaze_origin" | "user_position_guide" => PairRecord::unpack(&data).unwrap().pack(),
            _ => GazeDataRecord::unpack(&data).unwrap().pack(),
        };
        assert_eq!(repacked, data, "{name}");
    }
}

#[test]
fn gaze_point_is_the_device_combined_point() {
    let device = device();
    let frame = GazeFrame::new(device.clone());
    assert_eq!(frame.gaze_point.point, device.combined_point);
    assert_eq!(frame.gaze_point.valid, 1);
    assert_eq!(frame.responsive_gaze_point.point, device.responsive_point);
    assert_eq!(frame.gaze_origin.left, device.left_origin_display);
}

#[test]
fn validity_rules() {
    let payload = changed(vec![
        (7, Some(uint32_column(1))),
        (13, Some(uint32_column(2))),
        (27, Some(uint32_column(4))),
        (31, Some(uint32_column(0))),
        (33, Some(uint32_column(2))),
        (35, Some(uint32_column(1))),
        (42, Some(uint32_column(3))),
    ]);
    let frame = frame(&payload);
    assert_eq!(frame.gaze_point.valid, 1);
    assert_eq!(frame.responsive_gaze_point.valid, 0);
    assert_eq!((frame.gaze_origin.left_valid, frame.gaze_origin.right_valid), (0, 1));
    assert_eq!((frame.gaze_data.left.eye_valid, frame.gaze_data.right.eye_valid), (1, 0));
    assert_eq!(frame.eye_openness.left_valid, 1);
}

#[test]
fn position_guide_is_clamped_and_nan_passes() {
    let payload = changed(vec![(37, Some(vec3_column([-0.5, 0.25, 1.75]).unwrap())), (38, Some(uint32_column(1)))]);
    let guide = frame(&payload).user_position_guide;
    assert_eq!(guide.left, [0.0, 0.25, 1.0]);
    assert_eq!(guide.left_valid, 1);
    let device = DeviceGaze { left_position_guide: [f64::NAN, 0.5, 2.0], ..DeviceGaze::default() };
    let record = user_position_guide_record(&device);
    assert!(record.left[0].is_nan());
    assert_eq!(record.left[1..], [0.5, 1.0]);
}

#[test]
fn eyeball_needs_its_column() {
    let frame = frame(&changed(vec![(23, None)]));
    assert_eq!(frame.gaze_data.left.eyeball_valid, 0);
    assert_eq!(frame.gaze_data.left.eyeball_centre, [0.0; 3]);
    assert_eq!(frame.gaze_data.right.eyeball_valid, 1);
}

#[test]
fn known_column_with_another_type_drops_the_message() {
    assert!(decode_gaze_message(&changed(vec![(28, Some(vec3_column([0.5; 3]).unwrap()))])).unwrap().is_none());
    assert!(decode_gaze_message(&changed(vec![(7, Some(int32_column(0)))])).unwrap().is_none());
}

#[test]
fn unknown_and_missing_columns() {
    let payload = changed(vec![(99, Some(vec3_column([1.0, 2.0, 3.0]).unwrap())), (27, None), (28, None)]);
    let record = decode_gaze_message(&payload).unwrap().unwrap();
    assert_eq!((record.combined_validity, record.combined_point), (0, [0.0, 0.0]));
}

#[test]
fn fixed_point_values_are_narrowed_to_float32() {
    let payload = changed(vec![
        (28, Some(vec2_column([NARROWED.0, NARROWED.1]).unwrap())),
        (6, Some(fixed16_column(1.234_567).unwrap())),
    ]);
    let record = decode_gaze_message(&payload).unwrap().unwrap();
    assert_eq!(record.combined_point, [f64::from(NARROWED.0 as f32), f64::from(NARROWED.1 as f32)]);
    assert_eq!(record.left_pupil_diameter, (1.234_567f64 * 65536.0).trunc() / 65536.0);
}

const NARROWED: (f64, f64) = (0.123_456_789_1, -0.987_654_321_2);

#[test]
fn restamping_keeps_values() {
    let frame = frame(&hex(DEVICE_MESSAGE_PAYLOAD));
    let moved = frame.with_timestamp(42);
    assert_eq!((moved.gaze_point.timestamp, moved.gaze_point.point), (42, frame.gaze_point.point));
}

fn captured(name: &str) -> Vec<u8> {
    let key = format!("\"{name}\": \"");
    let start = CAPTURED_PAYLOADS.find(&key).unwrap() + key.len();
    let end = start + CAPTURED_PAYLOADS[start..].find('"').unwrap();
    hex(&CAPTURED_PAYLOADS[start..end])
}

#[test]
fn captured_validity_patterns() {
    let frames: Vec<GazeFrame> = ["both", "left", "right", "none"].map(|name| frame(&captured(name))).to_vec();
    let [both, left, right, none] = frames.try_into().unwrap();
    assert_eq!(both.timestamp_us(), 898_743_954_739);
    assert_eq!((both.gaze_point.valid, both.gaze_data.left.eye_valid, both.gaze_data.right.eye_valid), (1, 1, 1));
    assert!((both.gaze_point.point[0] - 0.471703).abs() < 5e-6);
    assert!((both.gaze_data.left.pupil_diameter - 3.121475).abs() < 5e-6);
    assert_eq!((left.gaze_data.left.eye_valid, left.gaze_data.right.eye_valid), (1, 0));
    assert_eq!((right.gaze_data.left.eye_valid, right.gaze_data.right.eye_valid), (0, 1));
    assert_eq!((none.gaze_point.valid, none.gaze_point.point), (0, [-1.0, -1.0]));
}

#[test]
fn presence_values_map_as_in_the_runtime() {
    for (device_value, expected) in [(0, 1), (1, 2), (2, 3), (3, 1), (7, 1)] {
        assert_eq!(prp_presence(platmod_presence(device_value)), expected);
    }
}

#[test]
fn presence_messages() {
    let payload = columns_payload(&[(1, int64_column(123)), (2, int32_column(2))]).unwrap();
    assert_eq!(presence_message(&payload).unwrap(), Some(Presence { timestamp_us: 123, value: 2 }));
    let recorded = hex("0000050000000400020bb8050000000400020bb90200000004000000010600000008000000d18a5be71e\
         050000000400020bb9020000000400000002010000000400000002");
    assert_eq!(presence_message(&recorded).unwrap(), Some(Presence { timestamp_us: 899_969_443_614, value: 2 }));
    let timestamp = int64_column(5);
    let wrong_value = columns_payload(&[(1, timestamp.clone()), (2, uint32_column(2))]).unwrap();
    assert_eq!(presence_message(&wrong_value).unwrap(), None);
    let other = columns_payload(&[(1, timestamp), (9, uint32_column(4))]).unwrap();
    assert_eq!(presence_message(&other).unwrap(), Some(Presence { timestamp_us: 5, value: 0 }));
    let wrong_time = columns_payload(&[(1, uint32_column(5)), (2, int32_column(1))]).unwrap();
    assert_eq!(presence_message(&wrong_time).unwrap(), None);
}
