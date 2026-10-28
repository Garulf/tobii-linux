//! SESP service over the relay: SespServiceTests of tests/test_sesp.py, driven by a simulated
//! bridge. ORIGINAL_* requests are the game DLL's builders run in emulation by
//! tools/sesp_oracle.py. The codec, legacy-position and counter-mapping cases are
//! tobii_service's own tests.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tobii_headpose::pipeline::HeadPoseSample;
use tobii_linux::service::clock::{Clocks, wine_clock};
use tobii_linux::service::{Options, PrpService};
use tobii_service::prp_service::{
    DEFAULT_NAME, FRAME_BRIDGE_HELLO, FRAME_MESSAGE, FRAME_PIPE_OPEN, FRAME_REPLY_PIPE, decode_names, encode_frame,
    encode_pipe_open,
};
use tobii_service::sesp::{
    CAPABILITY_HEAD_POSE, CAPABILITY_STREAM_STATUS, COMMAND_CHANNEL, ERROR_NOT_FOUND, ERROR_NOT_SUPPORTED, Field,
    FrameDecoder, INVALID, PIPE_NAME, REQUEST_GET_DISPLAY_INFO, REQUEST_HEADPOSE_STOP, REQUEST_INITIALIZE,
    REQUEST_SEND_STATISTICS, RESPONSE_HEADPOSE_START, RESPONSE_HEADPOSE_STOP, RESPONSE_NOTIFY_STREAM_STATUS,
    RESPONSE_SEND_STATISTICS, SIGNAL_INVALID_OPERATION, SUBSCRIPTION_CHANNEL, Value, capabilities, decode,
    encode_hello, head_pose, message,
};
use tobii_service::sesp_service::LegacyPositionTransform;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

const ORIGINAL_HEADPOSE_START_REQUEST: &str = concat!(
    "73657370260000005565737004000000e8ffffff040000000800000004000000fcffffff040004000a000d00",
    "04000c000800",
);
const ORIGINAL_STREAM_STATUS_REQUEST: &str = concat!(
    "736573703e0000004d65737004000000d0ffffff05000000080000001a000000e8ffffff0c00000004000000",
    "00000000010000000d00000008000c00040008000a000d0004000c000800",
);
const ALBEDO_AREA: [[f64; 3]; 3] = [
    [-296.6200866699219, 326.0040588378906, 111.8187484741211],
    [300.3799133300781, 326.0040588378906, 111.8187484741211],
    [-296.6200866699219, 10.267330169677734, -3.100020170211792],
];
const SESP_CLIENT_PID: u32 = 0x114;
const PIPE_CHUNK: usize = 4096;

/// One streamengineservices connection as the bridge relays it.
struct RelayedPipe {
    stream: TcpStream,
    decoder: FrameDecoder,
    reply_pipe: Option<String>,
    pending: Vec<Vec<u8>>,
}

impl RelayedPipe {
    async fn open(port: u16) -> Self {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let open = encode_frame(FRAME_PIPE_OPEN, &encode_pipe_open(SESP_CLIENT_PID, PIPE_NAME).unwrap()).unwrap();
        stream.write_all(&open).await.unwrap();
        Self { stream, decoder: FrameDecoder::default(), reply_pipe: None, pending: Vec::new() }
    }

    async fn raw_within(&mut self, timeout: Duration) -> Received {
        loop {
            match tokio::time::timeout(timeout, read_frame(&mut self.stream)).await {
                Err(_) => return Received::Timeout,
                Ok(None) => return Received::Closed,
                Ok(Some((FRAME_REPLY_PIPE, payload))) => self.reply_pipe = decode_names(&payload).unwrap().pop(),
                Ok(Some((FRAME_MESSAGE, payload))) => return Received::Message(payload),
                Ok(Some((kind, _))) => panic!("unexpected frame {kind}"),
            }
        }
    }

    async fn hello(&mut self, channel: u32, reply_pipe: &str) -> u32 {
        self.send(&encode_hello(channel, reply_pipe).unwrap()).await;
        let Received::Message(slot) = self.raw_within(TIMEOUT).await else { panic!("no slot") };
        assert_eq!((self.reply_pipe.as_deref(), slot.len()), (Some(reply_pipe), 4));
        u32::from_le_bytes(slot.try_into().unwrap())
    }

    async fn send(&mut self, data: &[u8]) {
        for chunk in data.chunks(PIPE_CHUNK) {
            self.stream.write_all(&encode_frame(FRAME_MESSAGE, chunk).unwrap()).await.unwrap();
        }
    }

    /// The next SESP message buffer; `None` when nothing arrives within `timeout`.
    async fn message_within(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        while self.pending.is_empty() {
            let Received::Message(data) = self.raw_within(timeout).await else { return None };
            self.pending.extend(self.decoder.feed(&data).unwrap());
        }
        Some(self.pending.remove(0))
    }

    async fn message(&mut self) -> Vec<u8> {
        self.message_within(TIMEOUT).await.expect("a SESP message")
    }
}

fn sequence_and_kind(buffer: &[u8]) -> (u32, u8) {
    let message = decode(buffer).unwrap();
    (message.sequence, message.kind)
}

fn game_request(sequence: u32, kind: u8) -> Vec<u8> {
    message(sequence, kind, &[], &[])
}

fn initialize_request(subscription_slot: u32) -> Vec<u8> {
    let slot = Field::Scalar { index: 1, value: Value::U32(subscription_slot) };
    message(1, REQUEST_INITIALIZE, &[slot], &[(0, b"tet-tcp://127.0.0.1".to_vec())])
}

fn albedo() -> LegacyPositionTransform {
    let [top_left, top_right, bottom_left] = ALBEDO_AREA;
    LegacyPositionTransform::from_display_area(top_left, top_right, bottom_left)
}

fn sample(timestamp_us: i64, valid: bool, position_mm: [f32; 3], rotation_rad: [f32; 3]) -> HeadPoseSample {
    HeadPoseSample { timestamp_us, valid, position_mm, rotation_rad }
}

struct Fixture {
    service: PrpService,
    port: u16,
}

impl Fixture {
    async fn new() -> Self {
        let clock = wine_clock("raw", Clocks::Scripted(Arc::new(|_| 0))).unwrap();
        let options = Options {
            display_area: Some(ALBEDO_AREA),
            tet_tcp: true,
            tet_tcp_port: 0,
            ..common::options(DEFAULT_NAME)
        };
        let (service, port, _) = service(options, clock).await;
        service.attach_tracker(None, None);
        Self { service, port }
    }

    /// Command and subscription channels after initialize, and the initialize response.
    async fn connect(&self) -> (RelayedPipe, RelayedPipe, Vec<u8>) {
        let mut command = RelayedPipe::open(self.port).await;
        command.hello(COMMAND_CHANNEL, "client_streamengineservices_00000114_1").await;
        let mut subscription = RelayedPipe::open(self.port).await;
        let slot = subscription.hello(SUBSCRIPTION_CHANNEL, "client_streamengineservices_00000114_2").await;
        command.send(&initialize_request(slot)).await;
        let response = command.message().await;
        (command, subscription, response)
    }
}

#[test]
fn bridge_serves_only_the_sesp_pipe() {
    run(async {
        let fixture = Fixture::new().await;
        let mut bridge = TcpStream::connect(("127.0.0.1", fixture.port)).await.unwrap();
        bridge.write_all(&encode_frame(FRAME_BRIDGE_HELLO, &1u32.to_le_bytes()).unwrap()).await.unwrap();
        let (_, payload) = read_frame(&mut bridge).await.unwrap();
        assert_eq!(decode_names(&payload).unwrap(), [PIPE_NAME]);
    });
}

#[test]
fn initialize_advertises_head_pose() {
    run(async {
        let fixture = Fixture::new().await;
        let (_, _, response) = fixture.connect().await;
        let message = decode(&response).unwrap();
        assert_eq!(message.sequence, 1);
        assert_eq!(
            capabilities(message.kind, message.payload.as_ref()).unwrap(),
            (0, vec![CAPABILITY_HEAD_POSE, CAPABILITY_STREAM_STATUS])
        );
    });
}

#[test]
fn initialize_with_unknown_subscription_fails() {
    run(async {
        let fixture = Fixture::new().await;
        let mut command = RelayedPipe::open(fixture.port).await;
        command.hello(COMMAND_CHANNEL, "client_a").await;
        command.send(&initialize_request(999)).await;
        let response = command.message().await;
        let message = decode(&response).unwrap();
        assert_eq!(capabilities(message.kind, message.payload.as_ref()).unwrap(), (ERROR_NOT_FOUND, vec![]));
    });
}

#[test]
fn head_pose_flows_on_the_subscription_channel_until_stopped() {
    run(async {
        let fixture = Fixture::new().await;
        let (mut command, mut subscription, _) = fixture.connect().await;
        command.send(&hex(ORIGINAL_HEADPOSE_START_REQUEST)).await;
        assert_eq!(sequence_and_kind(&command.message().await), (4, RESPONSE_HEADPOSE_START));
        tokio::time::sleep(Duration::from_millis(50)).await;
        fixture.service.hub().publish(&sample(1_000_000, true, [0.0, 0.0, 600.0], [0.1, -0.2, 0.0]));
        fixture.service.hub().publish(&sample(1_030_000, false, [0.0; 3], [0.0; 3]));
        let (first, second) = (subscription.message().await, subscription.message().await);
        assert_eq!([decode(&first).unwrap().sequence, decode(&second).unwrap().sequence], [1, 2]);
        let first = decode(&first).unwrap();
        let pose = head_pose(first.kind, first.payload.as_ref()).unwrap();
        assert_eq!(pose.timestamp_us, 1_000_000);
        let restored = albedo().apply(pose.position);
        assert!(restored.iter().zip([0.0, 0.0, 600.0]).all(|(got, want)| (got - want).abs() < 1e-3), "{restored:?}");
        let second = decode(&second).unwrap();
        assert_eq!(head_pose(second.kind, second.payload.as_ref()).unwrap().position_validity, INVALID);
        command.send(&game_request(5, REQUEST_HEADPOSE_STOP)).await;
        assert_eq!(sequence_and_kind(&command.message().await), (5, RESPONSE_HEADPOSE_STOP));
        tokio::time::sleep(Duration::from_millis(50)).await;
        fixture.service.hub().publish(&sample(1_060_000, true, [0.0, 0.0, 600.0], [0.0; 3]));
        assert_eq!(subscription.message_within(Duration::from_millis(200)).await, None);
    });
}

#[test]
fn other_requests() {
    run(async {
        let fixture = Fixture::new().await;
        let (mut command, _subscription, _) = fixture.connect().await;
        command.send(&hex(ORIGINAL_STREAM_STATUS_REQUEST)).await;
        assert_eq!(sequence_and_kind(&command.message().await).1, RESPONSE_NOTIFY_STREAM_STATUS);
        command.send(&game_request(6, REQUEST_SEND_STATISTICS)).await;
        assert_eq!(sequence_and_kind(&command.message().await), (6, RESPONSE_SEND_STATISTICS));
        command.send(&game_request(7, REQUEST_GET_DISPLAY_INFO)).await;
        let refused = command.message().await;
        let refused = decode(&refused).unwrap();
        assert_eq!((refused.sequence, refused.kind), (7, SIGNAL_INVALID_OPERATION));
        assert_eq!(refused.payload.unwrap().u32_or(0, 0).unwrap(), ERROR_NOT_SUPPORTED);
    });
}

#[test]
fn detaching_the_tracker_closes_sesp_connections() {
    run(async {
        let fixture = Fixture::new().await;
        let (mut command, _subscription, _) = fixture.connect().await;
        fixture.service.detach_tracker();
        assert_eq!(command.raw_within(Duration::from_secs(1)).await, Received::Closed);
    });
}
