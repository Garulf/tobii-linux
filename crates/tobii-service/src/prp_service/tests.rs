//! Real-service vectors of tests/test_prp_service.py (live, Windows, captured with
//! tools/prp_real_service_probe.py) and the game DLL's serializer output for a presence
//! record (tools/presence_oracle.py).

use super::*;
use crate::prp_session::*;
use crate::test_util::hex;

const REAL_STREAM_PACKAGE: &str = concat!(
    "54505250580000000c5052500c00000000000000010000000400000008000000020000000400000034000000cb000000",
    "063ea7e80000000000000000000000000000000000000000000000000000000000000000000000000000000004000000",
    "00000000",
);
const REAL_STREAM_START_RESPONSE: &str = concat!(
    "5450525030000000645052500c000000020000000a000000040000000c00000000000000030000000400000008000000",
    "020000000400000000000000",
);
const REAL_STREAM_STOP_RESPONSE: &str = concat!(
    "5450525030000000645052500c000000030000000a000000040000000c00000000000000040000000400000008000000",
    "020000000400000000000000",
);
const REAL_TIMESYNC_RESPONSE: &str = concat!(
    "5450525034000000605052500c000000010000000a000000040000000c0000000000000016000000040000000c000000",
    "cb000000c01fa7e80400000000000000",
);
const REAL_DISPLAY_AREA_RESPONSE: &str = concat!(
    "5450525050000000045052500c0000000200000006000000040000000c00000000000000010000000400000028000000",
    "5f4f94c38500a34333a3df42a13096438500a34333a3df425f4f94c3fc462441bb6646c00400000000000000",
);
const REAL_DISPLAY_ID_RESPONSE: &str = concat!(
    "545052507c000000285052500c0000000300000006000000040000000c00000000000000120000000400000008000000",
    "0400000050000000444953504c41595c47534d354237465c352635303238413642263026554944343132323123236549",
    "643a3145364437463542304232323031303030363145236f723a3123763a312e3000000000000000",
);
const REAL_MODEL_RESPONSE: &str = concat!(
    "54505250480000001c5052500c0000000900000006000000040000000c00000000000000080000000400000008000000",
    "040000001c0000004953355f4c617267655f457965747261636b65725f35003600000000",
);
const REAL_PROPERTY_4_RESPONSE: &str = concat!(
    "5450525030000000645052500c0000000400000006000000040000000c00000000000000040000000400000008000000",
    "000000000400000000000000",
);
const REAL_EXCLUSIVE_MODE_GET: &str = concat!(
    "5450525030000000645052500c0000000200000006000000040000000c00000000000000050000000400000008000000",
    "000000000400000000000000",
);
const GAME_DISPLAY_AREA_REQUEST: &str =
    "5450525020000000745052500c00000002000000050000000400000008000000010000000400000000000000";
const GAME_PRESENCE_SUBSCRIBE_REQUEST: &str = concat!(
    "545052502c000000785052500c000000040000000900000004000000080000000500000004000000080000000b000000",
    "0400000000000000",
);
const ORIGINAL_PRESENCE_NOTIFICATION: &str = concat!(
    "5450525034000000605052500c000000000000000200000004000000080000000b000000040000001000000022110000",
    "66554433010000000400000000000000",
);
const REAL_SUBSCRIPTION_INFO: &str = "080000000000";
const REAL_TIMESYNC_INFO: &str = "000000000000";
const REAL_DISPLAY_ID: &str = "DISPLAY\\GSM5B7F\\5&5028A6B&0&UID41221##eId:1E6D7F5B0B220100061E#or:1#v:1.0";
/// (tag, property id, response)
const REAL_PROPERTY_REFUSALS: [(u32, u32, &str); 9] = [
    (
        0xd,
        0x0,
        concat!(
            "545052502c000000785052500c0000000d00000006000000040000000c00000002000000000000000400000004000000",
            "0400000000000000",
        ),
    ),
    (
        0x9,
        0x3,
        concat!(
            "5450525030000000645052500c0000000900000006000000040000000c00000002000000030000000400000008000000",
            "000000000400000000000000",
        ),
    ),
    (
        0xa,
        0xa,
        concat!(
            "5450525044000000105052500c0000000a00000006000000040000000c000000020000000a000000040000001c000000",
            "0000000000000000000000000000000000000000000000000400000000000000",
        ),
    ),
    (
        0x2,
        0xb,
        concat!(
            "54505250380000006c5052500c0000000200000006000000040000000c000000020000000b0000000400000010000000",
            "0000000000000000000000000400000000000000",
        ),
    ),
    (
        0x7,
        0xd,
        concat!(
            "5450525034000000605052500c0000000700000006000000040000000c000000020000000d000000040000000c000000",
            "00000000000000000400000000000000",
        ),
    ),
    (
        0x8,
        0xe,
        concat!(
            "5450525030000000645052500c0000000800000006000000040000000c000000020000000e0000000400000008000000",
            "000000000400000000000000",
        ),
    ),
    (
        0x3,
        0x14,
        concat!(
            "54505250380000006c5052500c0000000300000006000000040000000c00000002000000140000000400000010000000",
            "0000000000000000000000000400000000000000",
        ),
    ),
    (
        0xc,
        0x17,
        concat!(
            "545052505c000000085052500c0000000c00000006000000040000000c00000002000000170000000400000034000000",
            "000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "0400000000000000",
        ),
    ),
    (
        0x4,
        0x18,
        concat!(
            "54505250380000006c5052500c0000000400000006000000040000000c00000002000000180000000400000010000000",
            "0000000000000000000000000400000000000000",
        ),
    ),
];
/// (tag, property id, response)
const REAL_SUBSCRIBE_RESPONSES: [(u32, u32, &str); 6] = [
    (
        0x3,
        0x1,
        concat!(
            "545052505c000000085052500c000000030000000a000000040000000c00000000000000050000000400000008000000",
            "0100000004000000280000005f4f94c38500a34333a3df42a13096438500a34333a3df425f4f94c3fc462441bb6646c0",
            "0400000000000000",
        ),
    ),
    (
        0x4,
        0xb,
        concat!(
            "5450525044000000105052500c000000040000000a000000040000000c00000000000000050000000400000008000000",
            "0b0000000400000010000000cd0000005d058c72020000000400000000000000",
        ),
    ),
    (
        0x5,
        0x2,
        concat!(
            "545052503c000000685052500c000000050000000a000000040000000c00000000000000050000000400000008000000",
            "020000000400000008000000000000000400000000000000",
        ),
    ),
    (
        0x6,
        0x4,
        concat!(
            "545052503c000000685052500c000000060000000a000000040000000c00000000000000050000000400000008000000",
            "040000000400000008000000000000000400000000000000",
        ),
    ),
    (
        0x4,
        0x5,
        concat!(
            "545052503c000000685052500c000000040000000a000000040000000c00000000000000050000000400000008000000",
            "050000000400000008000000000000000400000000000000",
        ),
    ),
    (
        0x6,
        0x14,
        concat!(
            "5450525044000000105052500c000000060000000a000000040000000c00000011000000050000000400000008000000",
            "1400000004000000100000000000000000000000000000000400000000000000",
        ),
    ),
];
/// (tag, property id, response)
const REAL_UNSUBSCRIBE_RESPONSES: [(u32, u32, &str); 2] = [
    (
        0x7,
        0x1,
        concat!(
            "5450525030000000645052500c000000070000000a000000040000000c00000000000000060000000400000008000000",
            "010000000400000000000000",
        ),
    ),
    (
        0x8,
        0xb,
        concat!(
            "5450525030000000645052500c000000080000000a000000040000000c00000000000000060000000400000008000000",
            "0b0000000400000000000000",
        ),
    ),
];
/// (tag, property id, response)
const REAL_UNSUBSCRIBE_LICENSED: [(u32, u32, &str); 1] = [(
    0x9,
    0x14,
    concat!(
        "5450525030000000645052500c000000090000000a000000040000000c00000011000000060000000400000008000000",
        "140000000400000000000000",
    ),
)];
const REAL_PRESENCE_SINCE_US: i64 = 0xCD728C055D;

fn real_display_area() -> Vec<f64> {
    let packet = hex(REAL_DISPLAY_AREA_RESPONSE);
    (0..9)
        .map(|index| f64::from(f32::from_le_bytes(packet[48 + 4 * index..52 + 4 * index].try_into().unwrap())))
        .collect()
}

fn presence(platmod: i64) -> UserPresence {
    let mut presence = UserPresence::default();
    presence.record(REAL_PRESENCE_SINCE_US, platmod);
    presence
}

fn source<'a>(area: Option<&'a [f64]>, display_id: Option<&'a str>, presence: &'a UserPresence) -> PropertySource<'a> {
    PropertySource { display_area: area, display_id, calibration_id: 0, presence, field_of_use: None, paused: false }
}

fn reply(source: &PropertySource, tag: u32, property_id: u32) -> Vec<u8> {
    encode_property_response(&answer_property(source, tag, property_id, false).unwrap())
}

#[test]
fn command_responses_match_the_real_service() {
    let word = |tag, command| CommandResponse { tag, command, status: 0, body: ResponseBody::Word(HEAD_POSE as i32) };
    assert_eq!(encode_command_response(&word(2, COMMAND_STREAM_START), true).unwrap(), hex(REAL_STREAM_START_RESPONSE));
    assert_eq!(encode_command_response(&word(3, COMMAND_STREAM_STOP), true).unwrap(), hex(REAL_STREAM_STOP_RESPONSE));
    let timesync =
        CommandResponse { tag: 1, command: COMMAND_TIMESYNC, status: 0, body: ResponseBody::Timesync(0xCB_E8A7_1FC0) };
    assert_eq!(encode_command_response(&timesync, true).unwrap(), hex(REAL_TIMESYNC_RESPONSE));
    for (tag, property_id, expected) in REAL_UNSUBSCRIBE_RESPONSES.iter().chain(&REAL_UNSUBSCRIBE_LICENSED) {
        let status = if LICENSED_SUBSCRIPTIONS.contains(property_id) { STATUS_INSUFFICIENT_LICENSE } else { 0 };
        let response = CommandResponse {
            tag: *tag,
            command: COMMAND_PROPERTY_UNSUBSCRIBE,
            status,
            body: ResponseBody::Word(*property_id as i32),
        };
        assert_eq!(encode_command_response(&response, true).unwrap(), hex(expected));
    }
}

#[test]
fn stream_package_matches_the_real_service() {
    let pose = crate::prp::HeadPose {
        timestamp: 0xCB_E8A7_3E06,
        position_valid: false,
        position: [0.0; 3],
        rotation_valid: [false; 3],
        rotation: [0.0; 3],
    };
    let packet = crate::prp::encode_stream_packet(&crate::prp::StreamSample::Head(pose), 0, false).unwrap();
    assert_eq!(packet, hex(REAL_STREAM_PACKAGE));
}

#[test]
fn handshake_info_without_urls() {
    let info = |client_id| HandshakeInfo { client_id, subscription_url: String::new(), timesync_url: String::new() };
    assert_eq!(encode_handshake_info(&info(8)).unwrap(), hex(REAL_SUBSCRIPTION_INFO));
    assert_eq!(encode_handshake_info(&info(0)).unwrap(), hex(REAL_TIMESYNC_INFO));
}

#[test]
fn requests_match_the_game_dll() {
    assert_eq!(encode_property_request(2, PROPERTY_DISPLAY_AREA), hex(GAME_DISPLAY_AREA_REQUEST));
    assert_eq!(
        encode_property_subscribe_request(4, 0xB, COMMAND_PROPERTY_SUBSCRIBE).unwrap(),
        hex(GAME_PRESENCE_SUBSCRIBE_REQUEST)
    );
}

#[test]
fn display_area_and_id() {
    let (area, presence) = (real_display_area(), UserPresence::default());
    let configured = source(Some(&area), Some(REAL_DISPLAY_ID), &presence);
    assert_eq!(reply(&configured, 2, PROPERTY_DISPLAY_AREA), hex(REAL_DISPLAY_AREA_RESPONSE));
    assert_eq!(reply(&configured, 3, PROPERTY_DISPLAY_ID), hex(REAL_DISPLAY_ID_RESPONSE));
    let decoded = decode_display_area(&decode_property_response(&hex(REAL_DISPLAY_AREA_RESPONSE)).unwrap()).unwrap();
    assert!((decoded[3] - decoded[0] - 597.0).abs() < 1e-3);
    let id = decode_string_value(&decode_property_response(&hex(REAL_DISPLAY_ID_RESPONSE)).unwrap()).unwrap();
    assert_eq!(id, REAL_DISPLAY_ID);
}

#[test]
fn model_name_differs_only_in_padding() {
    let presence = UserPresence::default();
    let got = reply(&source(None, None, &presence), 9, PROPERTY_MODEL);
    let real = hex(REAL_MODEL_RESPONSE);
    assert_eq!(got.len(), real.len());
    assert_eq!(got[..got.len() - 6], real[..real.len() - 6]);
}

#[test]
fn zero_word_properties_and_refusals() {
    let (area, presence) = (real_display_area(), UserPresence::default());
    let configured = source(Some(&area), Some(REAL_DISPLAY_ID), &presence);
    assert_eq!(reply(&configured, 4, 4), hex(REAL_PROPERTY_4_RESPONSE));
    assert_eq!(reply(&configured, 2, 5), hex(REAL_EXCLUSIVE_MODE_GET));
    for (tag, property_id, expected) in REAL_PROPERTY_REFUSALS {
        assert_eq!(reply(&configured, tag, property_id), hex(expected), "property {property_id:#x}");
    }
}

#[test]
fn property_4_follows_the_paused_state() {
    let presence = UserPresence::default();
    let mut paused = source(None, None, &presence);
    paused.paused = true;
    for subscribe in [false, true] {
        let response = answer_property(&paused, 1, PROPERTY_DEVICE_PAUSED, subscribe).unwrap();
        assert_eq!((response.status, response.block), (STATUS_OK, word_value(1).0));
    }
    let running = answer_property(&source(None, None, &presence), 1, PROPERTY_DEVICE_PAUSED, false).unwrap();
    assert_eq!(running.block, word_value(0).0);
    let (block, region) = word_value(1);
    let expected = encode_property_notification(NOTIFICATION_TAG, PROPERTY_DEVICE_PAUSED, &block, &region);
    assert_eq!(paused_notification(true), expected);
}

#[test]
fn unconfigured_display_is_refused() {
    let presence = UserPresence::default();
    for property_id in [PROPERTY_DISPLAY_AREA, PROPERTY_DISPLAY_ID] {
        let unconfigured = source(None, Some(""), &presence);
        assert_eq!(
            answer_property(&unconfigured, 5, property_id, false).unwrap(),
            property_error(5, property_id, STATUS_NOT_SUPPORTED)
        );
    }
}

#[test]
fn subscribe_responses_match_the_real_service() {
    let (area, presence) = (real_display_area(), presence(tobii_protocol::gaze::PLATMOD_AWAY.into()));
    let subscribed = source(Some(&area), None, &presence);
    for (tag, property_id, expected) in REAL_SUBSCRIBE_RESPONSES {
        let response = answer_property(&subscribed, tag, property_id, true).unwrap();
        assert_eq!(encode_property_subscribe_response(&response), hex(expected), "property {property_id:#x}");
    }
}

#[test]
fn presence_follows_the_device() {
    for (platmod, wire) in [(0, 1), (1, 2), (2, 3)] {
        let presence = presence(platmod);
        let response = answer_property(&source(None, None, &presence), 1, 0xB, true).unwrap();
        let since = REAL_PRESENCE_SINCE_US as u64;
        let words: Vec<u32> = [0x10, (since >> 32) as u32, since as u32, wire].to_vec();
        assert_eq!(response.block, words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
    }
}

#[test]
fn presence_is_refused_before_any_record() {
    let presence = UserPresence::default();
    let response = answer_property(&source(None, None, &presence), 1, 0xB, true).unwrap();
    assert_eq!(response, property_error(1, 0xB, STATUS_NOT_SUPPORTED));
    assert!(presence.notification().is_err());
}

#[test]
fn every_record_after_the_first_is_notified() {
    let mut presence = UserPresence::default();
    let notified: Vec<bool> =
        [(10, 0), (20, 1), (30, 1), (40, 2)].iter().map(|(t, v)| presence.record(*t, *v)).collect();
    assert_eq!(notified, [false, true, true, true]);
    assert_eq!((presence.value, presence.since_us, presence.present()), (Some(3), 40, true));
    presence.clear();
    assert!(!presence.record(50, 2));
}

#[test]
fn notification_matches_the_original_serializer() {
    let mut presence = UserPresence::default();
    presence.record(0x112233445566, 0);
    assert_eq!(presence.notification().unwrap(), hex(ORIGINAL_PRESENCE_NOTIFICATION));
}

#[test]
fn subscribe_time_leads_the_tracker_by_30_ms() {
    assert_eq!(presence_subscribe_time(900_000_000_000), 899_999_970_000);
    assert_eq!(presence_subscribe_time(12_345), 12_345);
}

#[test]
fn subscribe_only_properties() {
    let presence = UserPresence::default();
    let mut subscribed = source(None, None, &presence);
    subscribed.calibration_id = 0x1234;
    subscribed.field_of_use = Some(2);
    let words = |response: PropertyResponse| response.block;
    assert_eq!(words(answer_property(&subscribed, 1, PROPERTY_CALIBRATION_ID, true).unwrap()), word_value(0x1234).0);
    assert_eq!(words(answer_property(&subscribed, 1, PROPERTY_FIELD_OF_USE, true).unwrap()), word_value(2).0);
    assert_eq!(answer_property(&subscribed, 1, PROPERTY_TRACK_BOX, true).unwrap().block.len(), 0x64);
    assert_eq!(answer_property(&subscribed, 1, PROPERTY_TRACK_BOX, false).unwrap().status, STATUS_NOT_SUPPORTED);
    let faults = answer_property(&subscribed, 1, PROPERTY_FAULTS, true).unwrap();
    assert_eq!(decode_string_value(&faults).unwrap(), TRACKER_STATUS_TEXT);
    let licensed = answer_property(&subscribed, 1, PROPERTY_LENS_CONFIGURATION, true).unwrap();
    assert_eq!((licensed.status, licensed.block.len()), (STATUS_INSUFFICIENT_LICENSE, 0x18));
    assert_eq!(answer_property(&subscribed, 1, PROPERTY_MODEL, true).unwrap().block, string_value("").unwrap().0);
}

#[test]
fn relay_framing() {
    let frame = encode_frame(FRAME_MESSAGE, b"abc").unwrap();
    assert_eq!(decode_frame_header(&frame[..8]).unwrap(), (FRAME_MESSAGE, 3));
    assert!(encode_frame(FRAME_MESSAGE, &vec![0; MAX_FRAME + 1]).is_err());
    let oversized = [FRAME_MESSAGE.to_le_bytes(), ((MAX_FRAME + 1) as u32).to_le_bytes()].concat();
    assert!(decode_frame_header(&oversized).is_err());
    assert_eq!(decode_names(&encode_names(&["a", "", "bc"]).unwrap()).unwrap(), ["a", "", "bc"]);
    assert!(decode_names(b"").unwrap().is_empty());
    assert!(decode_names(b"a").is_err());
    let open = encode_pipe_open(0x1234, "TOBIIPRP-ET5").unwrap();
    assert_eq!(decode_pipe_open(&open).unwrap(), (0x1234, "TOBIIPRP-ET5".to_string()));
    let mut wrong_version = open.clone();
    wrong_version[0] = 2;
    assert!(decode_pipe_open(&wrong_version).is_err());
    assert!(decode_pipe_open(&open[..8]).is_err());
}

#[test]
fn unknown_commands_get_the_generic_error() {
    let request = encode_stream_request(9, COMMAND_STREAM_START, 2).unwrap();
    assert_eq!(packet_operation(&request).unwrap(), OPERATION_COMMAND_REQUEST);
    assert_eq!(request_header(&request).unwrap(), (9, COMMAND_STREAM_START));
    let response = error_response(9, 2, STATUS_NOT_SUPPORTED);
    let decoded = decode_command_response(&response, true);
    assert!(decoded.is_err());
    let payload = crate::prp::decode_envelope(&response, crate::prp::DEFAULT_MAX_PAYLOAD).unwrap();
    assert_eq!(le_u32(payload, 20), STATUS_NOT_SUPPORTED as u32);
    assert!(request_header(&encode_property_request(1, 1)).is_err());
}

#[test]
fn connect_response_carries_the_streams_and_licenses() {
    let response = device_connect_response("SERIAL", 2, &GAZE_SERVICE_STREAMS.map(|s| s as i32));
    let message = CommandResponse {
        tag: 1,
        command: COMMAND_CONNECT,
        status: 0,
        body: ResponseBody::Connect(Box::new(response)),
    };
    let decoded = decode_command_response(&encode_command_response(&message, true).unwrap(), true).unwrap();
    let ResponseBody::Connect(connect) = decoded.body else { panic!("connect expected") };
    assert_eq!(
        (connect.license_results, connect.streams, connect.strings[0].as_str()),
        (vec![0, 0], vec![6, 3, 29, 4, 2], "SERIAL")
    );
    assert_eq!(connect.words[1], 184.0f32.to_bits() as i32);
}
