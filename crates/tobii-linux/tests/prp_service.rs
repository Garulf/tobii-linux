//! PRP service over the relay: the checks of tests/test_prp_service.py, driven by a
//! simulated bridge with the client-side codecs. REAL_* vectors were captured live from the
//! real ET5 service (tools/prp_real_service_probe.py).

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::*;
use tobii_headpose::pipeline::HeadPoseSample;
use tobii_linux::service::clock::{Clocks, now_us, wine_clock};
use tobii_linux::service::enabled::EnabledState;
use tobii_linux::service::{Options, PrpService, gaze_package, stream_package, synthetic::synthetic_gaze_frame};
use tobii_protocol::gaze::{PLATMOD_AWAY, PLATMOD_PRESENT};
use tobii_service::prp::{DEFAULT_MAX_PAYLOAD, HEAD_POSE, decode_envelope, decode_gaze_body, decode_head_body};
use tobii_service::prp_service::{
    FRAME_BRIDGE_HELLO, FRAME_DISPLAY_ID, FRAME_PIPES, GAZE_SERVICE_STREAMS, PROPERTY_DEVICE_PAUSED, decode_names,
    encode_frame, error_response, paused_notification, word_value,
};
use tobii_service::prp_session::{
    COMMAND_CHANNEL, COMMAND_CONNECT, COMMAND_PROPERTY_SUBSCRIBE, COMMAND_PROPERTY_UNSUBSCRIBE, COMMAND_STREAM_START,
    COMMAND_STREAM_STOP, COMMAND_TIMESYNC, CONNECT_RESPONSE_BLOCK_EXTENDED, ClientHello, ConnectRequest, HandshakeInfo,
    PROPERTY_DISPLAY_AREA, PROPERTY_DISPLAY_ID, ResponseBody, SUBSCRIPTION_CHANNEL, TIMESYNC_CHANNEL,
    check_handshake_ack, decode_command_response, decode_display_area, decode_property_response,
    decode_property_subscribe_response, decode_string_value, encode_client_hello, encode_client_pid,
    encode_connect_request, encode_property_request, encode_property_subscribe_request, encode_stream_request,
    encode_timesync_request,
};
use tobii_service::prp_streams::{PRP_GAZE_ORIGIN, PRP_GAZE_POINT, PRP_RESPONSIVE_GAZE_POINT};
use tobii_service::qpc_clock::{ClockReader, LinuxClock, LinuxClocks};
use tokio::io::AsyncWriteExt;

const REAL_TIMESYNC_RESPONSE: &str = "5450525034000000605052500c000000010000000a000000040000000c0000000000000016000000040000000c000000cb000000c01fa7e80400000000000000";
const REAL_DISPLAY_ID: &str = "DISPLAY\\GSM5B7F\\5&5028A6B&0&UID41221##eId:1E6D7F5B0B220100061E#or:1#v:1.0";
const REAL_PRESENCE_SINCE_US: i64 = 0xCD_728C_055D;
const REAL_UNSUBSCRIBE_LICENSED: (u32, u32, &str) = (
    9,
    0x14,
    "5450525030000000645052500c000000090000000a000000040000000c00000011000000060000000400000008000000140000000400000000000000",
);
const REAL_DISPLAY_AREA_RESPONSE: &str = "5450525050000000045052500c0000000200000006000000040000000c000000000000000100000004000000280000005f4f94c38500a34333a3df42a13096438500a34333a3df425f4f94c3fc462441bb6646c00400000000000000";
const ALBEDO_AREA: [[f64; 3]; 3] = [[-296.62, 326.0, 111.82], [300.38, 326.0, 111.82], [-296.62, 10.27, -3.1]];

/// The corners of the real service's display area answer.
fn real_area() -> [[f64; 3]; 3] {
    let bytes = hex(REAL_DISPLAY_AREA_RESPONSE);
    let floats: Vec<f64> =
        bytes[48..84].chunks(4).map(|c| f64::from(f32::from_le_bytes(c.try_into().unwrap()))).collect();
    [[floats[0], floats[1], floats[2]], [floats[3], floats[4], floats[5]], [floats[6], floats[7], floats[8]]]
}
const REAL_GAZE_STREAM_RESPONSES: [(u32, u32, i32, &str); 7] = [
    (2, COMMAND_STREAM_START, 3, "0000000003000000040000000800000003000000"),
    (3, COMMAND_STREAM_START, 29, "000000000300000004000000080000001d000000"),
    (4, COMMAND_STREAM_START, 4, "0000000003000000040000000800000004000000"),
    (5, COMMAND_STREAM_START, 6, "1100000003000000040000000800000006000000"),
    (6, COMMAND_STREAM_START, 5, "0200000003000000040000000800000005000000"),
    (7, COMMAND_STREAM_START, 33, "1100000003000000040000000800000021000000"),
    (8, COMMAND_STREAM_STOP, 3, "0000000004000000040000000800000003000000"),
];

struct Fixture {
    service: PrpService,
    port: u16,
}

impl Fixture {
    async fn new(options: Options) -> Self {
        let (service, port, _) = service(options, fixed_clock()).await;
        let fixture = Self { service, port };
        fixture.attach();
        fixture
    }

    async fn default() -> Self {
        Self::new(common::options("ET5-TEST")).await
    }

    fn attach(&self) {
        self.service.attach_tracker(None);
        self.service.publish_presence(REAL_PRESENCE_SINCE_US, i64::from(PLATMOD_AWAY));
    }

    async fn channel(&self, pipe: &str) -> RelayedChannel {
        RelayedChannel::open(self.port, pipe, CLIENT_PID).await
    }

    /// A connected client: command and subscription channels, the next tag, and the connect response.
    async fn session_with(&self, licenses: Vec<Vec<u8>>, field_of_use: i32) -> Session {
        let mut command = self.channel("ET5-TEST").await;
        let info = command.handshake(COMMAND_CHANNEL, "client_TOBIIPRP-ET5-TEST_00001234_1", CLIENT_PID).await;
        let mut subscription = self.channel(info.subscription_url.strip_prefix("prp-internal://").unwrap()).await;
        let sub_info = subscription.handshake(SUBSCRIPTION_CHANNEL, "client_sub", CLIENT_PID).await;
        let request = ConnectRequest {
            subscription_client_id: sub_info.client_id,
            licenses,
            process_id: CLIENT_PID,
            image_directory: "\\Games\\".encode_utf16().flat_map(u16::to_le_bytes).collect(),
            image_name: "StarCitizen".encode_utf16().flat_map(u16::to_le_bytes).collect(),
            field_of_use,
            ..ConnectRequest::default()
        };
        command.send_packet(&encode_connect_request(1, &request, true).unwrap()).await;
        let connect = command.packet().await;
        Session { command, subscription, info, tag: 1, connect }
    }

    async fn session(&self) -> Session {
        self.session_with(Vec::new(), 1).await
    }
}

struct Session {
    command: RelayedChannel,
    subscription: RelayedChannel,
    info: HandshakeInfo,
    tag: u32,
    connect: Vec<u8>,
}

impl Session {
    fn next_tag(&mut self) -> u32 {
        self.tag += 1;
        self.tag
    }

    async fn stream_request(
        &mut self,
        command: u32,
        stream: u32,
    ) -> (u32, tobii_service::prp_session::CommandResponse) {
        let tag = self.next_tag();
        self.command.send_packet(&encode_stream_request(tag, command, stream as i32).unwrap()).await;
        (tag, decode_command_response(&self.command.packet().await, true).unwrap())
    }
}

fn word(response: &tobii_service::prp_session::CommandResponse) -> i32 {
    match response.body {
        ResponseBody::Word(value) => value,
        _ => panic!("not a word response"),
    }
}

fn sample(timestamp_us: i64, position: [f32; 3], rotation: [f32; 3]) -> HeadPoseSample {
    HeadPoseSample { timestamp_us, valid: true, position_mm: position, rotation_rad: rotation }
}

#[test]
fn bridge_learns_pipe_names() {
    run(async {
        let fixture = Fixture::default().await;
        let mut bridge = tokio::net::TcpStream::connect(("127.0.0.1", fixture.port)).await.unwrap();
        bridge.write_all(&encode_frame(FRAME_BRIDGE_HELLO, &1u32.to_le_bytes()).unwrap()).await.unwrap();
        let (kind, payload) = read_frame(&mut bridge).await.unwrap();
        assert_eq!(kind, FRAME_PIPES);
        let names = decode_names(&payload).unwrap();
        assert_eq!(names[..2], ["TOBIIPRP-ET5-TEST", "ET5-TEST"]);
        assert_eq!(names, fixture.service.pipe_names());
    });
}

impl Fixture {
    async fn display_id_answer(&self) -> String {
        let mut session = self.session().await;
        let tag = session.next_tag();
        session.command.send_packet(&encode_property_request(tag, PROPERTY_DISPLAY_ID)).await;
        decode_string_value(&decode_property_response(&session.command.packet().await).unwrap()).unwrap()
    }

    async fn bridge_reports_display_id(&self, display_id: &str) {
        let mut bridge = Bridge::attach(self.port).await;
        bridge.send(FRAME_DISPLAY_ID, display_id.as_bytes()).await;
        assert!(eventually(|| self.service.reported_display_id().as_deref() == Some(display_id)).await);
    }
}

#[test]
fn bridge_reported_display_id_is_used_unless_configured() {
    run(async {
        let fixture = Fixture::default().await;
        fixture.bridge_reports_display_id("DISPLAY\\DEFAULT_MONITOR\\0000&0000").await;
        assert_eq!(fixture.display_id_answer().await, "DISPLAY\\DEFAULT_MONITOR\\0000&0000");
        let configured =
            Fixture::new(Options { display_id: Some(REAL_DISPLAY_ID.into()), ..common::options("ET5-TEST") }).await;
        configured.bridge_reports_display_id("DISPLAY\\DEFAULT_MONITOR\\0000&0000").await;
        assert_eq!(configured.display_id_answer().await, REAL_DISPLAY_ID);
    });
}

#[test]
fn handshake_matches_real_service_shape() {
    run(async {
        let fixture = Fixture::default().await;
        let mut command = fixture.channel("TOBIIPRP-ET5-TEST").await;
        let info = command.handshake(COMMAND_CHANNEL, "client_TOBIIPRP-ET5-TEST_00001234_1", CLIENT_PID).await;
        assert_eq!(command.reply_pipe.as_deref(), Some("client_TOBIIPRP-ET5-TEST_00001234_1"));
        assert!(info.client_id > 0);
        assert!(
            info.subscription_url.starts_with("prp-internal://") && info.timesync_url.starts_with("prp-internal://")
        );
        assert!(info.subscription_url.len().max(info.timesync_url.len()) < 128);
        let names = fixture.service.pipe_names();
        let mut subscription = fixture.channel(&names[2]).await;
        let sub_info = subscription.handshake(SUBSCRIPTION_CHANNEL, "client_sub", CLIENT_PID).await;
        assert_eq!((sub_info.subscription_url.as_str(), sub_info.timesync_url.as_str()), ("", ""));
        assert_ne!(sub_info.client_id, -1);
        let mut timesync = fixture.channel(&names[3]).await;
        assert_eq!(timesync.handshake(TIMESYNC_CHANNEL, "client_ts", CLIENT_PID).await.client_id, 0);
    });
}

async fn connect_satisfies_device_create_with(fixture: Fixture) {
    for licenses in [Vec::new(), vec![b"license-one".to_vec(), b"license-two".to_vec()]] {
        let count = licenses.len();
        let session = fixture.session_with(licenses, 1).await;
        let payload = decode_envelope(&session.connect, DEFAULT_MAX_PAYLOAD).unwrap();
        let response = decode_command_response(&session.connect, true).unwrap();
        assert_eq!((response.tag, response.command, response.status), (1, COMMAND_CONNECT, 0));
        assert_eq!(u32::from_le_bytes(payload[32..36].try_into().unwrap()) as usize, CONNECT_RESPONSE_BLOCK_EXTENDED);
        let ResponseBody::Connect(connect) = response.body else { panic!("connect body") };
        assert_eq!(connect.license_results, vec![0; count]);
        assert!(!connect.firmware_upgrade_in_progress);
        assert!(connect.streams.contains(&(HEAD_POSE as i32)));
        assert!(connect.supported_commands.contains(&(COMMAND_TIMESYNC as i32)));
    }
}

#[test]
fn connect_satisfies_device_create() {
    run(async { connect_satisfies_device_create_with(Fixture::default().await).await });
}

#[test]
fn gaze_connect_satisfies_device_create() {
    run(async { connect_satisfies_device_create_with(gaze_fixture().await).await });
}

#[test]
fn connect_with_unknown_subscription_fails() {
    run(async {
        let fixture = Fixture::default().await;
        let mut command = fixture.channel("ET5-TEST").await;
        command.handshake(COMMAND_CHANNEL, "client_cmd", CLIENT_PID).await;
        command
            .send_packet(
                &encode_connect_request(
                    1,
                    &ConnectRequest { subscription_client_id: -1, ..ConnectRequest::default() },
                    true,
                )
                .unwrap(),
            )
            .await;
        assert_ne!(decode_command_response(&command.packet().await, true).unwrap().status, 0);
    });
}

async fn stream_start_pushes_samples_until_stop_with(fixture: Fixture) {
    let mut session = fixture.session().await;
    let (tag, response) = session.stream_request(COMMAND_STREAM_START, HEAD_POSE).await;
    assert_eq!((response.tag, response.status, word(&response)), (tag, 0, HEAD_POSE as i32));
    let pose = sample(FIXED_CLOCK_US + 5, [1.0, 2.0, 600.0], [0.1, -0.2, 0.3]);
    fixture.service.hub().publish(&pose);
    let package = session.subscription.packet().await;
    assert_eq!(package, stream_package(&pose));
    let payload = decode_envelope(&package, DEFAULT_MAX_PAYLOAD).unwrap();
    let words: Vec<u32> = payload[..28].chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    assert_eq!(words, [12, 0, 1, 4, 8, HEAD_POSE, 4]);
    let body = decode_head_body(&payload[28..80]).unwrap();
    assert_eq!((body.timestamp, body.rotation_valid), (pose.timestamp_us, [true; 3]));
    assert!((body.rotation[1] + 0.2).abs() < 1e-6);
    let (_, response) = session.stream_request(COMMAND_STREAM_STOP, HEAD_POSE).await;
    assert_eq!((response.status, word(&response)), (0, HEAD_POSE as i32));
    assert_eq!(fixture.service.hub().subscribers(), 0);
    fixture.service.hub().publish(&pose);
    assert_eq!(session.subscription.packet_within(Duration::from_millis(200)).await, None);
}

#[test]
fn stream_start_pushes_samples_until_stop() {
    run(async { stream_start_pushes_samples_until_stop_with(Fixture::default().await).await });
}

#[test]
fn gaze_stream_start_pushes_samples_until_stop() {
    run(async { stream_start_pushes_samples_until_stop_with(gaze_fixture().await).await });
}

async fn client_disconnect_releases_stream_with(fixture: Fixture) {
    let mut session = fixture.session().await;
    session.stream_request(COMMAND_STREAM_START, HEAD_POSE).await;
    assert_eq!(fixture.service.hub().subscribers(), 1);
    drop(session);
    assert!(eventually(|| fixture.service.hub().subscribers() == 0 && fixture.service.subscriptions() == 0).await);
}

#[test]
fn client_disconnect_releases_stream() {
    run(async { client_disconnect_releases_stream_with(Fixture::default().await).await });
}

#[test]
fn gaze_client_disconnect_releases_stream() {
    run(async { client_disconnect_releases_stream_with(gaze_fixture().await).await });
}

#[test]
fn invalid_samples_are_flagged_invalid() {
    let package = stream_package(&HeadPoseSample {
        timestamp_us: 7,
        valid: false,
        position_mm: [0.0; 3],
        rotation_rad: [0.0; 3],
    });
    let body = decode_head_body(&decode_envelope(&package, DEFAULT_MAX_PAYLOAD).unwrap()[28..80]).unwrap();
    assert!(!body.position_valid);
    assert_eq!(body.rotation_valid, [false; 3]);
}

#[test]
fn stream_requests_for_other_streams_and_before_start_fail() {
    run(async {
        let fixture = Fixture::default().await;
        let mut session = fixture.session().await;
        assert_ne!(session.stream_request(COMMAND_STREAM_START, 3).await.1.status, 0);
        assert_ne!(session.stream_request(COMMAND_STREAM_STOP, HEAD_POSE).await.1.status, 0);
    });
}

#[test]
fn timesync_on_its_channel_matches_the_real_service() {
    run(async {
        let fixture = Fixture::default().await;
        let mut timesync = fixture.channel(&fixture.service.pipe_names()[3]).await;
        timesync.handshake(TIMESYNC_CHANNEL, "client_ts", CLIENT_PID).await;
        timesync.send_packet(&encode_timesync_request(1)).await;
        let packet = timesync.packet().await;
        let response = decode_command_response(&packet, true).unwrap();
        assert_eq!((response.tag, response.command, response.status), (1, COMMAND_TIMESYNC, 0));
        assert_eq!(response.body, ResponseBody::Timesync(FIXED_CLOCK_US as u64));
        assert_eq!(packet, hex(REAL_TIMESYNC_RESPONSE));
    });
}

#[test]
fn unknown_command_gets_nonzero_status_with_same_tag() {
    run(async {
        let fixture = Fixture::default().await;
        let mut session = fixture.session().await;
        let mut payload = Vec::new();
        for word in [12u32, 9, 9, 4, 8, 2, 4, 0x10, 0, 0, 0, 4, 0] {
            payload.extend(word.to_le_bytes());
        }
        let request = tobii_service::prp::encode_envelope(&payload, DEFAULT_MAX_PAYLOAD).unwrap();
        assert_eq!(tobii_service::prp_service::request_header(&request).unwrap(), (9, 2));
        session.command.send_packet(&request).await;
        let packet = session.command.packet().await;
        assert_eq!(packet, error_response(9, 2, 2));
        let payload = decode_envelope(&packet, DEFAULT_MAX_PAYLOAD).unwrap();
        let words: Vec<u32> = payload[..28].chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(words, [12, 9, 10, 4, 12, 2, 2]);
    });
}

#[test]
fn property_get_and_subscribe_keep_the_session() {
    run(async {
        let fixture = Fixture::new(Options { display_area: Some(ALBEDO_AREA), ..common::options("ET5-TEST") }).await;
        let mut session = fixture.session().await;
        let tag = session.next_tag();
        session.command.send_packet(&encode_property_request(tag, PROPERTY_DISPLAY_AREA)).await;
        let response = decode_property_response(&session.command.packet().await).unwrap();
        assert_eq!((response.tag, response.status, response.property_id), (tag, 0, PROPERTY_DISPLAY_AREA));
        let corners = decode_display_area(&response).unwrap();
        assert!((corners[0] + 296.62).abs() < 1e-4 && (corners[8] + 3.1).abs() < 1e-4);
        let tag = session.next_tag();
        session.command.send_packet(&encode_property_request(tag, PROPERTY_DISPLAY_ID)).await;
        let response = decode_property_response(&session.command.packet().await).unwrap();
        assert_eq!((response.tag, response.status), (tag, 2));
        let tag = session.next_tag();
        session
            .command
            .send_packet(&encode_property_subscribe_request(tag, 0xB, COMMAND_PROPERTY_SUBSCRIBE).unwrap())
            .await;
        let response = decode_property_subscribe_response(&session.command.packet().await).unwrap();
        assert_eq!((response.tag, response.status, response.property_id), (tag, 0, 0xB));
        let tag = session.next_tag();
        session
            .command
            .send_packet(&encode_property_subscribe_request(tag, 0xB, COMMAND_PROPERTY_UNSUBSCRIBE).unwrap())
            .await;
        let response = decode_command_response(&session.command.packet().await, true).unwrap();
        assert_eq!(
            (response.tag, response.command, response.status, word(&response)),
            (tag, COMMAND_PROPERTY_UNSUBSCRIBE, 0, 0xB)
        );
        assert_eq!(session.stream_request(COMMAND_STREAM_START, HEAD_POSE).await.1.status, 0);
        fixture.service.hub().publish(&sample(FIXED_CLOCK_US, [0.0; 3], [0.0; 3]));
        assert!(!session.subscription.packet().await.is_empty());
    });
}

#[test]
fn unsupported_property_subscribe_carries_a_value_block() {
    run(async {
        let fixture = Fixture::default().await;
        let mut session = fixture.session().await;
        let tag = session.next_tag();
        session
            .command
            .send_packet(&encode_property_subscribe_request(tag, 3, COMMAND_PROPERTY_SUBSCRIBE).unwrap())
            .await;
        let response = decode_property_subscribe_response(&session.command.packet().await).unwrap();
        assert_eq!((response.tag, response.status, response.property_id), (tag, 2, 3));
        assert_eq!(response.block, [8u32.to_le_bytes(), 0u32.to_le_bytes()].concat());
    });
}

#[test]
fn every_property_subscription_matches_the_real_service() {
    let real: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/data/real-property-subscriptions.json")).unwrap();
    run(async {
        let options = Options {
            display_area: Some(real_area()),
            display_id: Some(REAL_DISPLAY_ID.into()),
            ..common::options("ET5-TEST")
        };
        let fixture = Fixture::new(options).await;
        fixture.service.attach_tracker(Some(0xBC38_1A37));
        fixture.service.detach_tracker();
        fixture.service.attach_tracker(None);
        fixture.service.publish_presence(0xD4_41C5_AAA6, i64::from(PLATMOD_AWAY));
        let mut session = fixture.session_with(Vec::new(), 2).await;
        for (command, key) in [(5, "subscribe"), (6, "unsubscribe")] {
            let mut vectors: Vec<(u32, &serde_json::Value)> =
                real[key].as_object().unwrap().iter().map(|(id, vector)| (id.parse().unwrap(), vector)).collect();
            vectors.sort_by_key(|(id, _)| *id);
            for (property_id, vector) in vectors {
                let tag = vector["tag"].as_u64().unwrap() as u32;
                session
                    .command
                    .send_packet(&encode_property_subscribe_request(tag, property_id, command).unwrap())
                    .await;
                let (packet, expected) = (session.command.packet().await, hex(vector["hex"].as_str().unwrap()));
                if command == 5 && [0x12, 0x13].contains(&property_id) {
                    assert_eq!(
                        (packet.len(), &packet[..52]),
                        (expected.len(), &expected[..52]),
                        "{command} {property_id:#x}"
                    );
                } else {
                    assert_eq!(packet, expected, "{command} {property_id:#x}");
                }
            }
        }
    });
}

#[test]
fn licensed_presence_unsubscribe_matches_the_real_service() {
    run(async {
        let fixture = Fixture::default().await;
        let mut session = fixture.session().await;
        let (tag, property_id, expected) = REAL_UNSUBSCRIBE_LICENSED;
        session
            .command
            .send_packet(&encode_property_subscribe_request(tag, property_id, COMMAND_PROPERTY_UNSUBSCRIBE).unwrap())
            .await;
        assert_eq!(session.command.packet().await, hex(expected));
    });
}

#[test]
fn presence_changes_reach_only_subscribed_clients() {
    run(async {
        let fixture = Fixture::default().await;
        let mut watching = fixture.session().await;
        let mut other = fixture.session().await;
        let tag = watching.next_tag();
        watching
            .command
            .send_packet(&encode_property_subscribe_request(tag, 0xB, COMMAND_PROPERTY_SUBSCRIBE).unwrap())
            .await;
        let response = decode_property_subscribe_response(&watching.command.packet().await).unwrap();
        assert_eq!(u32::from_le_bytes(response.block[12..16].try_into().unwrap()), 2);
        fixture.service.publish_presence(FIXED_CLOCK_US + 10, i64::from(PLATMOD_PRESENT));
        let notification = watching.subscription.packet().await;
        assert_eq!(notification, fixture.service.presence().notification().unwrap());
        let payload = decode_envelope(&notification, DEFAULT_MAX_PAYLOAD).unwrap();
        assert_eq!(payload[8..12], 2u32.to_le_bytes());
        assert_eq!(other.subscription.packet_within(Duration::from_millis(200)).await, None);
        let tag = watching.next_tag();
        watching
            .command
            .send_packet(&encode_property_subscribe_request(tag, 0xB, COMMAND_PROPERTY_UNSUBSCRIBE).unwrap())
            .await;
        watching.command.packet().await;
        fixture.service.publish_presence(FIXED_CLOCK_US + 20, i64::from(PLATMOD_AWAY));
        assert_eq!(watching.subscription.packet_within(Duration::from_millis(200)).await, None);
        assert_eq!(fixture.service.counters().sent_notifications.get(), 1);
    });
}

fn state_directory(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("tobii-linux-enabled-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    directory
}

#[test]
fn enabled_state_round_trips() {
    let directory = state_directory("round-trip");
    let path = directory.join("nested/eye-tracking-enabled");
    let state = EnabledState::new(path.clone());
    assert!(state.load());
    state.store(false).unwrap();
    assert!(!state.load());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "0\n");
    state.store(true).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "1\n");
    assert!(state.load());
    std::fs::write(&path, "garbage").unwrap();
    assert!(state.load());
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, ["eye-tracking-enabled"]);
    std::fs::remove_dir_all(&directory).unwrap();

    let state_file = std::path::PathBuf::from("tobii-linux/eye-tracking-enabled");
    assert_eq!(
        EnabledState::default_path(Some("/state"), Some("/home/u")),
        std::path::Path::new("/state").join(&state_file)
    );
    let home = std::path::Path::new("/home/u/.local/state").join(&state_file);
    assert_eq!(EnabledState::default_path(None, Some("/home/u")), home);
    assert_eq!(EnabledState::default_path(Some(""), Some("/home/u")), home);
}

async fn paused_answer(session: &mut Session) -> Vec<u8> {
    let tag = session.next_tag();
    session.command.send_packet(&encode_property_request(tag, PROPERTY_DEVICE_PAUSED)).await;
    decode_property_response(&session.command.packet().await).unwrap().block
}

#[test]
fn disabling_without_a_tracker_session_is_stored_and_read_back() {
    run(async {
        let directory = state_directory("detached");
        let path = directory.join("eye-tracking-enabled");
        let options = Options { enabled_state: Some(path.clone()), ..common::options("ET5-TEST") };
        let fixture = Fixture::new(options.clone()).await;
        assert!(fixture.service.enabled());
        let mut session = fixture.session().await;
        assert_eq!(paused_answer(&mut session).await, word_value(0).0);
        let tag = session.next_tag();
        session
            .command
            .send_packet(
                &encode_property_subscribe_request(tag, PROPERTY_DEVICE_PAUSED, COMMAND_PROPERTY_SUBSCRIBE).unwrap(),
            )
            .await;
        session.command.packet().await;

        fixture.service.set_enabled(false).await.unwrap();
        assert!(!fixture.service.enabled());
        assert_eq!(session.subscription.packet().await, paused_notification(true));
        assert_eq!(paused_answer(&mut session).await, word_value(1).0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "0\n");
        fixture.service.set_enabled(false).await.unwrap();
        assert_eq!(session.subscription.packet_within(Duration::from_millis(200)).await, None);

        let restarted = Fixture::new(options).await;
        assert!(!restarted.service.enabled());
        assert_eq!(paused_answer(&mut restarted.session().await).await, word_value(1).0);
        std::fs::remove_dir_all(&directory).unwrap();
    });
}

async fn two_clients_receive_every_sample_with(fixture: Fixture) {
    let mut clients = vec![fixture.session().await, fixture.session().await];
    for client in &mut clients {
        assert_eq!(client.stream_request(COMMAND_STREAM_START, HEAD_POSE).await.1.status, 0);
    }
    let samples: Vec<HeadPoseSample> =
        (0..3).map(|i| sample(FIXED_CLOCK_US + i, [i as f32, 0.0, 600.0], [0.0; 3])).collect();
    for pose in &samples {
        fixture.service.hub().publish(pose);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for client in &mut clients {
        for pose in &samples {
            assert_eq!(client.subscription.packet().await, stream_package(pose));
        }
    }
    clients[0].stream_request(COMMAND_STREAM_STOP, HEAD_POSE).await;
    fixture.service.hub().publish(&samples[0]);
    assert_eq!(clients[1].subscription.packet().await, stream_package(&samples[0]));
    assert_eq!(clients[0].subscription.packet_within(Duration::from_millis(200)).await, None);
}

#[test]
fn two_clients_receive_every_sample() {
    run(async { two_clients_receive_every_sample_with(Fixture::default().await).await });
}

#[test]
fn gaze_two_clients_receive_every_sample() {
    run(async { two_clients_receive_every_sample_with(gaze_fixture().await).await });
}

#[test]
fn closing_the_command_channel_closes_its_subscription() {
    run(async {
        let fixture = Fixture::default().await;
        let Session { command, mut subscription, .. } = fixture.session().await;
        drop(command);
        assert!(subscription.closed().await);
        assert!(eventually(|| fixture.service.subscriptions() == 0).await);
    });
}

async fn tracker_loss_closes_clients_until_it_is_back_with(fixture: Fixture) {
    let mut session = fixture.session().await;
    session.stream_request(COMMAND_STREAM_START, HEAD_POSE).await;
    fixture.service.detach_tracker();
    assert!(session.command.closed().await);
    assert!(session.subscription.closed().await);
    assert_eq!(fixture.service.hub().subscribers(), 0);
    let mut refused = fixture.channel("ET5-TEST").await;
    refused
        .send(
            &encode_client_hello(&ClientHello {
                channel: COMMAND_CHANNEL,
                reply_pipe: "client_cmd".into(),
                version: 1,
            })
            .unwrap(),
        )
        .await;
    assert!(refused.closed().await);
    fixture.attach();
    let session = fixture.session().await;
    assert_eq!(decode_command_response(&session.connect, true).unwrap().status, 0);
    let _ = session.info;
}

#[test]
fn tracker_loss_closes_clients_until_it_is_back() {
    run(async { tracker_loss_closes_clients_until_it_is_back_with(Fixture::default().await).await });
}

#[test]
fn gaze_tracker_loss_closes_clients_until_it_is_back() {
    run(async { tracker_loss_closes_clients_until_it_is_back_with(gaze_fixture().await).await });
}

#[test]
fn pid_mismatch_closes_connection() {
    run(async {
        let fixture = Fixture::default().await;
        let mut command = fixture.channel("ET5-TEST").await;
        command
            .send(
                &encode_client_hello(&ClientHello {
                    channel: COMMAND_CHANNEL,
                    reply_pipe: "client_cmd".into(),
                    version: 1,
                })
                .unwrap(),
            )
            .await;
        check_handshake_ack(&command.receive().await).unwrap();
        command.send(&encode_client_pid(CLIENT_PID + 1)).await;
        assert_eq!(command.receive_within(TIMEOUT).await, Received::Closed);
    });
}

#[test]
fn version_zero_hello_closes_connection() {
    run(async {
        let fixture = Fixture::default().await;
        let mut command = fixture.channel("ET5-TEST").await;
        let mut hello = [0u32.to_le_bytes(), COMMAND_CHANNEL.to_le_bytes()].concat();
        hello.extend(b"client_cmd\0");
        command.send(&hello).await;
        assert_eq!(command.receive_within(TIMEOUT).await, Received::Closed);
    });
}

async fn gaze_fixture() -> Fixture {
    Fixture::new(Options { gaze: true, ..common::options("ET5-TEST") }).await
}

#[test]
fn connect_lists_the_real_service_gaze_streams() {
    run(async {
        let fixture = gaze_fixture().await;
        let session = fixture.session().await;
        let ResponseBody::Connect(connect) = decode_command_response(&session.connect, true).unwrap().body else {
            panic!("connect body")
        };
        assert_eq!(connect.streams, GAZE_SERVICE_STREAMS.map(|s| s as i32).to_vec());
    });
}

#[test]
fn gaze_stream_responses_match_the_real_service() {
    run(async {
        let fixture = gaze_fixture().await;
        let mut session = fixture.session().await;
        for (tag, command, stream, expected) in REAL_GAZE_STREAM_RESPONSES {
            session.command.send_packet(&encode_stream_request(tag, command, stream).unwrap()).await;
            let expected = format!(
                "5450525030000000645052500c000000{tag:02x}0000000a000000040000000c000000{expected}0400000000000000"
            );
            assert_eq!(session.command.packet().await, hex(&expected), "{tag}");
        }
    });
}

#[test]
fn gaze_streams_push_each_frame_until_stopped() {
    run(async {
        let fixture = gaze_fixture().await;
        let mut session = fixture.session().await;
        let streams = [PRP_GAZE_POINT, PRP_RESPONSIVE_GAZE_POINT, PRP_GAZE_ORIGIN];
        for stream in streams {
            assert_eq!(session.stream_request(COMMAND_STREAM_START, stream).await.1.status, 0);
        }
        let frame = synthetic_gaze_frame(FIXED_CLOCK_US + 9, FIXED_CLOCK_US);
        fixture.service.gaze_hub().unwrap().publish(&frame);
        let mut packages = BTreeMap::new();
        for _ in streams {
            let package = session.subscription.packet().await;
            let payload = decode_envelope(&package, DEFAULT_MAX_PAYLOAD).unwrap();
            packages.insert(u32::from_le_bytes(payload[20..24].try_into().unwrap()), package);
        }
        let expected: BTreeMap<u32, Vec<u8>> =
            streams.iter().map(|&stream| (stream, gaze_package(&frame, stream).unwrap())).collect();
        assert_eq!(packages, expected);
        let point =
            decode_gaze_body(&decode_envelope(&packages[&PRP_GAZE_POINT], DEFAULT_MAX_PAYLOAD).unwrap()[28..52])
                .unwrap();
        assert_eq!(
            (point.timestamp, point.valid, [point.x, point.y]),
            (FIXED_CLOCK_US + 9, true, frame.device.combined_point.map(f64::from))
        );
        for stream in streams {
            assert_eq!(session.stream_request(COMMAND_STREAM_STOP, stream).await.1.status, 0);
        }
        assert_eq!(fixture.service.gaze_hub().unwrap().subscribers(), 0);
        fixture.service.gaze_hub().unwrap().publish(&frame);
        assert_eq!(session.subscription.packet_within(Duration::from_millis(200)).await, None);
    });
}

#[test]
fn head_pose_and_gaze_streams_are_independent() {
    run(async {
        let fixture = gaze_fixture().await;
        let mut session = fixture.session().await;
        for stream in [HEAD_POSE, PRP_GAZE_POINT] {
            session.stream_request(COMMAND_STREAM_START, stream).await;
        }
        session.stream_request(COMMAND_STREAM_STOP, HEAD_POSE).await;
        fixture.service.hub().publish(&sample(FIXED_CLOCK_US, [0.0, 0.0, 600.0], [0.0; 3]));
        let frame = synthetic_gaze_frame(FIXED_CLOCK_US, FIXED_CLOCK_US);
        fixture.service.gaze_hub().unwrap().publish(&frame);
        assert_eq!(session.subscription.packet().await, gaze_package(&frame, PRP_GAZE_POINT).unwrap());
        assert_eq!(session.subscription.packet_within(Duration::from_millis(200)).await, None);
    });
}

#[test]
fn gaze_disconnect_releases_streams() {
    run(async {
        let fixture = gaze_fixture().await;
        let mut session = fixture.session().await;
        session.stream_request(COMMAND_STREAM_START, PRP_GAZE_POINT).await;
        assert_eq!(fixture.service.gaze_hub().unwrap().subscribers(), 1);
        drop(session);
        assert!(eventually(|| fixture.service.gaze_hub().unwrap().subscribers() == 0).await);
    });
}

#[test]
fn gaze_stream_requests_for_other_streams_and_before_start() {
    run(async {
        let fixture = gaze_fixture().await;
        let mut session = fixture.session().await;
        assert_eq!(session.stream_request(COMMAND_STREAM_START, 7).await.1.status, 2);
        assert_eq!(session.stream_request(COMMAND_STREAM_STOP, PRP_GAZE_POINT).await.1.status, 5);
    });
}

#[test]
fn clock_follows_the_wine_counter_source() {
    for (name, source) in [("boottime", LinuxClock::Boottime), ("raw", LinuxClock::Raw)] {
        let clock = wine_clock(name, Clocks::Host).unwrap();
        let Ok(before) = LinuxClocks.read_us(source);
        let value = now_us(&clock);
        let Ok(after) = LinuxClocks.read_us(source);
        assert!(before <= value && value <= after, "{name}: {before} {value} {after}");
    }
}

#[test]
fn unknown_clock_is_rejected() {
    assert!(wine_clock("realtime", Clocks::Host).is_none());
}
