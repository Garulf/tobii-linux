//! Vectors of tests/test_prp_session.py and the original serializer's output for the inputs
//! of tools/prp_session_oracle.py (emulated, not captured traffic).

use super::*;
use crate::error::ServiceError;
use crate::prp::{GazePoint, HeadPose, StreamSample, encode_stream_packet};
use crate::test_util::hex;

const STREAM_START_HEAD: &str = concat!(
    "545052502c000000785052500c00000009000000090000000400000008000000",
    "030000000400000008000000020000000400000000000000",
);
const TIMESYNC_REQUEST: &str = concat!(
    "54505250280000007c5052500c00000005000000090000000400000008000000",
    "1600000004000000040000000400000000000000",
);
const STREAM_STOP_RESPONSE: &str = concat!(
    "5450525030000000645052500c000000050000000a000000040000000c000000",
    "05000000040000000400000008000000020000000400000000000000",
);
const TIMESYNC_RESPONSE: &str = concat!(
    "5450525034000000605052500c000000060000000a000000040000000c000000",
    "0000000016000000040000000c00000003020100070605040400000000000000",
);
const EMPTY_CONNECT_REQUEST: &str = concat!(
    "5450525064000000305052500c000000ffffff7f090000000400000008000000",
    "010000000400000040000000f9ffffff00000000040000000000000000000000",
    "0400000011000000000000000600000000000000000000000000000000000000",
    "01000000020000000400000000000000",
);

/// Connect requests of tools/prp_session_oracle.py by (tag, extended), original serializer output.
const ORIGINAL_CONNECT_REQUESTS: [(u32, bool, &str); 4] = [
    (
        1,
        true,
        concat!(
            "545052501c010000485152500c0000000100000009000000040000000800000001000000040000004000000003000000",
            "03000000040000004600000030000000040000001100000000000000060000002a000000900000001600000078000000",
            "3412000001000000bc0000000f0000001c0000000000000000000000020000002c0000006c6963656e73652d6b65792d",
            "6f6e65006b3200000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728",
            "292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f40414243444546000053007400610072004300690074006900",
            "7a0065006e00000043003a005c00470061006d00650073005c00530074006100720043006900740069007a0065006e00",
            "5c00000000000000",
        ),
    ),
    (
        1,
        false,
        concat!(
            "54505250180100004c5152500c0000000100000009000000040000000800000001000000040000003c00000003000000",
            "03000000040000004600000030000000040000001100000000000000060000002a000000900000001600000078000000",
            "34120000bc0000000f0000001c0000000000000000000000020000002c0000006c6963656e73652d6b65792d6f6e6500",
            "6b3200000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c",
            "2d2e2f303132333435363738393a3b3c3d3e3f404142434445460000530074006100720043006900740069007a006500",
            "6e00000043003a005c00470061006d00650073005c00530074006100720043006900740069007a0065006e005c000000",
            "00000000",
        ),
    ),
    (
        2147483647,
        true,
        concat!(
            "5450525064000000305052500c000000ffffff7f090000000400000008000000010000000400000040000000f9ffffff",
            "000000000400000000000000000000000400000011000000000000000600000000000000000000000000000000000000",
            "01000000020000000400000000000000",
        ),
    ),
    (
        2147483647,
        false,
        concat!(
            "5450525060000000345052500c000000ffffff7f09000000040000000800000001000000040000003c000000f9ffffff",
            "000000000400000000000000000000000400000011000000000000000600000000000000000000000000000000000000",
            "010000000400000000000000",
        ),
    ),
];

/// Stream requests (tag 9) by (command, stream).
const ORIGINAL_STREAM_REQUESTS: [(u32, i32, &str); 3] = [
    (
        3,
        2,
        concat!(
            "545052502c000000785052500c0000000900000009000000040000000800000003000000040000000800000002000000",
            "0400000000000000",
        ),
    ),
    (
        4,
        2,
        concat!(
            "545052502c000000785052500c0000000900000009000000040000000800000004000000040000000800000002000000",
            "0400000000000000",
        ),
    ),
    (
        3,
        29,
        concat!(
            "545052502c000000785052500c000000090000000900000004000000080000000300000004000000080000001d000000",
            "0400000000000000",
        ),
    ),
];

const ORIGINAL_TIMESYNC_REQUEST: &str = concat!(
    "54505250280000007c5052500c0000000500000009000000040000000800000016000000040000000400000004000000",
    "00000000",
);

/// Responses by (tag, extended), in the order of the oracle's inputs.
const ORIGINAL_RESPONSES: [(u32, bool, &str); 12] = [
    (
        1,
        true,
        concat!(
            "5450525094010000c05152500c000000010000000a000000040000000c000000000000000100000004000000a8000000",
            "04000000000000000200000014000000010000001c00000000000000200000000300000020000000030000002c000000",
            "0300000038000000020000004400000068000000700000007800000084000000900000009c000000a000000000000000",
            "010000000200000003000000040000000500000006000000070000000800000003000000a40000000300000002000000",
            "b0000000b8000000bc000000c0000000c4000000c8000000312e322e332e342d74657374000000000100000002000000",
            "03000000fcffffff050000000600000002000000030000001d0000000000000020000000070000000000000003000000",
            "5c00000004000000000000006800000001000000020000000300000073657269616c00006d6f64656c00000067656e65",
            "726174696f6e00006669726d7761726500000000696e746567726174696f6e0078000000790000000000000001000000",
            "0a00000008000000090000006530000065310000000000006533000000000000",
        ),
    ),
    (
        1,
        false,
        concat!(
            "5450525074010000205152500c000000010000000a000000040000000c00000000000000010000000400000098000000",
            "04000000000000000200000014000000010000001c00000000000000200000000300000020000000030000002c000000",
            "0300000038000000020000004400000068000000700000007800000084000000900000009c000000a000000000000000",
            "010000000200000003000000040000000500000006000000070000000800000003000000a40000000300000002000000",
            "b0000000b8000000312e322e332e342d7465737400000000010000000200000003000000fcffffff0500000006000000",
            "02000000030000001d00000000000000200000000700000000000000030000005c000000040000000000000068000000",
            "01000000020000000300000073657269616c00006d6f64656c00000067656e65726174696f6e00006669726d77617265",
            "00000000696e746567726174696f6e00780000007900000000000000010000000a000000080000000900000000000000",
        ),
    ),
    (
        2,
        true,
        concat!(
            "54505250080100005c5152500c000000020000000a000000040000000c000000000000000100000004000000a8000000",
            "0400000000000000000000000c000000000000000c000000000000000c000000000000000c000000010000000c000000",
            "000000001000000000000000100000001000000014000000180000001c00000020000000240000002800000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000000000002c0000000000000000000000",
            "2c0000002c0000003000000034000000380000003c0000006d696e696d616c0002000000000000000000000000000000",
            "000000000000000000000000000000000000000000000000000000000000000000000000",
        ),
    ),
    (
        2,
        false,
        concat!(
            "54505250e8000000bc5052500c000000020000000a000000040000000c00000000000000010000000400000098000000",
            "0400000000000000000000000c000000000000000c000000000000000c000000000000000c000000010000000c000000",
            "000000001000000000000000100000001000000014000000180000001c00000020000000240000002800000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000000000002c0000000000000000000000",
            "2c0000002c0000006d696e696d616c000200000000000000000000000000000000000000000000000000000000000000",
            "00000000",
        ),
    ),
    (
        3,
        true,
        concat!(
            "5450525000010000545152500c000000030000000a000000040000000c000000110000000100000004000000a8000000",
            "040000000000000000000000080000000000000008000000000000000800000000000000080000000000000008000000",
            "00000000080000000000000008000000080000000c0000001000000014000000180000001c0000002000000000000000",
            "000000000000000000000000000000000000000000000000000000000000000000000000240000000000000000000000",
            "2400000024000000280000002c0000003000000034000000000000000000000000000000000000000000000000000000",
            "00000000000000000000000000000000000000000000000000000000",
        ),
    ),
    (
        3,
        false,
        concat!(
            "54505250e0000000b45052500c000000030000000a000000040000000c00000011000000010000000400000098000000",
            "040000000000000000000000080000000000000008000000000000000800000000000000080000000000000008000000",
            "00000000080000000000000008000000080000000c0000001000000014000000180000001c0000002000000000000000",
            "000000000000000000000000000000000000000000000000000000000000000000000000240000000000000000000000",
            "2400000024000000000000000000000000000000000000000000000000000000000000000000000000000000",
        ),
    ),
    (
        4,
        true,
        concat!(
            "5450525030000000645052500c000000040000000a000000040000000c00000000000000030000000400000008000000",
            "020000000400000000000000",
        ),
    ),
    (
        4,
        false,
        concat!(
            "5450525030000000645052500c000000040000000a000000040000000c00000000000000030000000400000008000000",
            "020000000400000000000000",
        ),
    ),
    (
        5,
        true,
        concat!(
            "5450525030000000645052500c000000050000000a000000040000000c00000005000000040000000400000008000000",
            "020000000400000000000000",
        ),
    ),
    (
        5,
        false,
        concat!(
            "5450525030000000645052500c000000050000000a000000040000000c00000005000000040000000400000008000000",
            "020000000400000000000000",
        ),
    ),
    (
        6,
        true,
        concat!(
            "5450525034000000605052500c000000060000000a000000040000000c0000000000000016000000040000000c000000",
            "03020100070605040400000000000000",
        ),
    ),
    (
        6,
        false,
        concat!(
            "5450525034000000605052500c000000060000000a000000040000000c0000000000000016000000040000000c000000",
            "03020100070605040400000000000000",
        ),
    ),
];

fn handshake_info() -> HandshakeInfo {
    HandshakeInfo {
        client_id: 7,
        subscription_url: "tobii-prp://linux-bridge_subscription".into(),
        timesync_url: "tobii-prp://linux-bridge_timesync".into(),
    }
}

fn handshake_info_wire() -> Vec<u8> {
    [&7u32.to_le_bytes()[..], b"tobii-prp://linux-bridge_subscription\0", b"tobii-prp://linux-bridge_timesync\0"]
        .concat()
}

fn is_prp_error<T: std::fmt::Debug>(result: Result<T>) -> bool {
    matches!(result, Err(ServiceError::Prp(_)))
}

#[test]
fn pipe_names() {
    assert_eq!(server_pipe_names("tobii-prp://ET5").unwrap(), ("TOBIIPRP-ET5".into(), "ET5".into()));
    assert_eq!(client_pipe_name("TOBIIPRP-ET5", 0x1A2B, 3), "client_TOBIIPRP-ET5_00001a2b_0000000000000003");
    for bad in ["tobii-prp://", "tprp-tcp://x", "ET5"] {
        assert!(is_prp_error(server_pipe_names(bad)));
    }
}

#[test]
fn client_hello_round_trip_and_rejection() {
    let hello = ClientHello {
        channel: COMMAND_CHANNEL,
        reply_pipe: "client_TOBIIPRP-ET5_00000010_0000000000000001".into(),
        version: HANDSHAKE_VERSION,
    };
    let wire = encode_client_hello(&hello).unwrap();
    assert_eq!(&wire[..8], &[1, 0, 0, 0, 0x21, 0x4E, 0, 0]);
    assert_eq!(decode_client_hello(&wire).unwrap(), hello);
    let version_zero = [&[0u8; 4][..], &0x4E21u32.to_le_bytes(), b"x\0"].concat();
    for bad in [&wire[..8], &wire[..wire.len() - 1], &version_zero] {
        assert!(is_prp_error(decode_client_hello(bad)));
    }
    let long = ClientHello { reply_pipe: "x".repeat(0xF8), ..hello };
    assert!(is_prp_error(encode_client_hello(&long)));
}

#[test]
fn ack_pid_and_info() {
    check_handshake_ack(&encode_handshake_ack()).unwrap();
    for bad in [&[1u8, 0, 0, 0, 0][..], &[2, 0, 0, 0], &[]] {
        assert!(is_prp_error(check_handshake_ack(bad)));
    }
    assert_eq!(decode_client_pid(&encode_client_pid(4321)).unwrap(), 4321);
    let wire = handshake_info_wire();
    assert_eq!(encode_handshake_info(&handshake_info()).unwrap(), wire);
    assert_eq!(decode_handshake_info(&wire).unwrap(), handshake_info());
    assert!(is_prp_error(decode_handshake_info(&wire[..wire.len() - 1])));
    let long = HandshakeInfo { client_id: 1, subscription_url: "a".repeat(200), timesync_url: "b".repeat(60) };
    assert!(is_prp_error(encode_handshake_info(&long)));
    assert!(is_prp_error(encode_handshake_info(&HandshakeInfo { subscription_url: "é".into(), ..handshake_info() })));
}

#[test]
fn chunking_and_reassembly() {
    let head = encode_stream_packet(
        &StreamSample::Head(HeadPose {
            timestamp: 5,
            position_valid: true,
            position: [1.0, 2.0, 3.0],
            rotation_valid: [true; 3],
            rotation: [0.0; 3],
        }),
        0,
        false,
    )
    .unwrap();
    let gaze =
        encode_stream_packet(&StreamSample::Gaze(GazePoint { timestamp: 6, valid: true, x: 0.5, y: 0.5 }), 0, false)
            .unwrap();
    let stream = [head.as_slice(), &gaze].concat();
    let (packets, tail) = split_packets(&stream[..stream.len() - 3], DEFAULT_MAX_PAYLOAD).unwrap();
    assert_eq!((packets, tail), (vec![head.as_slice()], &gaze[..gaze.len() - 3]));
    let (packets, tail) = split_packets(&stream, DEFAULT_MAX_PAYLOAD).unwrap();
    assert_eq!((packets, tail), (vec![head.as_slice(), gaze.as_slice()], &[][..]));
    let big = vec![0; PIPE_CHUNK + 5];
    assert_eq!(pipe_chunks(&big).iter().map(|chunk| chunk.len()).collect::<Vec<_>>(), [PIPE_CHUNK, 5]);
    assert_eq!(pipe_chunks(&[]), [&[] as &[u8]]);
    let mut corrupt = stream.clone();
    corrupt[0] = b'X';
    assert!(is_prp_error(split_packets(&corrupt, DEFAULT_MAX_PAYLOAD)));
}

fn empty_connect() -> ConnectRequest {
    ConnectRequest {
        subscription_client_id: -7,
        licenses: Vec::new(),
        signer_public_key: Vec::new(),
        client_version: vec![4, 17, 0, 6],
        process_id: 1,
        image_directory: Vec::new(),
        image_name: Vec::new(),
        field_of_use: 2,
    }
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn licensed_connect(directory: &str, name: &str) -> ConnectRequest {
    ConnectRequest {
        subscription_client_id: 3,
        licenses: vec![b"license-key-one".to_vec(), Vec::new(), b"k2".to_vec()],
        signer_public_key: (1..71).collect(),
        client_version: vec![4, 17, 0, 6],
        process_id: 0x1234,
        image_directory: utf16(directory),
        image_name: utf16(name),
        field_of_use: 1,
    }
}

#[test]
fn emulated_request_vectors() {
    assert_eq!(encode_stream_request(9, COMMAND_STREAM_START, 2).unwrap(), hex(STREAM_START_HEAD));
    assert_eq!(encode_timesync_request(5), hex(TIMESYNC_REQUEST));
    assert_eq!(encode_connect_request(0x7FFF_FFFF, &empty_connect(), true).unwrap(), hex(EMPTY_CONNECT_REQUEST));
    let decoded = decode_command_request(&hex(STREAM_START_HEAD), true).unwrap();
    assert_eq!((decoded.tag, decoded.command, decoded.stream), (9, 3, Some(2)));
    assert_eq!(decode_command_request(&hex(EMPTY_CONNECT_REQUEST), true).unwrap().connect, Some(empty_connect()));
}

#[test]
fn connect_request_round_trip() {
    let request = licensed_connect("C:\\Games\\", "Game");
    for extended in [true, false] {
        let wire = encode_connect_request(1, &request, extended).unwrap();
        let expected = if extended { request.clone() } else { ConnectRequest { field_of_use: 0, ..request.clone() } };
        assert_eq!(decode_command_request(&wire, extended).unwrap().connect, Some(expected));
    }
    let bad_version = ConnectRequest { client_version: vec![4, 17, 0], ..request.clone() };
    assert!(is_prp_error(encode_connect_request(1, &bad_version, true)));
    let too_many = ConnectRequest { licenses: vec![b"k".to_vec(); 0x101], ..request };
    assert!(is_prp_error(encode_connect_request(1, &too_many, true)));
}

#[test]
fn original_request_vectors() {
    let requests = [(1, licensed_connect("C:\\Games\\StarCitizen\\", "StarCitizen")), (0x7FFF_FFFF, empty_connect())];
    for (tag, extended, packet) in ORIGINAL_CONNECT_REQUESTS {
        let (_, request) = requests.iter().find(|(request_tag, _)| *request_tag == tag).unwrap();
        assert_eq!(encode_connect_request(tag, request, extended).unwrap(), hex(packet));
    }
    for (command, stream, packet) in ORIGINAL_STREAM_REQUESTS {
        assert_eq!(encode_stream_request(9, command, stream).unwrap(), hex(packet));
    }
    assert_eq!(encode_timesync_request(5), hex(ORIGINAL_TIMESYNC_REQUEST));
}

fn word_response(tag: u32, command: u32, status: i32, value: i32) -> CommandResponse {
    CommandResponse { tag, command, status, body: ResponseBody::Word(value) }
}

fn connect_response(tag: u32, status: i32, connect: ConnectResponse) -> CommandResponse {
    CommandResponse { tag, command: COMMAND_CONNECT, status, body: ResponseBody::Connect(Box::new(connect)) }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn full_connect(strings_7: &[&str]) -> ConnectResponse {
    ConnectResponse {
        build_version: "1.2.3.4-test".into(),
        firmware_upgrade_in_progress: false,
        list_a: vec![1, 2],
        list_b: vec![3],
        list_c: vec![],
        list_d: vec![-4, 5, 6],
        streams: vec![2, 3, 29],
        supported_commands: vec![0, 0x20, 7],
        groups: vec![(0, vec![1, 2, 3]), (4, vec![])],
        strings: strings(strings_7),
        words: (0..9).collect(),
        license_results: vec![0, 1, 10],
        mode: 3,
        list_g: vec![8, 9],
        extended_strings: strings(&["e0", "e1", "", "e3"]),
    }
}

#[test]
fn emulated_response_vectors() {
    let stop = word_response(5, COMMAND_STREAM_STOP, 5, 2);
    assert_eq!(encode_command_response(&stop, true).unwrap(), hex(STREAM_STOP_RESPONSE));
    let timesync = CommandResponse {
        tag: 6,
        command: COMMAND_TIMESYNC,
        status: 0,
        body: ResponseBody::Timesync(0x0001020304050607),
    };
    assert_eq!(encode_command_response(&timesync, true).unwrap(), hex(TIMESYNC_RESPONSE));
    assert_eq!(decode_command_response(&hex(TIMESYNC_RESPONSE), true).unwrap(), timesync);
    assert_eq!(decode_command_response(&hex(STREAM_STOP_RESPONSE), true).unwrap().status, 5);
}

#[test]
fn original_response_vectors() {
    let full = full_connect(&["serial", "model", "generation", "firmware", "integration", "x", "y"]);
    let minimal = ConnectResponse { build_version: "minimal".into(), streams: vec![2], ..ConnectResponse::default() };
    let responses = [
        connect_response(1, 0, full),
        connect_response(2, 0, minimal),
        connect_response(3, 17, ConnectResponse::default()),
        word_response(4, COMMAND_STREAM_START, 0, 2),
        word_response(5, COMMAND_STREAM_STOP, 5, 2),
        CommandResponse {
            tag: 6,
            command: COMMAND_TIMESYNC,
            status: 0,
            body: ResponseBody::Timesync(0x0001020304050607),
        },
    ];
    for (tag, extended, packet) in ORIGINAL_RESPONSES {
        let response = responses.iter().find(|response| response.tag == tag).unwrap();
        let wire = encode_command_response(response, extended).unwrap();
        assert_eq!(wire, hex(packet), "tag {tag} extended {extended}");
        let mut expected = response.clone();
        if let (ResponseBody::Connect(connect), false) = (&mut expected.body, extended) {
            connect.extended_strings = vec![String::new(); 4];
        }
        assert_eq!(decode_command_response(&wire, extended).unwrap(), expected);
    }
}

#[test]
fn connect_response_round_trip_and_limits() {
    let message = connect_response(1, 0, full_connect(&["a", "b", "c", "d", "e", "x", "y"]));
    assert_eq!(decode_command_response(&encode_command_response(&message, true).unwrap(), true).unwrap(), message);
    let minimal = connect_response(2, 0, minimal_connect_response("bridge", 2, &[2, 3, 29]));
    let decoded = decode_command_response(&encode_command_response(&minimal, true).unwrap(), true).unwrap();
    let ResponseBody::Connect(decoded) = decoded.body else { panic!("connect body expected") };
    assert_eq!(
        (decoded.streams, decoded.license_results, decoded.firmware_upgrade_in_progress),
        (vec![2, 3, 29], vec![0, 0], false)
    );
    for bad in [
        ConnectResponse { streams: (0..0x21).collect(), ..ConnectResponse::default() },
        ConnectResponse { supported_commands: vec![0x21], ..ConnectResponse::default() },
        ConnectResponse { groups: vec![(5, vec![])], ..ConnectResponse::default() },
        ConnectResponse { build_version: "x".repeat(0x100), ..ConnectResponse::default() },
        ConnectResponse { words: vec![0; 8], ..ConnectResponse::default() },
    ] {
        assert!(is_prp_error(encode_command_response(&connect_response(1, 0, bad), true)));
    }
}

#[test]
fn malformed_messages() {
    assert!(is_prp_error(decode_command_request(&hex(TIMESYNC_RESPONSE), true)));
    assert!(is_prp_error(decode_command_response(&hex(STREAM_START_HEAD), true)));
    let mut truncated = hex(STREAM_START_HEAD);
    truncated[36..40].copy_from_slice(&0x40u32.to_le_bytes());
    assert!(is_prp_error(decode_command_request(&truncated, true)));
    assert!(is_prp_error(encode_stream_request(1, COMMAND_TIMESYNC, 2)));
    assert!(is_prp_error(response_kind(7)));
}

#[test]
fn license_table_outside_its_region_is_a_truncated_read() {
    let mut wire = hex(EMPTY_CONNECT_REQUEST);
    wire[48..52].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(decode_command_request(&wire, true), Err(ServiceError::Truncated(_))));
}

#[test]
fn property_messages_round_trip() {
    let request = encode_property_request(3, PROPERTY_DISPLAY_AREA);
    assert_eq!(decode_property_request(&request).unwrap(), (3, PROPERTY_DISPLAY_AREA));
    let subscribe = encode_property_subscribe_request(4, 0xB, COMMAND_PROPERTY_UNSUBSCRIBE).unwrap();
    assert_eq!(decode_property_subscribe_request(&subscribe).unwrap(), (4, COMMAND_PROPERTY_UNSUBSCRIBE, 0xB));
    assert!(is_prp_error(encode_property_subscribe_request(4, 0xB, 7)));
    assert!(is_prp_error(decode_property_subscribe_request(&request)));
    let corners = [-1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.25];
    let (block, region) = display_area_value(&corners).unwrap();
    let response = PropertyResponse { tag: 3, status: 0, property_id: PROPERTY_DISPLAY_AREA, block, region };
    let decoded = decode_property_response(&encode_property_response(&response)).unwrap();
    assert_eq!(decoded, response);
    assert_eq!(decode_display_area(&decoded).unwrap(), corners);
    let (block, region) = string_value("DISPLAY1").unwrap();
    let response = PropertyResponse { tag: 5, status: 0, property_id: PROPERTY_DISPLAY_ID, block, region };
    let decoded = decode_property_subscribe_response(&encode_property_subscribe_response(&response)).unwrap();
    assert_eq!((decoded.property_id, decode_string_value(&decoded).unwrap()), (PROPERTY_DISPLAY_ID, "DISPLAY1".into()));
    assert!(is_prp_error(display_area_value(&corners[..8])));
}
