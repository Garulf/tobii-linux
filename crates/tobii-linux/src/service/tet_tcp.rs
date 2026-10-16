//! Production tet-tcp endpoint: the game DLL's legacy tracker route served from the session.
//!
//! The game DLL reports `tet-tcp://127.0.0.1` when TCP 127.0.0.1:4455 accepts a connection
//! and then runs its legacy TTP module against that port (docs/tet-tcp.md). Nothing a game
//! sends reaches the tracker:
//!
//! - the startup replies (protocol upgrade, unlock_ppu) and the getters the module asks for
//!   are the device's own replies, read once per tracker session before its streams start;
//! - subscribe and unsubscribe of gaze (0x500) and presence (0x504) only change which
//!   connections receive the service's stream, which every connection gets from the one
//!   device subscription the service holds;
//! - everything else, including every write, is refused with the device's
//!   unsupported-operation status.
//!
//! Stream messages keep their bytes except column 1, the device timestamp, which becomes the
//! game's QueryPerformanceCounter microseconds: the module passes it through unchanged
//! because timesync (UDP 4457) never connects under Wine. The port listens only while a
//! tracker session runs. Answers and restamping are tobii_service::tet_tcp.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

use tobii_protocol::Message;
use tobii_protocol::commands::STREAM_DATA_KIND;
use tobii_protocol::ttp::Decoder;
use tobii_service::tet_tcp::{Answer, MAX_PENDING_BYTES, STREAMS, answer, restamp};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

use super::GameTime;
use super::link::Link;
use crate::tracker::DeviceReplies;

#[derive(Debug)]
struct Connection {
    number: u64,
    link: Link,
    streams: RefCell<BTreeSet<u32>>,
}

#[derive(Debug)]
struct Listening {
    accept: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
struct Inner {
    game_time: GameTime,
    host: String,
    port: u16,
    device: RefCell<DeviceReplies>,
    server: RefCell<Option<Listening>>,
    bound_port: Cell<Option<u16>>,
    connections: RefCell<Vec<Rc<Connection>>>,
    numbers: Cell<u64>,
    sent: Cell<u64>,
    dropped: Cell<u64>,
    attaching: Cell<u64>,
}

/// Listens on 127.0.0.1:4455 while a tracker session runs; all calls on the event loop.
#[derive(Debug, Clone)]
pub struct TetTcpService(Rc<Inner>);

fn stream_list(streams: &BTreeSet<u32>) -> String {
    streams.iter().map(|stream| format!("{stream:#x}")).collect::<Vec<_>>().join(", ")
}

impl TetTcpService {
    pub fn new(game_time: GameTime, host: String, port: u16) -> Self {
        Self(Rc::new(Inner {
            game_time,
            host,
            port,
            device: RefCell::new(DeviceReplies::default()),
            server: RefCell::new(None),
            bound_port: Cell::new(None),
            connections: RefCell::new(Vec::new()),
            numbers: Cell::new(1),
            sent: Cell::new(0),
            dropped: Cell::new(0),
            attaching: Cell::new(0),
        }))
    }

    pub fn device_display_area(&self) -> Option<super::Corners> {
        self.0.device.borrow().display_area()
    }

    pub fn port_bound(&self) -> Option<u16> {
        self.0.bound_port.get()
    }

    pub fn connections(&self) -> usize {
        self.0.connections.borrow().len()
    }

    pub fn sent(&self) -> u64 {
        self.0.sent.get()
    }

    pub fn dropped(&self) -> u64 {
        self.0.dropped.get()
    }

    /// Takes the replies of the session that just started and listens if not yet listening.
    pub fn attach(&self, device: DeviceReplies) {
        let streams = device.streams.clone();
        *self.0.device.borrow_mut() = device;
        if self.0.server.borrow().is_some() {
            return;
        }
        let generation = self.0.attaching.get() + 1;
        self.0.attaching.set(generation);
        let service = self.clone();
        tokio::task::spawn_local(async move {
            let listener = match super::listen(&service.0.host, service.0.port).await {
                Ok(listener) => listener,
                Err(error) => {
                    log::warn!("tet-tcp route not served: {error}");
                    return;
                }
            };
            if service.0.attaching.get() != generation || service.0.server.borrow().is_some() {
                return;
            }
            service.0.bound_port.set(listener.local_addr().ok().map(|address| address.port()));
            log::info!("tet-tcp route on {}:{} (streams {})", service.0.host, service.0.port, stream_list(&streams));
            let accept = service.accept_loop(listener);
            *service.0.server.borrow_mut() = Some(Listening { accept });
        });
    }

    fn accept_loop(&self, listener: TcpListener) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        tokio::task::spawn_local(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let service = service.clone();
                tokio::task::spawn_local(async move { service.serve(stream).await });
            }
        })
    }

    /// Stops listening and closes every connection.
    pub fn detach(&self) {
        self.0.attaching.set(self.0.attaching.get() + 1);
        if let Some(server) = self.0.server.borrow_mut().take() {
            server.accept.abort();
            self.0.bound_port.set(None);
            log::info!("tet-tcp route closed with {} connection(s)", self.0.connections.borrow().len());
        }
        let connections = self.0.connections.borrow().clone();
        for connection in connections {
            connection.link.close();
        }
    }

    async fn serve(&self, stream: tokio::net::TcpStream) {
        let number = self.0.numbers.get();
        self.0.numbers.set(number + 1);
        let (mut reader, writer) = stream.into_split();
        let link = Link::spawn(writer);
        let connection = Rc::new(Connection { number, link: link.clone(), streams: RefCell::new(BTreeSet::new()) });
        self.0.connections.borrow_mut().push(Rc::clone(&connection));
        let mut decoder = Decoder::default();
        let mut buffer = vec![0u8; 65536];
        loop {
            let read = tokio::select! {
                read = reader.read(&mut buffer) => read,
                () = link.closed() => break,
            };
            let count = match read {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) => {
                    log::info!("tet-tcp connection {number} ended: {error}");
                    break;
                }
            };
            match decoder.feed(&buffer[..count]) {
                Ok(messages) => messages.iter().for_each(|message| self.handle(&connection, message)),
                Err(error) => {
                    log::info!("tet-tcp connection {number} ended: {error}");
                    break;
                }
            }
        }
        self.0.connections.borrow_mut().retain(|other| !Rc::ptr_eq(other, &connection));
        link.close();
    }

    fn handle(&self, connection: &Connection, message: &Message) {
        let reply = {
            let device = self.0.device.borrow();
            answer(message, &device.replies, &device.streams)
        };
        let number = connection.number;
        let reply = match reply {
            Answer::Ignore => None,
            Answer::Reply(reply) => Some(reply),
            Answer::Subscribe { reply, stream, subscribe } => {
                let action = if subscribe {
                    connection.streams.borrow_mut().insert(stream);
                    "subscribe"
                } else {
                    connection.streams.borrow_mut().remove(&stream);
                    "unsubscribe"
                };
                log::info!("tet-tcp connection {number}: {action} {stream:#x}");
                Some(reply)
            }
            Answer::Refuse { reply, stream, subscription: true } => {
                let stream = stream.map_or_else(|| "None".to_string(), |stream| stream.to_string());
                log::info!("tet-tcp connection {number}: stream {stream} not served");
                Some(reply)
            }
            Answer::Refuse { reply, subscription: false, .. } => {
                log::info!("tet-tcp connection {number}: command {} refused", message.command);
                Some(reply)
            }
        };
        if let Some(reply) = reply
            && !connection.link.is_closing()
        {
            match reply.encode() {
                Ok(data) => connection.link.write(data),
                Err(error) => log::warn!("tet-tcp connection {number}: reply not sent: {error}"),
            }
        }
    }

    /// One device stream message, received at `service_us` on the service clock.
    pub fn publish(&self, message: &Message, service_us: i64) {
        if message.kind != STREAM_DATA_KIND || !STREAMS.contains(&message.command) {
            return;
        }
        let targets: Vec<Rc<Connection>> = self
            .0
            .connections
            .borrow()
            .iter()
            .filter(|connection| connection.streams.borrow().contains(&message.command))
            .cloned()
            .collect();
        if targets.is_empty() {
            return;
        }
        let payload = match restamp(&message.payload, self.0.game_time.map(service_us)) {
            Ok(payload) => payload,
            Err(error) => {
                log::warn!("tet-tcp stream message not restamped: {error}");
                return;
            }
        };
        let restamped =
            Message::new(message.kind, message.request_id, message.word2, message.command, message.word4, payload);
        let Ok(data) = restamped.encode() else { return };
        for connection in targets {
            if connection.link.is_closing() {
                continue;
            }
            if connection.link.pending() > MAX_PENDING_BYTES {
                self.0.dropped.set(self.0.dropped.get() + 1);
                continue;
            }
            connection.link.write(data.clone());
            self.0.sent.set(self.0.sent.get() + 1);
        }
    }
}
