//! A simulated Wine bridge: relayed pipe connections seen from the game client's side,
//! built from the client-side codecs. No pipes or Wine are involved.

#![allow(dead_code)]

use std::cell::RefCell;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tobii_linux::service::clock::{Clock, Clocks, wine_clock};
use tobii_linux::service::{Options, PrpService};
use tobii_service::prp::DEFAULT_MAX_PAYLOAD;
use tobii_service::prp_service::{
    FRAME_BRIDGE_HELLO, FRAME_DISPLAY_ID, FRAME_MESSAGE, FRAME_PIPE_OPEN, FRAME_PIPES, FRAME_QPC, FRAME_REPLY_PIPE,
    decode_frame_header, decode_names, encode_frame, encode_pipe_open,
};
use tobii_service::prp_session::{
    ClientHello, HandshakeInfo, check_handshake_ack, decode_handshake_info, encode_client_hello, encode_client_pid,
    pipe_chunks, split_packets,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const CLIENT_PID: u32 = 0x1234;
pub const FIXED_CLOCK_US: i64 = 0x0000_00CB_E8A7_1FC0;
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// Runs `test` on a single-threaded loop, as the service runs.
pub fn run<F: Future>(test: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&runtime, test)
}

pub fn fixed_clock() -> Arc<Clock> {
    wine_clock("boottime", Clocks::Scripted(Arc::new(|_| FIXED_CLOCK_US))).unwrap()
}

/// A service listening on a free loopback port.
pub async fn service(options: Options, clock: Arc<Clock>) -> (PrpService, u16, tokio::task::JoinHandle<()>) {
    let service = PrpService::new(options, clock);
    let listener = tobii_linux::service::listen("127.0.0.1", 0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accept = service.serve(listener);
    (service, port, accept)
}

pub fn options(name: &str) -> Options {
    Options { name: name.into(), ..Options::default() }
}

/// Reads one relay frame; `None` at the end of the connection.
pub async fn read_frame(stream: &mut TcpStream) -> Option<(u32, Vec<u8>)> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header).await.ok()?;
    let (kind, size) = decode_frame_header(&header).ok()?;
    let mut payload = vec![0; size];
    stream.read_exact(&mut payload).await.ok()?;
    Some((kind, payload))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    Message(Vec<u8>),
    Closed,
    Timeout,
}

/// One pipe connection as the bridge relays it, seen from the client's side.
#[derive(Debug)]
pub struct RelayedChannel {
    pub stream: TcpStream,
    pub reply_pipe: Option<String>,
    tail: Vec<u8>,
}

impl RelayedChannel {
    pub async fn open(port: u16, pipe: &str, client_pid: u32) -> Self {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream
            .write_all(&encode_frame(FRAME_PIPE_OPEN, &encode_pipe_open(client_pid, pipe).unwrap()).unwrap())
            .await
            .unwrap();
        Self { stream, reply_pipe: None, tail: Vec::new() }
    }

    pub async fn send(&mut self, message: &[u8]) {
        let _ = self.stream.write_all(&encode_frame(FRAME_MESSAGE, message).unwrap()).await;
    }

    pub async fn send_packet(&mut self, packet: &[u8]) {
        for chunk in pipe_chunks(packet) {
            self.send(chunk).await;
        }
    }

    pub async fn receive_within(&mut self, timeout: Duration) -> Received {
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

    pub async fn receive(&mut self) -> Vec<u8> {
        match self.receive_within(TIMEOUT).await {
            Received::Message(message) => message,
            other => panic!("expected a message, got {other:?}"),
        }
    }

    pub async fn packet_within(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        loop {
            let first = split_packets(&self.tail, DEFAULT_MAX_PAYLOAD).unwrap().0.first().map(|p| p.to_vec());
            if let Some(first) = first {
                self.tail.drain(..first.len());
                return Some(first);
            }
            match self.receive_within(timeout).await {
                Received::Message(message) => self.tail.extend(message),
                _ => return None,
            }
        }
    }

    pub async fn packet(&mut self) -> Vec<u8> {
        self.packet_within(TIMEOUT).await.expect("a packet")
    }

    pub async fn handshake(&mut self, channel: u32, reply_pipe: &str, pid: u32) -> HandshakeInfo {
        self.send(&encode_client_hello(&ClientHello { channel, reply_pipe: reply_pipe.into(), version: 1 }).unwrap())
            .await;
        check_handshake_ack(&self.receive().await).unwrap();
        self.send(&encode_client_pid(pid)).await;
        decode_handshake_info(&self.receive().await).unwrap()
    }

    /// True once the service has closed the connection; messages before that are skipped.
    pub async fn closed(&mut self) -> bool {
        for _ in 0..1000 {
            match self.receive_within(TIMEOUT).await {
                Received::Closed => return true,
                Received::Timeout => return false,
                Received::Message(_) => {}
            }
        }
        false
    }
}

/// A bridge control connection, after the hello and the pipe list.
#[derive(Debug)]
pub struct Bridge {
    pub stream: TcpStream,
    pub pipes: Vec<String>,
    markers: u32,
}

impl Bridge {
    pub async fn attach(port: u16) -> Self {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(&encode_frame(FRAME_BRIDGE_HELLO, &1u32.to_le_bytes()).unwrap()).await.unwrap();
        let (kind, payload) = read_frame(&mut stream).await.unwrap();
        assert_eq!(kind, FRAME_PIPES);
        Self { stream, pipes: decode_names(&payload).unwrap(), markers: 0 }
    }

    pub async fn send(&mut self, kind: u32, payload: &[u8]) {
        self.stream.write_all(&encode_frame(kind, payload).unwrap()).await.unwrap();
    }

    /// Sends a QueryPerformanceCounter report and waits until the service has handled it.
    pub async fn report(&mut self, service: &PrpService, payload: &[u8]) {
        self.send(FRAME_QPC, payload).await;
        self.sync(service).await;
    }

    /// Waits until the service has handled every frame sent so far.
    ///
    /// The service handles a bridge's frames in order, so a display id sent last shows
    /// that the frames before it were handled.
    pub async fn sync(&mut self, service: &PrpService) {
        self.markers += 1;
        let marker = format!("marker {}", self.markers);
        self.send(FRAME_DISPLAY_ID, marker.as_bytes()).await;
        let handled = eventually(|| service.reported_display_id().as_deref() == Some(marker.as_str())).await;
        assert!(handled, "frames not handled");
    }
}

/// Waits up to two seconds for `condition`.
pub async fn eventually(mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    condition()
}

pub fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

thread_local! {
    static LOGGED: RefCell<Vec<(log::Level, String)>> = const { RefCell::new(Vec::new()) };
}

/// Keeps the info and warning records of each thread; every test runs on its own thread.
struct LogCapture;

impl log::Log for LogCapture {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            LOGGED.with(|logged| logged.borrow_mut().push((record.level(), record.args().to_string())));
        }
    }

    fn flush(&self) {}
}

/// Starts capturing this thread's log records, dropping what was captured before.
pub fn capture_logs() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        log::set_logger(&LogCapture).expect("no other logger in the test binary");
        log::set_max_level(log::LevelFilter::Info);
    });
    LOGGED.with(|logged| logged.borrow_mut().clear());
}

/// The messages logged on this thread at `level` or above since the last call, as
/// `assertLogs` collects them.
pub fn take_logs(level: log::Level) -> Vec<String> {
    LOGGED.with(|logged| {
        logged.borrow_mut().drain(..).filter(|(logged, _)| *logged <= level).map(|(_, message)| message).collect()
    })
}

pub mod fake_tracker;
