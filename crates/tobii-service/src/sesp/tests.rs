//! Vectors of tests/test_sesp.py and tools/sesp_oracle.py: service messages from
//! ses_windows.dll's builders and requests from the game DLL's builders, run in emulation.

use super::*;
use crate::test_util::{hex, to_hex};

/// Service builder output of ses_windows.dll by case (tools/sesp_oracle.py).
const ORIGINAL_SERVICE_MESSAGES: [(&str, &str); 14] = [
    (
        "initialize, head pose",
        concat!(
            "736573702e0000005d65737004000000e0ffffff010000000800000003000000f8ffffff010000000800050000000400",
            "0a000d0004000c000800",
        ),
    ),
    (
        "initialize, head pose and stream status",
        concat!(
            "73657370320000004165737004000000dcffffff020000000800000003000000f8ffffff010100000c00060000000400",
            "000005000a000d0004000c000800",
        ),
    ),
    (
        "initialize, every flag",
        concat!(
            "73657370560000002565737004000000b8ffffff070000000800000003000000ecffffff010101010101010101010101",
            "010101002400130000000400050006000700080009000a000b000c000d000e000f001000110012000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "initialize with error",
        concat!(
            "736573704e0000003d65737004000000c0ffffff040302010800000003000000f4ffffff050000000101000024000a00",
            "04000800000000000000000000000000000000000000000000000000000009000a000d0004000c000800",
        ),
    ),
    (
        "initialize, error above 7",
        concat!(
            "736573702c0000005f65737004000000e2ffffff030000000800000003000000f8ffffff010000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "initialize, sequence 0",
        concat!(
            "736573703a0000004965737004000000d4ffffff0800000003000000f8ffffff01010000180006000000000004000000",
            "0000000000000000000005000a000900000008000400",
        ),
    ),
    (
        "head-pose start",
        concat!(
            "73657370260000005565737004000000e8ffffff030000000800000005000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "head-pose stop with error",
        concat!(
            "736573702c0000005f65737004000000e2ffffff040000000800000007000000f8ffffff060000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "statistics, largest sequence",
        concat!(
            "736573702c0000005f65737004000000e2ffffffffffffff0800000033000000f8ffffff010000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "invalid operation, sequence 0",
        concat!(
            "73657370280000005b65737004000000e6ffffff0800000001000000f8ffffff070000000600080004000a0009000000",
            "08000400",
        ),
    ),
    (
        "stream status",
        concat!(
            "73657370260000005565737004000000e8ffffff63000000080000001b000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "head pose",
        concat!(
            "73657370560000002565737004000000b8ffffff010000000800000008000000dcffffff01000000785634120000c03f",
            "000000c000001644cdcccc3dcdcc4cbe9a99993e14002400040008000c001000140018001c0020000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "head pose, mostly zero",
        concat!(
            "73657370320000004165737004000000dcffffff020000000800000008000000f4ffffff1c000000141a99be08000c00",
            "040008000a000d0004000c000800",
        ),
    ),
    (
        "head pose, invalid and negative time",
        concat!(
            "73657370660000001565737004000000a8ffffff030000000800000008000000d4fffffffffffffffbffffff00004441",
            "0080204300000744000000bf000040400000000000000000000000001c002c00040008000c0010001400180000001c00",
            "20002400000028000a000d0004000c000800",
        ),
    ),
];

const ORIGINAL_INITIALIZE_REQUEST: &str = concat!(
    "736573704a0000003965737004000000c4ffffff020000000800000002000000dcffffff080000000300000013000000",
    "7465742d7463703a2f2f3132372e302e302e310008000c00040008000a000d0004000c000800",
);
const ORIGINAL_INITIALIZE_REQUEST_WITHOUT_SUBSCRIPTION: &str = concat!(
    "73657370440000003765737004000000caffffff010000000800000002000000e0ffffff04000000130000007465742d",
    "7463703a2f2f3132372e302e302e31000600080004000a000d0004000c000800",
);
const ORIGINAL_HEADPOSE_START_REQUEST: &str = concat!(
    "73657370260000005565737004000000e8ffffff040000000800000004000000fcffffff040004000a000d0004000c00",
    "0800",
);
const ORIGINAL_STREAM_STATUS_REQUEST: &str = concat!(
    "736573703e0000004d65737004000000d0ffffff05000000080000001a000000e8ffffff0c0000000400000000000000",
    "010000000d00000008000c00040008000a000d0004000c000800",
);
/// ses_windows.dll builder output for the inputs of tools/sesp_oracle.py, in its order.
const ORIGINAL_BUILDER_CASES: [(&str, &str); 30] = [
    (
        "initialize response 1 error 0 flags [1]",
        concat!(
            "736573702e0000005d65737004000000e0ffffff010000000800000003000000f8ffffff010000000800050000000400",
            "0a000d0004000c000800",
        ),
    ),
    (
        "initialize response 2 error 0 flags [1, 3]",
        concat!(
            "73657370320000004165737004000000dcffffff020000000800000003000000f8ffffff010100000c00060000000400",
            "000005000a000d0004000c000800",
        ),
    ),
    (
        "initialize response 7 error 0 flags [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]",
        concat!(
            "73657370560000002565737004000000b8ffffff070000000800000003000000ecffffff010101010101010101010101",
            "010101002400130000000400050006000700080009000a000b000c000d000e000f001000110012000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "initialize response 16909060 error 5 flags [1, 15]",
        concat!(
            "736573704e0000003d65737004000000c0ffffff040302010800000003000000f4ffffff050000000101000024000a00",
            "04000800000000000000000000000000000000000000000000000000000009000a000d0004000c000800",
        ),
    ),
    (
        "initialize response 3 error 9 flags []",
        concat!(
            "736573702c0000005f65737004000000e2ffffff030000000800000003000000f8ffffff010000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "initialize response 0 error 0 flags [2, 9]",
        concat!(
            "736573703a0000004965737004000000d4ffffff0800000003000000f8ffffff01010000180006000000000004000000",
            "0000000000000000000005000a000900000008000400",
        ),
    ),
    (
        "type 5 sequence 3 error 0",
        concat!(
            "73657370260000005565737004000000e8ffffff030000000800000005000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "type 5 sequence 4 error 6",
        concat!(
            "736573702c0000005f65737004000000e2ffffff040000000800000005000000f8ffffff060000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "type 5 sequence 0 error 7",
        concat!(
            "73657370280000005b65737004000000e6ffffff0800000005000000f8ffffff070000000600080004000a0009000000",
            "08000400",
        ),
    ),
    (
        "type 5 sequence 4294967295 error 12",
        concat!(
            "736573702c0000005f65737004000000e2ffffffffffffff0800000005000000f8ffffff010000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "type 7 sequence 3 error 0",
        concat!(
            "73657370260000005565737004000000e8ffffff030000000800000007000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "type 7 sequence 4 error 6",
        concat!(
            "736573702c0000005f65737004000000e2ffffff040000000800000007000000f8ffffff060000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "type 7 sequence 0 error 7",
        concat!(
            "73657370280000005b65737004000000e6ffffff0800000007000000f8ffffff070000000600080004000a0009000000",
            "08000400",
        ),
    ),
    (
        "type 7 sequence 4294967295 error 12",
        concat!(
            "736573702c0000005f65737004000000e2ffffffffffffff0800000007000000f8ffffff010000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "type 51 sequence 3 error 0",
        concat!(
            "73657370260000005565737004000000e8ffffff030000000800000033000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "type 51 sequence 4 error 6",
        concat!(
            "736573702c0000005f65737004000000e2ffffff040000000800000033000000f8ffffff060000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "type 51 sequence 0 error 7",
        concat!(
            "73657370280000005b65737004000000e6ffffff0800000033000000f8ffffff070000000600080004000a0009000000",
            "08000400",
        ),
    ),
    (
        "type 51 sequence 4294967295 error 12",
        concat!(
            "736573702c0000005f65737004000000e2ffffffffffffff0800000033000000f8ffffff010000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "type 1 sequence 3 error 0",
        concat!(
            "73657370260000005565737004000000e8ffffff030000000800000001000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "type 1 sequence 4 error 6",
        concat!(
            "736573702c0000005f65737004000000e2ffffff040000000800000001000000f8ffffff060000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "type 1 sequence 0 error 7",
        concat!(
            "73657370280000005b65737004000000e6ffffff0800000001000000f8ffffff070000000600080004000a0009000000",
            "08000400",
        ),
    ),
    (
        "type 1 sequence 4294967295 error 12",
        concat!(
            "736573702c0000005f65737004000000e2ffffffffffffff0800000001000000f8ffffff010000000600080004000a00",
            "0d0004000c000800",
        ),
    ),
    (
        "stream status response 1",
        concat!(
            "73657370260000005565737004000000e8ffffff01000000080000001b000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "stream status response 0",
        "73657370220000005165737004000000ecffffff080000001b000000fcffffff040004000a000900000008000400",
    ),
    (
        "stream status response 99",
        concat!(
            "73657370260000005565737004000000e8ffffff63000000080000001b000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "head pose 1",
        concat!(
            "73657370560000002565737004000000b8ffffff010000000800000008000000dcffffff01000000785634120000c03f",
            "000000c000001644cdcccc3dcdcc4cbe9a99993e14002400040008000c001000140018001c0020000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "head pose 2",
        concat!(
            "73657370320000004165737004000000dcffffff020000000800000008000000f4ffffff1c000000141a99be08000c00",
            "040008000a000d0004000c000800",
        ),
    ),
    (
        "head pose 3",
        concat!(
            "73657370660000001565737004000000a8ffffff030000000800000008000000d4fffffffffffffffbffffff00004441",
            "0080204300000744000000bf000040400000000000000000000000001c002c00040008000c0010001400180000001c00",
            "20002400000028000a000d0004000c000800",
        ),
    ),
    (
        "head pose 4",
        concat!(
            "73657370660000001565737004000000a8ffffff040000000800000008000000d4ffffffb168de3a95bfd633000048c4",
            "0000803e0000403f000000be000000000000000000000000000000001c002c0000000400000008000c00100014001800",
            "1c002000240028000a000d0004000c000800",
        ),
    ),
    (
        "head pose 5",
        concat!(
            "73657370540000002765737004000000baffffff050000000800000008000000e4ffffff000000400000404000000040",
            "0000803f0000803f000000001a001c000400000008000c0010000000000014000000000018000a000d0004000c000800",
        ),
    ),
];

/// Requests of the game DLL's builders (tools/sesp_oracle.py).
const ORIGINAL_GAME_REQUESTS: [(&str, &str); 6] = [
    (
        "initialize request 1/0",
        concat!(
            "73657370440000003765737004000000caffffff010000000800000002000000e0ffffff04000000130000007465742d",
            "7463703a2f2f3132372e302e302e31000600080004000a000d0004000c000800",
        ),
    ),
    (
        "initialize request 2/3",
        concat!(
            "736573704a0000003965737004000000c4ffffff020000000800000002000000dcffffff080000000300000013000000",
            "7465742d7463703a2f2f3132372e302e302e310008000c00040008000a000d0004000c000800",
        ),
    ),
    (
        "initialize request 2147483647/65536",
        concat!(
            "736573704a0000003965737004000000c4ffffffffffff7f0800000002000000dcffffff080000000000010013000000",
            "7465742d7463703a2f2f3132372e302e302e310008000c00040008000a000d0004000c000800",
        ),
    ),
    (
        "request type 4",
        concat!(
            "73657370260000005565737004000000e8ffffff040000000800000004000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "request type 6",
        concat!(
            "73657370260000005565737004000000e8ffffff040000000800000006000000fcffffff040004000a000d0004000c00",
            "0800",
        ),
    ),
    (
        "notify stream status request",
        concat!(
            "736573703e0000004d65737004000000d0ffffff05000000080000001a000000e8ffffff0c0000000400000000000000",
            "010000000d00000008000c00040008000a000d0004000c000800",
        ),
    ),
];

fn pose(timestamp_us: i64, position: [f64; 3], rotation: [f64; 3]) -> HeadPose {
    HeadPose { timestamp_us, position, rotation, position_validity: VALID, rotation_validity: [VALID; 3] }
}

fn invalid_pose() -> HeadPose {
    HeadPose { position_validity: 0, rotation_validity: [0, 1, 0], ..pose(-5, [12.25, 160.5, 540.0], [-0.5, 0.0, 3.0]) }
}

fn service_message(name: &str) -> Vec<u8> {
    match name {
        "initialize, head pose" => encode_initialize_response(1, 0, &[1]),
        "initialize, head pose and stream status" => encode_initialize_response(2, 0, &[1, 3]),
        "initialize, every flag" => encode_initialize_response(7, 0, &(1..16).collect::<Vec<_>>()),
        "initialize with error" => encode_initialize_response(0x01020304, 5, &[1, 15]),
        "initialize, error above 7" => encode_initialize_response(3, 9, &[]),
        "initialize, sequence 0" => encode_initialize_response(0, 0, &[2, 9]),
        "head-pose start" => encode_error_response(3, RESPONSE_HEADPOSE_START, 0),
        "head-pose stop with error" => encode_error_response(4, RESPONSE_HEADPOSE_STOP, 6),
        "statistics, largest sequence" => encode_error_response(0xFFFF_FFFF, RESPONSE_SEND_STATISTICS, 12),
        "invalid operation, sequence 0" => encode_error_response(0, SIGNAL_INVALID_OPERATION, 7),
        "stream status" => encode_stream_status_response(99),
        "head pose" => encode_head_pose(1, &pose(0x112345678, [1.5, -2.0, 600.0], [0.1, -0.2, 0.3])),
        "head pose, mostly zero" => encode_head_pose(2, &pose(123456789012, [0.0; 3], [0.0; 3])),
        "head pose, invalid and negative time" => encode_head_pose(3, &invalid_pose()),
        other => panic!("unknown case {other}"),
    }
}

#[test]
fn service_messages_match_the_original_builders() {
    for (name, expected) in ORIGINAL_SERVICE_MESSAGES {
        assert_eq!(to_hex(&service_message(name)), hex_text(expected), "{name}");
    }
}

fn hex_text(text: &str) -> String {
    to_hex(&hex(text))
}

/// The inputs of tools/sesp_oracle.py's service_cases, in its order.
fn builder_inputs() -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    for (sequence, error, flags) in [
        (1, 0, vec![1]),
        (2, 0, vec![1, 3]),
        (7, 0, (1..16).collect()),
        (0x01020304, 5, vec![1, 15]),
        (3, 9, vec![]),
        (0, 0, vec![2, 9]),
    ] {
        messages.push(encode_initialize_response(sequence, error, &flags));
    }
    for kind in [RESPONSE_HEADPOSE_START, RESPONSE_HEADPOSE_STOP, RESPONSE_SEND_STATISTICS, SIGNAL_INVALID_OPERATION] {
        for (sequence, error) in [(3, 0), (4, 6), (0, 7), (0xFFFF_FFFF, 12)] {
            messages.push(encode_error_response(sequence, kind, error));
        }
    }
    for sequence in [1, 0, 99] {
        messages.push(encode_stream_status_response(sequence));
    }
    let poses = [
        pose(0x0000000112345678, [1.5, -2.0, 600.0], [0.1, -0.2, 0.3]),
        pose(123456789012, [0.0; 3], [0.0; 3]),
        invalid_pose(),
        HeadPose {
            position_validity: 0,
            rotation_validity: [0, 0, 0],
            ..pose(987654321, [-0.0, 1e-7, -800.0], [0.25, 0.75, -0.125])
        },
        HeadPose {
            position_validity: 1,
            rotation_validity: [1, 0, 1],
            ..pose(1 << 62, [3.0, 2.0, 1.0], [0.0, 0.0, 1.0])
        },
    ];
    for (sequence, pose) in poses.iter().enumerate() {
        messages.push(encode_head_pose(sequence as u32 + 1, pose));
    }
    messages
}

#[test]
fn every_oracle_builder_case_matches() {
    let inputs = builder_inputs();
    assert_eq!(inputs.len(), ORIGINAL_BUILDER_CASES.len());
    for (message, (name, expected)) in inputs.iter().zip(ORIGINAL_BUILDER_CASES) {
        assert_eq!(to_hex(message), hex_text(expected), "{name}");
    }
}

fn one_message(data: &[u8]) -> Vec<u8> {
    let buffers = FrameDecoder::default().feed(data).unwrap();
    assert_eq!(buffers.len(), 1);
    buffers.into_iter().next().unwrap()
}

#[test]
fn game_requests_decode() {
    let buffer = one_message(&hex(ORIGINAL_INITIALIZE_REQUEST));
    let message = decode(&buffer).unwrap();
    let request = initialize_request(message.kind, message.payload.as_ref()).unwrap();
    assert_eq!(request, InitializeRequest { device_url: "tet-tcp://127.0.0.1".into(), subscription_id: 3 });
    let buffer = one_message(&hex(ORIGINAL_INITIALIZE_REQUEST_WITHOUT_SUBSCRIPTION));
    let bare = decode(&buffer).unwrap();
    assert_eq!((bare.sequence, initialize_request(bare.kind, bare.payload.as_ref()).unwrap().subscription_id), (1, 0));
    let buffer = one_message(&hex(ORIGINAL_HEADPOSE_START_REQUEST));
    let start = decode(&buffer).unwrap();
    assert_eq!((start.sequence, start.kind), (4, REQUEST_HEADPOSE_START));
    let buffer = one_message(&hex(ORIGINAL_STREAM_STATUS_REQUEST));
    let status = decode(&buffer).unwrap();
    assert_eq!((status.sequence, status.kind), (5, REQUEST_NOTIFY_STREAM_STATUS));
}

#[test]
fn oracle_game_requests_decode() {
    let expected: [(u32, u8, Option<u32>); 6] = [
        (1, REQUEST_INITIALIZE, Some(0)),
        (2, REQUEST_INITIALIZE, Some(3)),
        (0x7FFF_FFFF, REQUEST_INITIALIZE, Some(0x10000)),
        (4, REQUEST_HEADPOSE_START, None),
        (4, REQUEST_HEADPOSE_STOP, None),
        (5, REQUEST_NOTIFY_STREAM_STATUS, None),
    ];
    for ((name, packet), (sequence, kind, subscription)) in ORIGINAL_GAME_REQUESTS.iter().zip(expected) {
        let buffer = one_message(&hex(packet));
        let message = decode(&buffer).unwrap();
        assert_eq!((message.sequence, message.kind), (sequence, kind), "{name}");
        if let Some(subscription) = subscription {
            let request = initialize_request(message.kind, message.payload.as_ref()).unwrap();
            assert_eq!((request.device_url.as_str(), request.subscription_id), ("tet-tcp://127.0.0.1", subscription));
        }
    }
}

#[test]
fn requests_built_here_match_the_game_builders() {
    let initialize = message(
        2,
        REQUEST_INITIALIZE,
        &[Field::Scalar { index: 1, value: Value::U32(3) }],
        &[(0, b"tet-tcp://127.0.0.1".to_vec())],
    );
    let buffer = one_message(&initialize);
    let decoded = decode(&buffer).unwrap();
    assert_eq!(initialize_request(decoded.kind, decoded.payload.as_ref()).unwrap().subscription_id, 3);
    assert_eq!(message(4, REQUEST_HEADPOSE_START, &[], &[]), hex(ORIGINAL_HEADPOSE_START_REQUEST));
}

fn round_trip(sequence: u32, pose: &HeadPose) -> HeadPose {
    let buffer = one_message(&encode_head_pose(sequence, pose));
    let message = decode(&buffer).unwrap();
    head_pose(message.kind, message.payload.as_ref()).unwrap()
}

#[test]
fn head_pose_round_trip_keeps_defaults_and_negative_time() {
    assert_eq!(round_trip(3, &invalid_pose()), invalid_pose());
    let valid = pose(7, [0.0; 3], [0.0; 3]);
    assert_eq!(round_trip(1, &valid), valid);
}

#[test]
fn capabilities_read_back() {
    let buffer = one_message(&encode_initialize_response(1, 0, &[1, 3]));
    let message = decode(&buffer).unwrap();
    assert_eq!(capabilities(message.kind, message.payload.as_ref()).unwrap(), (0, vec![1, 3]));
    assert!(head_pose(message.kind, message.payload.as_ref()).is_err());
}

#[test]
fn decoder_reassembles_split_and_joined_messages() {
    let first = encode_stream_status_response(1);
    let second = encode_head_pose(2, &pose(9, [1.0, 2.0, 3.0], [0.0; 3]));
    let joined = [first.as_slice(), &second].concat();
    let mut decoder = FrameDecoder::default();
    let mut buffers = decoder.feed(&joined[..5]).unwrap();
    buffers.extend(decoder.feed(&joined[5..40]).unwrap());
    buffers.extend(decoder.feed(&joined[40..]).unwrap());
    let kinds: Vec<u8> = buffers.iter().map(|buffer| decode(buffer).unwrap().kind).collect();
    assert_eq!(kinds, [RESPONSE_NOTIFY_STREAM_STATUS, SUBSCRIPTION_HEADPOSE]);
    assert!(decoder.pending().is_empty());
}

#[test]
fn malformed_input_is_rejected() {
    let is_sesp = |result: Result<()>| matches!(result, Err(ServiceError::Sesp(_)));
    assert!(is_sesp(FrameDecoder::default().feed(&[b"sesq".as_slice(), &[0; 8]].concat()).map(drop)));
    let header = [MAGIC.to_le_bytes(), 4u32.to_le_bytes(), 5u32.to_le_bytes()].concat();
    assert!(is_sesp(FrameDecoder::default().feed(&[header.as_slice(), &[0; 4]].concat()).map(drop)));
    assert!(is_sesp(FrameDecoder::new(3).feed(&frame(&[0; 4])).map(drop)));
    assert!(is_sesp(decode(&200u32.to_le_bytes()).map(drop)));
    assert!(is_sesp(decode(&[0; 3]).map(drop)));
    assert!(is_sesp(decode_hello(&[&0x4E21u32.to_le_bytes()[..], b"client\0"].concat()).map(drop)));
    assert!(is_sesp(decode_hello(&[&COMMAND_CHANNEL.to_le_bytes()[..], b"client"].concat()).map(drop)));
    assert!(is_sesp(decode_hello(&[&COMMAND_CHANNEL.to_le_bytes()[..], b"cli\0nt\0"].concat()).map(drop)));
}

#[test]
fn hello() {
    let hello = decode_hello(&encode_hello(SUBSCRIPTION_CHANNEL, "client_x").unwrap()).unwrap();
    assert_eq!(hello, Hello { channel: SUBSCRIPTION_CHANNEL, reply_pipe: "client_x".into() });
    assert_eq!(encode_slot(3), [3, 0, 0, 0]);
}

#[test]
fn errors_above_seven_are_internal_on_the_wire() {
    assert_eq!((wire_error(7), wire_error(8), wire_error(-1)), (7, ERROR_INTERNAL, ERROR_INTERNAL));
}
