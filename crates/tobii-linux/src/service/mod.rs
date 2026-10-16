//! Linux PRP service for the game DLL's head-pose session, reached through a pipe relay.
//!
//! A Windows bridge inside the Wine prefix (tools/prp_bridge) owns the named pipes and
//! forwards each pipe connection over its own loopback TCP connection. This module speaks
//! the service side of the PRP handshake and the subset of the session the game DLL uses:
//! connect, stream start/stop for head pose (stream 2) and, with gaze, gaze point (3),
//! responsive gaze point (29) and gaze origin (4), timesync, stream packages, and the
//! property requests of the game integration layer (display area, display id, user
//! presence, with presence change notifications). Like the Windows runtime, it serves
//! clients only while a tracker session runs: when the tracker goes away every client
//! connection is closed and new ones are refused until it is back. Relay framing and
//! real-service comparison: docs/prp-bridge.md, docs/gaze-host.md and
//! docs/service-behaviour.md. The messages, property answers and presence cache are
//! tobii_service::prp_service; this module runs them on a single-threaded tokio loop.

pub mod clock;
pub mod hub;
pub mod link;
pub mod sesp;
pub mod synthetic;
pub mod tet_tcp;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use tobii_headpose::pipeline::HeadPoseSample;
use tobii_protocol::Message;
use tobii_protocol::gaze::{GazeFrame, PROPERTY_USER_PRESENCE};
use tobii_service::prp::{DEFAULT_MAX_PAYLOAD, HEAD_POSE, HeadPose, StreamSample, encode_stream_packet};
use tobii_service::prp_service::{
    self as decisions, FRAME_BRIDGE_HELLO, FRAME_DISPLAY_ID, FRAME_PIPE_OPEN, FRAME_PIPES, FRAME_QPC, FRAME_REPLY_PIPE,
    GAZE_SERVICE_STREAMS, GAZE_STREAMS, INTERNAL_URL_SCHEME, LICENSED_STREAMS, LICENSED_SUBSCRIPTIONS, PropertySource,
    STATUS_CONNECTION_FAILED, STATUS_INSUFFICIENT_LICENSE, STATUS_INVALID_PARAMETER, STATUS_NOT_SUBSCRIBED,
    UserPresence, encode_names,
};
use tobii_service::prp_session::{
    COMMAND_CHANNEL, COMMAND_CONNECT, COMMAND_PROPERTY_SUBSCRIBE, COMMAND_PROPERTY_UNSUBSCRIBE, COMMAND_STREAM_START,
    COMMAND_TIMESYNC, CommandRequest, CommandResponse, ConnectResponse, HandshakeInfo, OPERATION_PROPERTY_GET,
    PropertyResponse, ResponseBody, SERVER_PIPE_PREFIX, STATUS_NOT_SUPPORTED, STATUS_OK, SUBSCRIPTION_CHANNEL,
    TIMESYNC_CHANNEL, decode_client_hello, decode_client_pid, decode_command_request, decode_property_request,
    decode_property_subscribe_request, encode_command_response, encode_handshake_ack, encode_handshake_info,
    encode_property_response, encode_property_subscribe_response, split_packets,
};
use tobii_service::prp_streams::{
    PRP_GAZE_ORIGIN, PRP_GAZE_POINT, PRP_RESPONSIVE_GAZE_POINT, PlatmodRecord, prp_record, stream_packet,
};
use tobii_service::qpc_clock::{Basis, ClockSwitch, CounterMapping, QpcClockFollower, QpcSample, decode_qpc_report};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::tracker::DeviceReplies;
use clock::{Clock, now_us, readings};
use hub::Hub;
use link::{Ended, Frames, Link, Outcome, peer_repr, refused};

/// Real service head-pose cadence on albedo: 30.207 ms between timestamps.
pub const REAL_SERVICE_PERIOD: Duration = Duration::from_micros(30207);
/// A bridge that has not reported its counter by then predates the report.
pub const QPC_REPORT_WAIT: Duration = Duration::from_secs(5);
/// Unsent bytes above which stream packages are dropped for a slow client.
pub const MAX_PENDING_STREAM_BYTES: usize = 64 * 1024;
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Top-left, top-right and bottom-left display corners in millimetres.
pub type Corners = [[f64; 3]; 3];

/// How a service is set up; see [`PrpService::new`].
#[derive(Debug, Clone)]
pub struct Options {
    pub name: String,
    pub serial: Option<String>,
    pub display_area: Option<Corners>,
    pub display_id: Option<String>,
    pub gaze: bool,
    pub tet_tcp: bool,
    pub tet_tcp_host: String,
    pub tet_tcp_port: u16,
    pub compensate_sesp_position: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            name: decisions::DEFAULT_NAME.into(),
            serial: None,
            display_area: None,
            display_id: None,
            gaze: false,
            tet_tcp: false,
            tet_tcp_host: "127.0.0.1".into(),
            tet_tcp_port: tobii_service::tet_tcp::COMMAND_PORT,
            compensate_sesp_position: true,
        }
    }
}

/// What a tracker session delivers to the service, in order, from its own threads.
#[derive(Debug, Clone)]
pub enum Event {
    Attach(Option<u32>),
    Detach,
    Sample(HeadPoseSample),
    Gaze(Box<GazeFrame>),
    Presence(i64, i64),
    Device(Box<DeviceReplies>),
    DeviceStream(Box<Message>, i64),
}

/// Service-clock microseconds as the game's QueryPerformanceCounter microseconds.
#[derive(Debug, Clone)]
pub struct GameTime {
    clock: Arc<Clock>,
    mapping: Rc<RefCell<CounterMapping>>,
}

impl GameTime {
    pub fn map(&self, service_us: i64) -> i64 {
        self.mapping.borrow().map(&self.clock.state(), service_us)
    }

    fn observe(&self, sample: QpcSample) {
        self.mapping.borrow_mut().observe(sample);
    }
}

/// Service side of one relayed pipe connection (one channel of one client).
#[derive(Debug)]
struct PipeConnection {
    link: Link,
    pipe: String,
    client_pid: u32,
    channel: Cell<Option<u32>>,
    subscription: RefCell<Option<Rc<Subscription>>>,
}

impl PipeConnection {
    fn send(&self, message: &[u8]) {
        if self.link.is_closing() {
            return;
        }
        log::debug!("{} -> {}", self.pipe, link::hex(message));
        self.link.message(message);
    }
}

/// One client: its subscription channel, the streams it started and the properties it watches.
///
/// The command channel that connected with this subscription is paired with it; when
/// either channel closes, the other is closed too, as the real service destroys a command
/// client together with its subscription client.
#[derive(Debug)]
struct Subscription {
    id: i32,
    connection: Rc<PipeConnection>,
    command: RefCell<Option<Rc<PipeConnection>>>,
    field_of_use: Cell<i32>,
    streams: RefCell<BTreeMap<u32, u64>>,
    properties: RefCell<BTreeSet<u32>>,
}

/// One bridge control connection and what has been logged about its clock.
#[derive(Debug)]
struct Bridge {
    key: u64,
    peer: String,
    link: Link,
    verdict_clock: Cell<Option<tobii_service::qpc_clock::LinuxClock>>,
    conflict_logged: Cell<bool>,
}

#[derive(Debug, Default)]
pub struct Counters {
    pub sent_packages: Cell<u64>,
    pub dropped_packages: Cell<u64>,
    pub sent_notifications: Cell<u64>,
}

#[derive(Debug)]
struct Inner {
    name: String,
    serial: String,
    display_area: Option<Corners>,
    display_id: Option<String>,
    reported_display_id: RefCell<Option<String>>,
    clock: Arc<Clock>,
    follower: RefCell<QpcClockFollower<u64>>,
    game_time: GameTime,
    hub: Rc<Hub<HeadPoseSample>>,
    gaze_hub: Option<Hub<GazeFrame>>,
    tet_tcp: Option<tet_tcp::TetTcpService>,
    sesp: Option<sesp::SespServer>,
    sesp_pipes: RefCell<Vec<Link>>,
    subscription_pipe: String,
    timesync_pipe: String,
    subscriptions: RefCell<BTreeMap<i32, Rc<Subscription>>>,
    connections: RefCell<Vec<Rc<PipeConnection>>>,
    bridges: RefCell<Vec<Rc<Bridge>>>,
    ids: Cell<i32>,
    bridge_keys: Cell<u64>,
    counters: Counters,
    presence: Cell<UserPresence>,
    tracker_attached: Cell<bool>,
    calibration_id: Cell<u32>,
}

/// Shared state of the service: pipe names, subscription ids, the hubs and the routes.
///
/// With `tet_tcp` the service serves the patch-free route instead of the PRP pipes: gaze
/// and presence on TCP 127.0.0.1:4455 and head pose over the `streamengineservices` pipe
/// (docs/tet-tcp.md). Timestamps on that route are the game's QueryPerformanceCounter
/// microseconds.
#[derive(Debug, Clone)]
pub struct PrpService(Rc<Inner>);

/// A random version 4 UUID in its canonical text form.
pub fn uuid4() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    if let Ok(mut random) = std::fs::File::open("/dev/urandom") {
        let _ = random.read_exact(&mut bytes);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = link::hex(&bytes);
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Python's `repr` of a list of strings.
pub fn list_repr(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|item| crate::args::python_repr(item)).collect();
    format!("[{}]", items.join(", "))
}

/// Operation 1 package as sent by the real service: message tag 0, one sample.
pub fn stream_package(sample: &HeadPoseSample) -> Vec<u8> {
    let pose = HeadPose {
        timestamp: sample.timestamp_us,
        position_valid: sample.valid,
        position: sample.position_mm.map(f64::from),
        rotation_valid: [sample.valid; 3],
        rotation: sample.rotation_rad.map(f64::from),
    };
    encode_stream_packet(&StreamSample::Head(pose), 0, false).expect("head pose packages always encode")
}

/// Gaze stream package as the real service sends it: message tag 0, one record.
pub fn gaze_package(frame: &GazeFrame, stream: u32) -> Option<Vec<u8>> {
    let record = match stream {
        PRP_GAZE_POINT => PlatmodRecord::GazePoint(frame.gaze_point),
        PRP_RESPONSIVE_GAZE_POINT => PlatmodRecord::ResponsiveGazePoint(frame.responsive_gaze_point),
        PRP_GAZE_ORIGIN => PlatmodRecord::GazeOrigin(frame.gaze_origin),
        _ => return None,
    };
    stream_packet(&prp_record(&record), 0, true).ok().flatten()
}

fn word_response(tag: u32, command: u32, status: i32, value: i32) -> CommandResponse {
    CommandResponse { tag, command, status, body: ResponseBody::Word(value) }
}

impl PrpService {
    pub fn new(options: Options, clock: Arc<Clock>) -> Self {
        let game_time =
            GameTime { clock: Arc::clone(&clock), mapping: Rc::new(RefCell::new(CounterMapping::default())) };
        let hub = Rc::new(Hub::default());
        let (tet_tcp, sesp) = if options.tet_tcp {
            (
                Some(tet_tcp::TetTcpService::new(
                    game_time.clone(),
                    options.tet_tcp_host.clone(),
                    options.tet_tcp_port,
                )),
                Some(sesp::SespServer::new(Rc::clone(&hub), game_time.clone(), options.compensate_sesp_position)),
            )
        } else {
            (None, None)
        };
        Self(Rc::new(Inner {
            serial: options.serial.clone().unwrap_or_else(|| options.name.clone()),
            name: options.name,
            display_area: options.display_area,
            display_id: options.display_id,
            reported_display_id: RefCell::new(None),
            follower: RefCell::new(QpcClockFollower::default()),
            clock,
            game_time,
            hub,
            gaze_hub: options.gaze.then(Hub::default),
            tet_tcp,
            sesp,
            sesp_pipes: RefCell::new(Vec::new()),
            subscription_pipe: uuid4(),
            timesync_pipe: uuid4(),
            subscriptions: RefCell::new(BTreeMap::new()),
            connections: RefCell::new(Vec::new()),
            bridges: RefCell::new(Vec::new()),
            ids: Cell::new(1),
            bridge_keys: Cell::new(0),
            counters: Counters::default(),
            presence: Cell::new(UserPresence::default()),
            tracker_attached: Cell::new(false),
            calibration_id: Cell::new(0),
        }))
    }

    pub fn clock(&self) -> &Arc<Clock> {
        &self.0.clock
    }

    pub fn hub(&self) -> &Hub<HeadPoseSample> {
        &self.0.hub
    }

    pub fn gaze_hub(&self) -> Option<&Hub<GazeFrame>> {
        self.0.gaze_hub.as_ref()
    }

    pub fn counters(&self) -> &Counters {
        &self.0.counters
    }

    pub fn presence(&self) -> UserPresence {
        self.0.presence.get()
    }

    pub fn tracker_attached(&self) -> bool {
        self.0.tracker_attached.get()
    }

    pub fn subscriptions(&self) -> usize {
        self.0.subscriptions.borrow().len()
    }

    pub fn connections(&self) -> usize {
        self.0.connections.borrow().len()
    }

    pub fn bridges(&self) -> usize {
        self.0.bridges.borrow().len()
    }

    pub fn reported_display_id(&self) -> Option<String> {
        self.0.reported_display_id.borrow().clone()
    }

    pub fn calibration_id(&self) -> u32 {
        self.0.calibration_id.get()
    }

    pub fn tet_tcp(&self) -> Option<&tet_tcp::TetTcpService> {
        self.0.tet_tcp.as_ref()
    }

    pub fn sesp(&self) -> Option<&sesp::SespServer> {
        self.0.sesp.as_ref()
    }

    pub fn game_time(&self) -> &GameTime {
        &self.0.game_time
    }

    fn next_id(&self) -> i32 {
        let id = self.0.ids.get();
        self.0.ids.set(id + 1);
        id
    }

    /// Pipes the bridge must serve: prefixed and bare command pipes, then the two internal ones.
    ///
    /// The client opens TOBIIPRP-<name> when a pipe listing shows it and <name> otherwise
    /// (0x18004e8a0); prp-internal:// URLs name their pipe directly. The tet-tcp route
    /// needs only the fixed SESP pipe.
    pub fn pipe_names(&self) -> Vec<String> {
        if self.0.sesp.is_some() {
            return vec![tobii_service::sesp::PIPE_NAME.into()];
        }
        vec![
            format!("{SERVER_PIPE_PREFIX}{}", self.0.name),
            self.0.name.clone(),
            self.0.subscription_pipe.clone(),
            self.0.timesync_pipe.clone(),
        ]
    }

    /// Applies one event of the tracker session.
    pub fn apply(&self, event: Event) {
        match event {
            Event::Attach(calibration_id) => self.attach_tracker(calibration_id),
            Event::Detach => self.detach_tracker(),
            Event::Sample(sample) => self.0.hub.publish(&sample),
            Event::Gaze(frame) => {
                if let Some(hub) = &self.0.gaze_hub {
                    hub.publish(&frame);
                }
            }
            Event::Presence(time_us, value) => self.publish_presence(time_us, value),
            Event::Device(replies) => self.attach_device(*replies),
            Event::DeviceStream(message, time_us) => self.publish_device_stream(&message, time_us),
        }
    }

    /// A tracker session is running: accept clients, as the real server starts listening.
    pub fn attach_tracker(&self, calibration_id: Option<u32>) {
        if let Some(id) = calibration_id {
            self.0.calibration_id.set(id);
        }
        if !self.0.tracker_attached.get() {
            log::info!("tracker attached; accepting game clients (calibration id {:#x})", self.0.calibration_id.get());
        }
        self.0.tracker_attached.set(true);
    }

    /// The tracker is gone: close every client connection and refuse new ones.
    ///
    /// The real service shuts its server down when the device is removed or the host goes
    /// to sleep, destroying every command, subscription and timesync client, and listens
    /// again once the device is re-added; clients reconnect.
    pub fn detach_tracker(&self) {
        if self.0.tracker_attached.get() {
            log::info!("tracker detached; closing {} client connection(s)", self.0.connections.borrow().len());
        }
        self.0.tracker_attached.set(false);
        let connections = self.0.connections.borrow().clone();
        for connection in connections {
            connection.link.close();
        }
        let pipes = self.0.sesp_pipes.borrow().clone();
        for pipe in pipes {
            pipe.close();
        }
        let subscriptions: Vec<Rc<Subscription>> = self.0.subscriptions.borrow().values().cloned().collect();
        for subscription in subscriptions {
            self.stop_all(&subscription);
        }
        let mut presence = self.0.presence.get();
        presence.clear();
        self.0.presence.set(presence);
        if let Some(tet_tcp) = &self.0.tet_tcp {
            tet_tcp.detach();
        }
    }

    /// One platmod presence record on the service clock; pushes operation 2 when due.
    pub fn publish_presence(&self, timestamp_us: i64, platmod_value: i64) {
        let mut presence = self.0.presence.get();
        let notify = presence.record(timestamp_us, platmod_value);
        self.0.presence.set(presence);
        if !notify {
            return;
        }
        let Ok(notification) = presence.notification() else { return };
        let subscriptions: Vec<Rc<Subscription>> = self.0.subscriptions.borrow().values().cloned().collect();
        for subscription in subscriptions {
            if subscription.properties.borrow().contains(&PROPERTY_USER_PRESENCE) {
                subscription.connection.send(&notification);
                self.0.counters.sent_notifications.set(self.0.counters.sent_notifications.get() + 1);
            }
        }
        log::info!("user presence {} since {}", presence.value.unwrap_or_default(), presence.since_us);
    }

    /// Device replies and subscribed streams of the session that just started (tet-tcp route).
    pub fn attach_device(&self, device: DeviceReplies) {
        if let Some(tet_tcp) = &self.0.tet_tcp {
            tet_tcp.attach(device);
        }
    }

    pub fn publish_device_stream(&self, message: &Message, service_us: i64) {
        if let Some(tet_tcp) = &self.0.tet_tcp {
            tet_tcp.publish(message, service_us);
        }
    }

    /// The display area the game's legacy module got from the device, else the configured one.
    pub fn legacy_display_area(&self) -> Option<Corners> {
        self.0.tet_tcp.as_ref().and_then(|tet_tcp| tet_tcp.device_display_area()).or(self.0.display_area)
    }

    fn stop_stream(&self, subscription: &Subscription, stream: u32) {
        let Some(id) = subscription.streams.borrow_mut().remove(&stream) else { return };
        if stream == HEAD_POSE {
            self.0.hub.unsubscribe(id);
        } else if let Some(hub) = &self.0.gaze_hub {
            hub.unsubscribe(id);
        }
    }

    fn stop_all(&self, subscription: &Subscription) {
        let streams: Vec<u32> = subscription.streams.borrow().keys().copied().collect();
        for stream in streams {
            self.stop_stream(subscription, stream);
        }
        subscription.properties.borrow_mut().clear();
    }

    fn close_subscription(&self, subscription: &Subscription) {
        self.stop_all(subscription);
        subscription.connection.link.close();
        if let Some(command) = subscription.command.borrow().as_ref() {
            command.link.close();
        }
    }

    fn served(&self, stream: u32) -> bool {
        stream == HEAD_POSE || (self.0.gaze_hub.is_some() && GAZE_STREAMS.contains(&stream))
    }

    fn advertised_streams(&self) -> Vec<i32> {
        if self.0.gaze_hub.is_some() {
            GAZE_SERVICE_STREAMS.iter().map(|&stream| stream as i32).collect()
        } else {
            vec![HEAD_POSE as i32]
        }
    }

    fn send_stream(service: &Weak<Inner>, connection: &PipeConnection, packet: &[u8]) {
        let Some(inner) = service.upgrade() else { return };
        if connection.link.pending() > MAX_PENDING_STREAM_BYTES {
            inner.counters.dropped_packages.set(inner.counters.dropped_packages.get() + 1);
            return;
        }
        inner.counters.sent_packages.set(inner.counters.sent_packages.get() + 1);
        connection.send(packet);
    }

    fn start_stream(&self, subscription: &Subscription, stream: u32) {
        if subscription.streams.borrow().contains_key(&stream) {
            return;
        }
        let service = Rc::downgrade(&self.0);
        let connection = Rc::clone(&subscription.connection);
        let id = if stream == HEAD_POSE {
            self.0.hub.subscribe(move |sample: &HeadPoseSample| {
                Self::send_stream(&service, &connection, &stream_package(sample));
            })
        } else {
            let hub = self.0.gaze_hub.as_ref().expect("gaze streams are served with a gaze hub");
            hub.subscribe(move |frame: &GazeFrame| match gaze_package(frame, stream) {
                Some(packet) => Self::send_stream(&service, &connection, &packet),
                None => log::warn!("no PRP body for gaze stream {stream}"),
            })
        };
        subscription.streams.borrow_mut().insert(stream, id);
    }

    fn handshake_info(&self, connection: &Rc<PipeConnection>) -> Outcome<HandshakeInfo> {
        match connection.channel.get() {
            Some(COMMAND_CHANNEL) => Ok(HandshakeInfo {
                client_id: self.next_id(),
                subscription_url: format!("{INTERNAL_URL_SCHEME}{}", self.0.subscription_pipe),
                timesync_url: format!("{INTERNAL_URL_SCHEME}{}", self.0.timesync_pipe),
            }),
            Some(SUBSCRIPTION_CHANNEL) => {
                let subscription = Rc::new(Subscription {
                    id: self.next_id(),
                    connection: Rc::clone(connection),
                    command: RefCell::new(None),
                    field_of_use: Cell::new(0),
                    streams: RefCell::new(BTreeMap::new()),
                    properties: RefCell::new(BTreeSet::new()),
                });
                let id = subscription.id;
                *connection.subscription.borrow_mut() = Some(Rc::clone(&subscription));
                self.0.subscriptions.borrow_mut().insert(id, subscription);
                Ok(HandshakeInfo { client_id: id, subscription_url: String::new(), timesync_url: String::new() })
            }
            Some(TIMESYNC_CHANNEL) => {
                Ok(HandshakeInfo { client_id: 0, subscription_url: String::new(), timesync_url: String::new() })
            }
            channel => refused(format!("unknown channel {:#x}", channel.unwrap_or_default())),
        }
    }

    fn respond(&self, connection: &Rc<PipeConnection>, request: &CommandRequest) -> CommandResponse {
        let (tag, command) = (request.tag, request.command);
        if command == COMMAND_TIMESYNC {
            let now = now_us(&self.0.clock);
            return CommandResponse { tag, command, status: STATUS_OK, body: ResponseBody::Timesync(now as u64) };
        }
        if connection.channel.get() != Some(COMMAND_CHANNEL) {
            return match command {
                COMMAND_CONNECT => CommandResponse {
                    tag,
                    command,
                    status: STATUS_NOT_SUPPORTED,
                    body: ResponseBody::Connect(Box::default()),
                },
                _ => word_response(tag, command, STATUS_NOT_SUPPORTED, 0),
            };
        }
        if command == COMMAND_CONNECT {
            return self.connect(connection, request);
        }
        let stream_word = request.stream.unwrap_or(0);
        let Some(subscription) = connection.subscription.borrow().clone() else {
            return word_response(tag, command, STATUS_CONNECTION_FAILED, stream_word);
        };
        let stream = request.stream.unwrap_or(-1);
        if stream < 0 || !self.served(stream as u32) {
            let licensed = self.0.gaze_hub.is_some()
                && stream >= 0
                && LICENSED_STREAMS.contains(&(stream as u32))
                && command == COMMAND_STREAM_START;
            let status = if licensed { STATUS_INSUFFICIENT_LICENSE } else { STATUS_NOT_SUPPORTED };
            return word_response(tag, command, status, stream_word);
        }
        let stream = stream as u32;
        if command == COMMAND_STREAM_START {
            self.start_stream(&subscription, stream);
            log::info!("stream {stream} started for subscription {}", subscription.id);
            return word_response(tag, command, STATUS_OK, stream_word);
        }
        if !subscription.streams.borrow().contains_key(&stream) {
            return word_response(tag, command, STATUS_NOT_SUBSCRIBED, stream_word);
        }
        self.stop_stream(&subscription, stream);
        log::info!("stream {stream} stopped for subscription {}", subscription.id);
        word_response(tag, command, STATUS_OK, stream_word)
    }

    fn connect(&self, connection: &Rc<PipeConnection>, request: &CommandRequest) -> CommandResponse {
        let connect = request.connect.clone().unwrap_or_default();
        let subscription = self.0.subscriptions.borrow().get(&connect.subscription_client_id).cloned();
        let streams = self.advertised_streams();
        let Some(subscription) = subscription else {
            let response = decisions::device_connect_response(&self.0.serial, 0, &streams);
            return CommandResponse {
                tag: request.tag,
                command: COMMAND_CONNECT,
                status: STATUS_INVALID_PARAMETER,
                body: ResponseBody::Connect(Box::new(response)),
            };
        };
        *connection.subscription.borrow_mut() = Some(Rc::clone(&subscription));
        *subscription.command.borrow_mut() = Some(Rc::clone(connection));
        subscription.field_of_use.set(connect.field_of_use);
        let licenses = connect.licenses.len();
        log::info!(
            "connect from pid {} with {licenses} license(s), subscription {}",
            connect.process_id,
            subscription.id
        );
        let response: ConnectResponse = decisions::device_connect_response(&self.0.serial, licenses, &streams);
        CommandResponse {
            tag: request.tag,
            command: COMMAND_CONNECT,
            status: STATUS_OK,
            body: ResponseBody::Connect(Box::new(response)),
        }
    }

    /// Value of a property for a get, or for a subscribe (command 5), or a refusal.
    fn answer_property(
        &self,
        tag: u32,
        property_id: u32,
        subscribe: bool,
        subscription: Option<&Subscription>,
    ) -> Outcome<PropertyResponse> {
        let corners: Option<Vec<f64>> = self.0.display_area.map(|area| area.iter().flatten().copied().collect());
        let reported = self.0.reported_display_id.borrow().clone();
        let display_id = self.0.display_id.clone().filter(|id| !id.is_empty()).or(reported);
        let presence = self.0.presence.get();
        let source = PropertySource {
            display_area: corners.as_deref(),
            display_id: display_id.as_deref(),
            calibration_id: self.0.calibration_id.get(),
            presence: &presence,
            field_of_use: subscription.map(|s| s.field_of_use.get()),
        };
        Ok(decisions::answer_property(&source, tag, property_id, subscribe)?)
    }

    /// Command 5 answers with the current value and starts notifications; command 6 ends them.
    fn property_subscription(&self, connection: &Rc<PipeConnection>, packet: &[u8]) -> Outcome<Vec<u8>> {
        let (tag, command, property_id) = decode_property_subscribe_request(packet)?;
        let subscription = connection.subscription.borrow().clone();
        if command == COMMAND_PROPERTY_UNSUBSCRIBE {
            let subscribed = subscription.as_ref().is_some_and(|s| s.properties.borrow().contains(&property_id));
            let status = if LICENSED_SUBSCRIPTIONS.contains(&property_id) {
                STATUS_INSUFFICIENT_LICENSE
            } else if subscribed {
                STATUS_OK
            } else {
                STATUS_NOT_SUBSCRIBED
            };
            if let Some(subscription) = &subscription {
                subscription.properties.borrow_mut().remove(&property_id);
            }
            log::info!("property {property_id:#x} unsubscribe tag {tag} status {status}");
            let response = word_response(tag, command, status, property_id as i32);
            return Ok(encode_command_response(&response, true)?);
        }
        let response = self.answer_property(tag, property_id, true, subscription.as_deref())?;
        if response.status == STATUS_OK
            && let Some(subscription) = &subscription
        {
            subscription.properties.borrow_mut().insert(property_id);
        }
        log::info!("property {property_id:#x} subscribe tag {tag} status {}", response.status);
        Ok(encode_property_subscribe_response(&response))
    }

    fn handle_packet(&self, connection: &Rc<PipeConnection>, packet: &[u8]) -> Outcome {
        if connection.channel.get() == Some(SUBSCRIPTION_CHANNEL) {
            log::warn!("ignoring client packet on subscription channel");
            return Ok(());
        }
        if decisions::packet_operation(packet)? == OPERATION_PROPERTY_GET {
            let (tag, property_id) = decode_property_request(packet)?;
            let response = self.answer_property(tag, property_id, false, None)?;
            log::info!("property {property_id:#x} tag {tag} status {}", response.status);
            connection.send(&encode_property_response(&response));
            return Ok(());
        }
        let (tag, command) = decisions::request_header(packet)?;
        if [COMMAND_PROPERTY_SUBSCRIBE, COMMAND_PROPERTY_UNSUBSCRIBE].contains(&command) {
            let response = self.property_subscription(connection, packet)?;
            connection.send(&response);
            return Ok(());
        }
        let request = match decode_command_request(packet, true) {
            Ok(request) => request,
            Err(_) => {
                log::info!("command {command:#x} tag {tag} not supported");
                connection.send(&decisions::error_response(tag, command, STATUS_NOT_SUPPORTED));
                return Ok(());
            }
        };
        let response = self.respond(connection, &request);
        connection.send(&encode_command_response(&response, true)?);
        Ok(())
    }

    /// Steps 1 to 5 of docs/service-transport.md, as the real service answered them.
    async fn handshake(&self, connection: &Rc<PipeConnection>, frames: &mut Frames) -> Outcome {
        let hello = decode_client_hello(&frames.message(&connection.pipe).await?)?;
        connection.channel.set(Some(hello.channel));
        connection.link.frame(FRAME_REPLY_PIPE, &encode_names(&[hello.reply_pipe.as_str()])?);
        connection.send(&encode_handshake_ack());
        let pid = decode_client_pid(&frames.message(&connection.pipe).await?)?;
        if connection.client_pid != 0 && pid != connection.client_pid {
            return refused(format!("client pid {pid} differs from pipe client {}", connection.client_pid));
        }
        let info = self.handshake_info(connection)?;
        connection.send(&encode_handshake_info(&info)?);
        log::info!("pipe {} channel {:#x} handshake done, pid {pid}", connection.pipe, hello.channel);
        Ok(())
    }

    async fn run_pipe(&self, link: Link, frames: &mut Frames, client_pid: u32, pipe: String) -> Outcome {
        let connection = Rc::new(PipeConnection {
            link,
            pipe,
            client_pid,
            channel: Cell::new(None),
            subscription: RefCell::new(None),
        });
        self.0.connections.borrow_mut().push(Rc::clone(&connection));
        let outcome = async {
            self.handshake(&connection, frames).await?;
            let mut tail = Vec::new();
            loop {
                tail.extend_from_slice(&frames.message(&connection.pipe).await?);
                let (packets, rest) = split_packets(&tail, DEFAULT_MAX_PAYLOAD)?;
                let packets: Vec<Vec<u8>> = packets.into_iter().map(<[u8]>::to_vec).collect();
                tail = rest.to_vec();
                for packet in packets {
                    self.handle_packet(&connection, &packet)?;
                }
            }
        }
        .await;
        self.0.connections.borrow_mut().retain(|other| !Rc::ptr_eq(other, &connection));
        if let Some(subscription) = connection.subscription.borrow_mut().take() {
            self.0.subscriptions.borrow_mut().remove(&subscription.id);
            self.close_subscription(&subscription);
            subscription.command.borrow_mut().take();
        }
        outcome
    }

    /// One relay connection from the bridge: its control connection or one relayed pipe.
    pub async fn handle(&self, stream: TcpStream) {
        let peer = peer_repr(stream.peer_addr().ok());
        let (reader, writer) = stream.into_split();
        let link = Link::spawn(writer);
        let mut frames = Frames::new(reader, link.clone());
        let outcome = async {
            let (kind, payload) = frames.next().await?;
            match kind {
                FRAME_BRIDGE_HELLO => {
                    link.frame(FRAME_PIPES, &encode_names(&self.pipe_names())?);
                    self.serve_bridge(&peer, &link, &mut frames).await
                }
                FRAME_PIPE_OPEN => {
                    let (client_pid, pipe) = decisions::decode_pipe_open(&payload)?;
                    if !self.0.tracker_attached.get() {
                        log::info!("refusing pid {client_pid} on {pipe}: no tracker");
                        return Ok(());
                    }
                    if let Some(sesp) = &self.0.sesp
                        && pipe == tobii_service::sesp::PIPE_NAME
                    {
                        self.0.sesp_pipes.borrow_mut().push(link.clone());
                        let outcome = sesp.serve(self, link.clone(), &mut frames, client_pid).await;
                        self.0.sesp_pipes.borrow_mut().retain(|other| !other.same(&link));
                        return outcome;
                    }
                    self.run_pipe(link.clone(), &mut frames, client_pid, pipe).await
                }
                kind => refused(format!("unexpected first relay frame {kind}")),
            }
        }
        .await;
        if let Err(Ended::Refused(message)) = outcome {
            log::warn!("closing relay connection {peer}: {message}");
        }
        link.close();
    }

    /// Control connection: display id and QueryPerformanceCounter reports until it closes.
    async fn serve_bridge(&self, peer: &str, link: &Link, frames: &mut Frames) -> Outcome {
        let key = self.0.bridge_keys.get();
        self.0.bridge_keys.set(key + 1);
        let bridge = Rc::new(Bridge {
            key,
            peer: peer.into(),
            link: link.clone(),
            verdict_clock: Cell::new(None),
            conflict_logged: Cell::new(false),
        });
        self.0.bridges.borrow_mut().push(Rc::clone(&bridge));
        log::info!("bridge {peer} attached");
        let reported = Rc::new(Cell::new(false));
        let unreported = {
            let (reported, service, bridge) = (Rc::clone(&reported), self.clone(), Rc::clone(&bridge));
            tokio::task::spawn_local(async move {
                tokio::time::sleep(QPC_REPORT_WAIT).await;
                if !reported.get() {
                    service.note_unreported_clock(&bridge);
                }
            })
        };
        let outcome = async {
            loop {
                let (kind, payload) = frames.next().await?;
                if kind == FRAME_QPC {
                    let host_us = readings(&self.0.clock);
                    reported.set(true);
                    unreported.abort();
                    self.observe_bridge_clock(&bridge, &payload, host_us);
                } else if kind == FRAME_DISPLAY_ID {
                    let id = String::from_utf8_lossy(&payload).into_owned();
                    log::info!("bridge reports primary monitor {id}");
                    *self.0.reported_display_id.borrow_mut() = Some(id);
                }
            }
        }
        .await;
        unreported.abort();
        self.0.bridges.borrow_mut().retain(|other| !Rc::ptr_eq(other, &bridge));
        self.0.follower.borrow_mut().forget(&bridge.key);
        outcome
    }

    fn note_unreported_clock(&self, bridge: &Bridge) {
        log::info!(
            "bridge {} does not report its QueryPerformanceCounter (older bridge); service clock stays {}",
            bridge.peer,
            self.0.clock.state().source.name()
        );
    }

    /// One QueryPerformanceCounter report from `bridge`, with the Linux clocks read at receipt.
    fn observe_bridge_clock(&self, bridge: &Bridge, payload: &[u8], host_us: [i64; 3]) {
        let report = match decode_qpc_report(payload) {
            Ok(report) => report,
            Err(error) => {
                log::warn!("bridge {}: ignoring clock report: {error}", bridge.peer);
                return;
            }
        };
        if !report.details.is_empty() {
            let details: Vec<String> = report.details.iter().map(|(key, value)| format!("{key} {value}")).collect();
            log::info!("bridge {} runs {}", bridge.peer, details.join(", "));
        }
        let sample = QpcSample { qpc_us: report.micros() as i64, host_us: host_us.map(Some) };
        self.0.game_time.observe(sample);
        let Ok(decision) = self.0.follower.borrow_mut().observe(&self.0.clock, bridge.key, sample);
        let verdict = &decision.verdict;
        let source = self.0.clock.state().source;
        if verdict.clock != bridge.verdict_clock.get() {
            bridge.verdict_clock.set(verdict.clock);
            if let Some(clock) = verdict.clock {
                log::info!(
                    "bridge {}: QueryPerformanceCounter follows {} ({})",
                    bridge.peer,
                    clock.name(),
                    verdict.describe()
                );
                if clock != source && !self.0.clock.automatic() {
                    log::warn!(
                        "bridge {} counter follows {} but the service clock is fixed to {}; timestamps will drift \
                         (use --wine-clock auto or {})",
                        bridge.peer,
                        clock.name(),
                        source.name(),
                        clock.name()
                    );
                }
            }
        }
        if let Some(holder) = decision.held_by
            && !bridge.conflict_logged.get()
        {
            bridge.conflict_logged.set(true);
            let holder_peer = self
                .0
                .bridges
                .borrow()
                .iter()
                .find(|other| other.key == holder)
                .map_or_else(|| "None".to_string(), |other| other.peer.clone());
            let verdict_name = verdict.clock.map_or("None", |clock| clock.name());
            log::warn!(
                "bridge {} counter follows {verdict_name}, but bridge {holder_peer} still confirms {}; keeping {}",
                bridge.peer,
                source.name(),
                source.name()
            );
        }
        if let Some(switch) = decision.switch {
            self.clock_switched(switch, verdict.basis);
        }
    }

    /// Keep cached times on the new clock and make clients timesync again.
    ///
    /// The game layer timesyncs only when it creates or reconnects its device
    /// (docs/prp-bridge.md, Time base), so connected clients are closed and reconnect as
    /// after a service restart.
    fn clock_switched(&self, switch: ClockSwitch, basis: Basis) {
        log::info!(
            "service clock now follows {} instead of {} ({} match); moved {:+.3} ms",
            switch.new.name(),
            switch.old.name(),
            basis.name(),
            switch.delta_us as f64 / 1000.0
        );
        let mut presence = self.0.presence.get();
        if presence.value.is_some() {
            presence.since_us += switch.delta_us;
            self.0.presence.set(presence);
        }
        let connections = self.0.connections.borrow().clone();
        if !connections.is_empty() {
            log::info!("closing {} game connection(s) so they timesync on the new clock", connections.len());
            for connection in connections {
                connection.link.close();
            }
        }
    }

    /// Accepts relay connections on `listener` until the returned task is aborted.
    pub fn serve(&self, listener: TcpListener) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        tokio::task::spawn_local(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let service = service.clone();
                tokio::task::spawn_local(async move { service.handle(stream).await });
            }
        })
    }

    /// Disconnects every client and bridge.
    pub fn close(&self) {
        self.detach_tracker();
        if let Some(tet_tcp) = &self.0.tet_tcp {
            tet_tcp.detach();
        }
        let bridges = self.0.bridges.borrow().clone();
        for bridge in bridges {
            bridge.link.close();
        }
    }

    /// Applies the events of a tracker session until their sender is dropped.
    pub async fn follow(&self, mut events: mpsc::UnboundedReceiver<Event>) {
        while let Some(event) = events.recv().await {
            self.apply(event);
        }
    }
}

/// Binds like asyncio's start_server, with its error text for a failed bind.
pub async fn listen(host: &str, port: u16) -> std::io::Result<TcpListener> {
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
    let mut last = None;
    for address in &addresses {
        match TcpListener::bind(address).await {
            Ok(listener) => return Ok(listener),
            Err(error) => last = Some((address, error)),
        }
    }
    let Some((address, error)) = last else {
        return Err(std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no address to bind"));
    };
    let reason = error.to_string();
    let reason = reason.split(" (os error").next().unwrap_or(&reason).to_lowercase();
    let message = format!(
        "[Errno {}] error while attempting to bind on address {}: {reason}",
        error.raw_os_error().unwrap_or(0),
        peer_repr(Some(*address))
    );
    Err(std::io::Error::new(error.kind(), message))
}
