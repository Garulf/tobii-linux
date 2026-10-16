//! The policy and restamp cases of tests/test_tet_tcp.py and tests/test_tet_tcp_service.py,
//! with a real device-info reply (evidence/private/gaze-capture/device-query.jsonl, live).

use tobii_protocol::commands::{STREAM_DATA_KIND, Stream, header_only_request, subscribe_stream, unsubscribe_stream};
use tobii_protocol::gaze::build::{columns_payload, int32_column, int64_column, uint32_column, vec2_column};
use tobii_protocol::gaze::{decode_gaze_message, presence_message};

use super::*;
use crate::test_util::hex;

const DEVICE_INFO_PAYLOAD: &str = concat!(
    "000014000000160000001249533546462d313030323034313431343633140000001a000000164953355f4c61",
    "7267655f457965747261636b65725f35140000000700000003495335140000000e0000000a30326131613661",
    "393737",
);

fn gaze_payload(timestamp: i64) -> Vec<u8> {
    columns_payload(&[(1, int64_column(timestamp)), (27, uint32_column(1)), (28, vec2_column([0.25, 0.75]).unwrap())])
        .unwrap()
}

#[test]
fn policy_contains_no_known_write_command() {
    let writes = [
        command::SET_OUTPUT_FREQUENCY,
        command::SET_FREQUENCIES,
        command::SET_ILLUMINATION_MODE,
        command::POWER_SAVE_ACTIVATE,
        command::POWER_SAVE_DEACTIVATE,
        command::PAUSE_DEVICE,
    ];
    assert!(writes.iter().all(|write| !READ_COMMANDS.contains(write)));
}

#[test]
fn subscribed_stream_reads_the_wire_id() {
    assert_eq!(subscribed_stream(&unsubscribe_stream(1, Stream::Gaze.index()).unwrap()), Some(GAZE_STREAM));
    assert_eq!(subscribed_stream(&tobii_protocol::commands::request(1, command::SUBSCRIBE_STREAM, &[]).unwrap()), None);
    let garbage = Message::new(REQUEST_KIND, 1, 0, command::SUBSCRIBE_STREAM, 0, vec![0, 0, 2]);
    assert_eq!(subscribed_stream(&garbage), None);
}

#[test]
fn only_the_timestamp_changes() {
    let original = gaze_payload(908279389020);
    let payload = restamp(&original, 43716142484).unwrap();
    assert_eq!(payload.len(), original.len());
    let decoded = decode_gaze_message(&payload).unwrap().unwrap();
    assert_eq!((decoded.timestamp, decoded.combined_point), (43716142484, [0.25, 0.75]));
    assert_eq!(restamp(&payload, 908279389020).unwrap(), original);
}

#[test]
fn presence_and_negative_times() {
    let payload = columns_payload(&[(1, int64_column(5)), (2, int32_column(2))]).unwrap();
    let presence = presence_message(&restamp(&payload, -7).unwrap()).unwrap().unwrap();
    assert_eq!((presence.timestamp_us, presence.value), (-7, 2));
}

#[test]
fn message_without_timestamp_is_rejected() {
    let payload = columns_payload(&[(27, uint32_column(1))]).unwrap();
    assert!(matches!(restamp(&payload, 1), Err(ServiceError::Protocol(_))));
    assert!(matches!(restamp(&[0, 0], 1), Err(ServiceError::Protocol(_))));
}

fn device() -> (HashMap<u32, Message>, BTreeSet<u32>) {
    let reply = |command, status, payload: &str| Message::new(RESPONSE_KIND, 0, 1, command, status, hex(payload));
    let replies = HashMap::from([
        (command::PROTOCOL_UPGRADE, reply(1000, 0, "0000020000000400010008")),
        (command::GET_DEVICE_INFO, reply(1420, 0, DEVICE_INFO_PAYLOAD)),
        (command::GET_DEVICE_PROPERTIES, reply(1330, UNSUPPORTED_OPERATION, "")),
    ]);
    (replies, BTreeSet::from([GAZE_STREAM, PRESENCE_STREAM, 0x501]))
}

#[test]
fn startup_and_getters_come_from_the_device_replies() {
    let (replies, served) = device();
    let Answer::Reply(info) = answer(&header_only_request(8, command::GET_DEVICE_INFO).unwrap(), &replies, &served)
    else {
        panic!("reply expected")
    };
    assert_eq!((info.kind, info.request_id, info.word4, info.payload), (RESPONSE_KIND, 8, 0, hex(DEVICE_INFO_PAYLOAD)));
    let Answer::Reply(properties) =
        answer(&header_only_request(9, command::GET_DEVICE_PROPERTIES).unwrap(), &replies, &served)
    else {
        panic!("reply expected")
    };
    assert_eq!((properties.request_id, properties.word4), (9, UNSUPPORTED_OPERATION));
}

#[test]
fn writes_and_unknown_commands_are_refused() {
    let (replies, served) = device();
    for request in [1440, command::SET_FREQUENCIES, 3000, command::GET_TRACK_BOX] {
        let message = Message::new(REQUEST_KIND, 11, 0, request, 0, vec![0, 0]);
        let Answer::Refuse { reply, stream: None, subscription: false } = answer(&message, &replies, &served) else {
            panic!("refusal expected")
        };
        assert_eq!((reply.command, reply.word4), (request, UNSUPPORTED_OPERATION));
    }
    let stream = Message::new(STREAM_DATA_KIND, 0, 0, GAZE_STREAM, 0, Vec::new());
    assert_eq!(answer(&stream, &replies, &served), Answer::Ignore);
}

#[test]
fn subscriptions_change_only_the_connection() {
    let (replies, served) = device();
    let subscribed = answer(&subscribe_stream(12, Stream::Gaze.index(), &[]).unwrap(), &replies, &served);
    let Answer::Subscribe { reply, stream: GAZE_STREAM, subscribe: true } = subscribed else {
        panic!("subscription expected")
    };
    assert_eq!((reply.request_id, reply.word2, reply.word4), (12, REPLY_WORD2, 0));
    let image = answer(&subscribe_stream(13, Stream::Image.index(), &[]).unwrap(), &replies, &served);
    let Answer::Refuse { reply, subscription: true, .. } = image else { panic!("refusal expected") };
    assert_eq!(reply.word4, UNSUPPORTED_OPERATION);
    let unsubscribed = answer(&unsubscribe_stream(14, Stream::Gaze.index()).unwrap(), &replies, &served);
    assert!(matches!(unsubscribed, Answer::Subscribe { stream: GAZE_STREAM, subscribe: false, .. }));
}
