//! A blocking control client for programs without an async runtime (the GTK apps use it
//! from a thread of their own).

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use tobii_service::prp_service::DEFAULT_PORT;

use crate::ControlError;
use crate::protocol::{
    Event, FRAME_CONTROL_EVENT, FRAME_CONTROL_GAZE, FRAME_CONTROL_HELLO, FRAME_CONTROL_REPLY, GazeSample, Hello,
    Request, Status, decode_event, decode_reply, encode, encode_request, split_frame,
};

/// Where `tobii-linux serve` listens by default.
pub const DEFAULT_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT);

/// Something the service sent without being asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Event(Event),
    Gaze(GazeSample),
}

/// One control connection; requests block until their reply arrives.
#[derive(Debug)]
pub struct ControlClient {
    stream: TcpStream,
    next_id: u64,
    received: Vec<u8>,
    pending: VecDeque<Incoming>,
}

impl ControlClient {
    /// Connects and introduces itself as `client` ("experience" or "setup").
    pub fn connect(addr: SocketAddr, client: &str, timeout: Duration) -> Result<ControlClient, ControlError> {
        let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(timeout))?;
        stream.write_all(&encode(FRAME_CONTROL_HELLO, &Hello::new(client))?)?;
        Ok(Self { stream, next_id: 1, received: Vec::new(), pending: VecDeque::new() })
    }

    /// Sends `request` and waits up to `timeout` for its reply; events that arrive meanwhile
    /// are kept for [`ControlClient::next`].
    pub fn request(&mut self, request: Request, timeout: Duration) -> Result<Status, ControlError> {
        let id = self.next_id;
        self.next_id += 1;
        self.stream.write_all(&encode_request(id, &request)?)?;
        let deadline = Instant::now() + timeout;
        loop {
            let Some((kind, payload)) = self.read_frame(deadline)? else { return Err(ControlError::Timeout) };
            if kind == FRAME_CONTROL_REPLY {
                let reply = decode_reply(&payload)?;
                if reply.id == id {
                    return reply.result.map_err(ControlError::Refused);
                }
                continue;
            }
            if let Some(incoming) = incoming(kind, &payload)? {
                self.pending.push_back(incoming);
            }
        }
    }

    /// The next event or gaze sample, or `None` when nothing arrives within `timeout`.
    pub fn next(&mut self, timeout: Duration) -> Result<Option<Incoming>, ControlError> {
        if let Some(incoming) = self.pending.pop_front() {
            return Ok(Some(incoming));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let Some((kind, payload)) = self.read_frame(deadline)? else { return Ok(None) };
            if let Some(incoming) = incoming(kind, &payload)? {
                return Ok(Some(incoming));
            }
        }
    }

    fn read_frame(&mut self, deadline: Instant) -> Result<Option<(u32, Vec<u8>)>, ControlError> {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some((kind, payload, used)) = split_frame(&self.received)? {
                self.received.drain(..used);
                return Ok(Some((kind, payload)));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            self.stream.set_read_timeout(Some(remaining))?;
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(ControlError::Closed),
                Ok(read) => self.received.extend_from_slice(&chunk[..read]),
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Ok(None),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
}

fn incoming(kind: u32, payload: &[u8]) -> Result<Option<Incoming>, ControlError> {
    Ok(match kind {
        FRAME_CONTROL_EVENT => Some(Incoming::Event(decode_event(payload)?)),
        FRAME_CONTROL_GAZE => Some(Incoming::Gaze(GazeSample::decode(payload)?)),
        _ => None,
    })
}
