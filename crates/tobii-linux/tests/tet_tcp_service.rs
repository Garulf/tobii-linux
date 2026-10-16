//! Production tet-tcp endpoint: ReplyCaptureTests and ServiceTests of
//! tests/test_tet_tcp_service.py. The device replies are real ET5 replies
//! (evidence/private/gaze-capture/device-query.jsonl, live). The restamp cases are
//! tobii_service's own tests.

mod common;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use tobii_device::channel::{Channel, ChannelError};
use tobii_device::session::TrackerSession;
use tobii_linux::service::clock::{Clocks, wine_clock};
use tobii_linux::service::tet_tcp::TetTcpService;
use tobii_linux::service::{Options, PrpService};
use tobii_linux::tracker::DeviceReplies;
use tobii_protocol::Message;
use tobii_protocol::commands::{
    RESPONSE_KIND, STREAM_DATA_KIND, Stream, command, header_only_request, request, subscribe_stream,
    unsubscribe_stream,
};
use tobii_protocol::fields::{encode_payload, int32_field};
use tobii_protocol::gaze::build::{columns_payload, int64_column, uint32_column, vec2_column};
use tobii_protocol::gaze::decode_gaze_message;
use tobii_protocol::ttp::{Decoder, REQUEST_KIND, protocol_upgrade};
use tobii_service::qpc_clock::encode_qpc_report;
use tobii_service::tet_tcp::{DISPLAY_AREA_WRITABLE, GAZE_STREAM, PRESENCE_STREAM, UNSUPPORTED_OPERATION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const DEVICE_INFO_PAYLOAD: &str = concat!(
    "000014000000160000001249533546462d313030323034313431343633140000001a000000164953355f4c61",
    "7267655f457965747261636b65725f35140000000700000003495335140000000e0000000a30326131613661",
    "393737",
);
const DISPLAY_AREA_PAYLOAD: &str = concat!(
    "0000050000000400031f410400000008fffb5d85080000000400000008000518042800000004000000080001bf4666000000",
    "050000000400031f4104000000080004b185080000000400000008000518042800000004000000080001bf46660000000500",
    "00000400031f410400000008fffb5d8508000000040000000800002911bf0000000400000008fffff3999450000005000000",
    "0400010100020000000400003039",
);
const UPGRADE_PAYLOAD: &str = "0000020000000400010008";
/// Service clock reading of every Linux clock, and the game counter one second behind it.
///
/// The Python test injects `us - 1000` as the game time; here the offset comes from a bridge
/// report and must exceed the counter mapping's 10 ms tolerance, below which the counter is
/// taken to be the service clock itself.
const HOST_US: i64 = 6_000_000;
const GAME_OFFSET_US: i64 = 1_000_000;

fn reply(command: u32, payload: &str, status: u32) -> Message {
    Message::new(RESPONSE_KIND, 0, 1, command, status, hex(payload))
}

fn gaze_message(timestamp: i64) -> Message {
    let columns = [(1, int64_column(timestamp)), (27, uint32_column(1)), (28, vec2_column([0.25, 0.75]).unwrap())];
    Message::new(STREAM_DATA_KIND, 0, 0, GAZE_STREAM, 0, columns_payload(&columns).unwrap())
}

/// Answers getters like a tracker; 1460 is never answered.
#[derive(Debug, Default)]
struct FakeSession {
    requests: Vec<Message>,
    decoder: Decoder,
    outgoing: VecDeque<u8>,
}

impl FakeSession {
    fn answer(request: &Message) -> Option<Message> {
        let answer =
            |word4, payload| Message::new(RESPONSE_KIND, request.request_id, 1, request.command, word4, payload);
        match request.command {
            DISPLAY_AREA_WRITABLE => None,
            command::PROTOCOL_UPGRADE => Some(answer(0, hex(UPGRADE_PAYLOAD))),
            command::UNLOCK_PPU => {
                Some(answer(0, encode_payload(&[int32_field(0), int32_field(0), int32_field(0)]).unwrap()))
            }
            command::GET_DEVICE_INFO => Some(answer(0, hex(DEVICE_INFO_PAYLOAD))),
            _ => Some(answer(UNSUPPORTED_OPERATION, Vec::new())),
        }
    }
}

impl Channel for FakeSession {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        for message in self.decoder.feed(data).map_err(|error| ChannelError::new(false, error))? {
            if let Some(reply) = Self::answer(&message) {
                self.outgoing.extend(reply.encode().unwrap());
            }
            self.requests.push(message);
        }
        Ok(())
    }

    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        if self.outgoing.is_empty() {
            std::thread::sleep(Duration::from_millis(u64::from(timeout_ms.min(5))));
        }
        Ok(self.outgoing.drain(..).collect())
    }
}

#[test]
fn capture_keeps_startup_replies_and_every_answer() {
    let mut session = TrackerSession::new(FakeSession::default());
    session.initialize().unwrap();
    session.channel_mut().requests.clear();
    let device = DeviceReplies::capture_getters(&mut session).unwrap();
    assert!(session.channel().requests.iter().all(|m| m.kind == REQUEST_KIND && m.payload.is_empty()));
    assert!(device.replies.contains_key(&command::GET_DEVICE_INFO));
    assert_eq!(device.replies[&command::GET_DEVICE_PROPERTIES].word4, UNSUPPORTED_OPERATION);
    assert!(!device.replies.contains_key(&DISPLAY_AREA_WRITABLE));
    assert_eq!(device.replies[&command::PROTOCOL_UPGRADE].payload, hex(UPGRADE_PAYLOAD));
    assert_eq!(device.display_area(), None);
}

#[test]
fn display_area_comes_from_the_1430_reply() {
    let replies = HashMap::from([(command::GET_DISPLAY_AREA, reply(1430, DISPLAY_AREA_PAYLOAD, 0))]);
    let [top_left, _, bottom_left] = DeviceReplies { replies, streams: BTreeSet::new() }.display_area().unwrap();
    assert!((top_left[0] + 296.62008666992).abs() < 5e-5, "{top_left:?}");
    assert!((bottom_left[2] + 3.1000201702).abs() < 5e-5, "{bottom_left:?}");
}

struct Fixture {
    service: TetTcpService,
    stream: TcpStream,
    decoder: Decoder,
    pending: VecDeque<Message>,
    _prp: PrpService,
}

impl Fixture {
    async fn new() -> Self {
        let clock = wine_clock("raw", Clocks::Scripted(Arc::new(|_| HOST_US))).unwrap();
        let (prp, port, _) = service(Options::default(), clock).await;
        let counter = (HOST_US - GAME_OFFSET_US) as u64 * 10;
        Bridge::attach(port).await.report(&prp, &encode_qpc_report(counter, 10_000_000, &[])).await;
        let service = TetTcpService::new(prp.game_time().clone(), "127.0.0.1".into(), 0);
        let replies = HashMap::from([
            (command::PROTOCOL_UPGRADE, reply(1000, UPGRADE_PAYLOAD, 0)),
            (command::GET_DEVICE_INFO, reply(1420, DEVICE_INFO_PAYLOAD, 0)),
            (command::GET_DEVICE_PROPERTIES, reply(1330, "", UNSUPPORTED_OPERATION)),
        ]);
        service.attach(DeviceReplies { replies, streams: BTreeSet::from([GAZE_STREAM, PRESENCE_STREAM, 0x501]) });
        assert!(eventually(|| service.port_bound().is_some()).await);
        let stream = TcpStream::connect(("127.0.0.1", service.port_bound().unwrap())).await.unwrap();
        assert!(eventually(|| service.connections() == 1).await);
        Self { service, stream, decoder: Decoder::default(), pending: VecDeque::new(), _prp: prp }
    }

    async fn ask(&mut self, message: Message) -> Message {
        self.stream.write_all(&message.encode().unwrap()).await.unwrap();
        self.receive_within(TIMEOUT).await.expect("an answer")
    }

    async fn receive_within(&mut self, timeout: Duration) -> Option<Message> {
        let mut buffer = vec![0; 65536];
        while self.pending.is_empty() {
            let count = tokio::time::timeout(timeout, self.stream.read(&mut buffer)).await.ok()?.unwrap();
            assert_ne!(count, 0, "connection closed");
            self.pending.extend(self.decoder.feed(&buffer[..count]).unwrap());
        }
        self.pending.pop_front()
    }
}

#[test]
fn startup_and_getters_come_from_the_device_replies() {
    run(async {
        let mut fixture = Fixture::new().await;
        let upgrade = fixture.ask(protocol_upgrade(7)).await;
        assert_eq!((upgrade.kind, upgrade.request_id, upgrade.payload), (RESPONSE_KIND, 7, hex(UPGRADE_PAYLOAD)));
        let info = fixture.ask(header_only_request(8, command::GET_DEVICE_INFO).unwrap()).await;
        assert_eq!((info.request_id, info.word4, info.payload), (8, 0, hex(DEVICE_INFO_PAYLOAD)));
        let properties = fixture.ask(header_only_request(9, command::GET_DEVICE_PROPERTIES).unwrap()).await;
        assert_eq!((properties.request_id, properties.word4), (9, UNSUPPORTED_OPERATION));
    });
}

#[test]
fn writes_and_unknown_commands_are_refused() {
    run(async {
        let mut fixture = Fixture::new().await;
        for command in [1440, command::SET_FREQUENCIES, 3000, command::GET_TRACK_BOX] {
            let answer = fixture.ask(request(11, command, &[]).unwrap()).await;
            assert_eq!((answer.command, answer.word4), (command, UNSUPPORTED_OPERATION));
        }
    });
}

#[test]
fn subscribed_connections_get_restamped_streams() {
    run(async {
        let mut fixture = Fixture::new().await;
        let answer = fixture.ask(subscribe_stream(12, Stream::Gaze.index(), &[]).unwrap()).await;
        assert_eq!((answer.request_id, answer.word2, answer.word4), (12, 1, 0));
        let image = fixture.ask(subscribe_stream(13, Stream::Image.index(), &[]).unwrap()).await;
        assert_eq!(image.word4, UNSUPPORTED_OPERATION);
        fixture.service.publish(&gaze_message(908279389020), 5_000_000);
        let streamed = fixture.receive_within(TIMEOUT).await.unwrap();
        assert_eq!((streamed.kind, streamed.command), (STREAM_DATA_KIND, GAZE_STREAM));
        let timestamp = decode_gaze_message(&streamed.payload).unwrap().unwrap().timestamp;
        assert_eq!(timestamp, 5_000_000 - GAME_OFFSET_US);
        fixture.ask(unsubscribe_stream(14, Stream::Gaze.index()).unwrap()).await;
        fixture.service.publish(&gaze_message(908279389021), 5_030_000);
        assert_eq!(fixture.receive_within(Duration::from_millis(200)).await, None);
    });
}

#[test]
fn detach_closes_the_port_and_connections() {
    run(async {
        let mut fixture = Fixture::new().await;
        let port = fixture.service.port_bound().unwrap();
        fixture.service.detach();
        let mut buffer = [0; 10];
        let read = tokio::time::timeout(Duration::from_secs(1), fixture.stream.read(&mut buffer)).await.unwrap();
        assert_eq!(read.unwrap(), 0);
        assert!(eventually(|| std::net::TcpStream::connect(("127.0.0.1", port)).is_err()).await);
    });
}
