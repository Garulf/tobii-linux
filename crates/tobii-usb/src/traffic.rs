//! Raw USB traffic log for live experiments: every transfer as one JSON line.
//!
//! [`LoggingTransport`] records each bulk write, each bulk read (including the partial
//! data of a read that timed out, and every chunk of the continuous reader) and each
//! control transfer with the host monotonic time, before any framing or TTP decoding.
//! [`replay_messages`] rebuilds the TTP messages of one channel from such a log with the
//! session's parsers, so a stalled run can be analysed offline.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use tobii_protocol::framing::IncomingFrames;
use tobii_protocol::ttp::Decoder;
use tobii_protocol::{Message, ProtocolError};

use crate::error::{Error, Result};
use crate::transport::{ContinuousSource, Transport};

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn round(value: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (value * scale).round() / scale
}

fn milliseconds(since: Instant) -> f64 {
    round(since.elapsed().as_secs_f64() * 1000.0, 1)
}

#[derive(Debug)]
pub struct TrafficLog {
    file: Mutex<Option<File>>,
    start: Instant,
}

impl TrafficLog {
    pub fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let log = Self { file: Mutex::new(Some(File::create(path)?)), start: Instant::now() };
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        log.record("start", [("wall_time", json!(wall))]);
        Ok(log)
    }

    /// One line `{"t": seconds since start, "op": op, ...values}`; writing errors are ignored.
    pub fn record<'a>(&self, op: &str, values: impl IntoIterator<Item = (&'a str, Value)>) {
        let mut line = Map::new();
        line.insert("t".into(), json!(round(self.start.elapsed().as_secs_f64(), 6)));
        line.insert("op".into(), json!(op));
        for (key, value) in values {
            line.insert(key.into(), value);
        }
        let mut file = self.file.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(file) = file.as_mut() {
            let _ = writeln!(file, "{}", Value::Object(line));
        }
    }

    pub fn close(&self) {
        self.file.lock().unwrap_or_else(|p| p.into_inner()).take();
    }
}

/// A transport that records every transfer of the transport it wraps.
#[derive(Debug)]
pub struct LoggingTransport<T: ?Sized> {
    log: Arc<TrafficLog>,
    inner: Arc<T>,
}

impl<T: ?Sized> LoggingTransport<T> {
    pub fn new(inner: Arc<T>, log: Arc<TrafficLog>) -> Self {
        Self { log, inner }
    }

    pub fn inner(&self) -> &Arc<T> {
        &self.inner
    }
}

impl<T: Transport + ?Sized> Transport for LoggingTransport<T> {
    fn read(&self, size: usize, timeout_ms: u32) -> Result<Vec<u8>> {
        let begin = Instant::now();
        match self.inner.read(size, timeout_ms) {
            Ok(data) => {
                let values = [
                    ("timeout_ms", json!(timeout_ms)),
                    ("hex", json!(hex(&data))),
                    ("waited_ms", json!(milliseconds(begin))),
                ];
                self.log.record("read", values);
                Ok(data)
            }
            Err(Error::Usb(error)) => {
                let values = [
                    ("code", json!(error.code)),
                    ("timeout_ms", json!(timeout_ms)),
                    ("hex", json!(hex(&error.partial_data))),
                    ("waited_ms", json!(milliseconds(begin))),
                ];
                self.log.record("read_error", values);
                Err(Error::Usb(error))
            }
            Err(error) => Err(error),
        }
    }

    fn write(&self, data: &[u8], timeout_ms: u32) -> Result<usize> {
        let begin = Instant::now();
        match self.inner.write(data, timeout_ms) {
            Ok(count) => {
                let values = [
                    ("hex", json!(hex(data))),
                    ("transferred", json!(count)),
                    ("waited_ms", json!(milliseconds(begin))),
                ];
                self.log.record("write", values);
                Ok(count)
            }
            Err(Error::Usb(error)) => {
                let values =
                    [("code", json!(error.code)), ("hex", json!(hex(data))), ("transferred", json!(error.transferred))];
                self.log.record("write_error", values);
                Err(Error::Usb(error))
            }
            Err(error) => Err(error),
        }
    }

    fn control_out(&self, request: u8, value: u16, index: u16, data: &[u8], timeout_ms: u32) -> Result<usize> {
        match self.inner.control_out(request, value, index, data, timeout_ms) {
            Ok(count) => {
                self.log.record("control_out", [("request", json!(request)), ("value", json!(value))]);
                Ok(count)
            }
            Err(Error::Usb(error)) => {
                let values = [("request", json!(request)), ("value", json!(value)), ("code", json!(error.code))];
                self.log.record("control_out_error", values);
                Err(Error::Usb(error))
            }
            Err(error) => Err(error),
        }
    }

    fn control_in(&self, request: u8, value: u16, index: u16, length: u16, timeout_ms: u32) -> Result<Vec<u8>> {
        let data = self.inner.control_in(request, value, index, length, timeout_ms)?;
        self.log
            .record("control_in", [("request", json!(request)), ("value", json!(value)), ("hex", json!(hex(&data)))]);
        Ok(data)
    }

    fn continuous_source(&self) -> Option<Box<dyn ContinuousSource>> {
        let source = self.inner.continuous_source()?;
        Some(Box::new(LoggingSource { source, log: Arc::clone(&self.log) }))
    }
}

struct LoggingSource {
    source: Box<dyn ContinuousSource>,
    log: Arc<TrafficLog>,
}

impl ContinuousSource for LoggingSource {
    fn next_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let begin = Instant::now();
        let timeout_ms = json!(timeout.as_millis() as u64);
        let chunk = self.source.next_chunk(timeout);
        match &chunk {
            Ok(Some(data)) => {
                self.log.record(
                    "read",
                    [("timeout_ms", timeout_ms), ("hex", json!(hex(data))), ("waited_ms", json!(milliseconds(begin)))],
                );
            }
            Err(Error::Usb(error)) => {
                let values = [
                    ("code", json!(error.code)),
                    ("timeout_ms", timeout_ms),
                    ("hex", json!(hex(&error.partial_data))),
                    ("waited_ms", json!(milliseconds(begin))),
                ];
                self.log.record("read_error", values);
            }
            _ => {}
        }
        chunk
    }

    fn shutdown(self: Box<Self>) {
        self.source.shutdown();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Out,
    In,
}

impl Direction {
    pub fn name(self) -> &'static str {
        match self {
            Direction::Out => "out",
            Direction::In => "in",
        }
    }
}

/// One replayed message, or the error that ended the incoming stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Replayed {
    pub time: f64,
    pub direction: Direction,
    pub message: std::result::Result<Message, String>,
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

fn bad_log(reason: &str) -> Error {
    Error::invalid(format!("Malformed traffic log: {reason}"))
}

fn protocol(error: ProtocolError) -> Error {
    Error::Foreign(Box::new(error))
}

/// Messages of `channel` in a traffic log, decoded as the session would decode them.
pub fn replay_messages(path: impl AsRef<Path>, channel: u32) -> Result<Vec<Replayed>> {
    let file = File::open(path).map_err(|e| Error::Foreign(Box::new(e)))?;
    let (mut frames, mut received, mut sent) = (IncomingFrames::default(), Decoder::default(), Decoder::default());
    let mut replayed = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| Error::Foreign(Box::new(e)))?;
        let row: Value = serde_json::from_str(&line).map_err(|_| bad_log("a line is not JSON"))?;
        let time = row["t"].as_f64().unwrap_or_default();
        let op = row["op"].as_str().unwrap_or_default();
        let data = row["hex"].as_str().and_then(unhex).unwrap_or_default();
        if op == "write" {
            for message in sent.feed(data.get(8..).unwrap_or_default()).map_err(protocol)? {
                replayed.push(Replayed { time, direction: Direction::Out, message: Ok(message) });
            }
        } else if (op == "read" || op == "read_error") && !data.is_empty() {
            let incoming = frames.feed(&data).map_err(|e| format!("ValueError: {e}")).and_then(|frames| {
                let mut messages = Vec::new();
                for frame in frames {
                    if frame.for_channel(channel).map_err(|e| format!("ValueError: {e}"))? {
                        messages.extend(received.feed(&frame.payload).map_err(|e| format!("ProtocolError: {e}"))?);
                    }
                }
                Ok(messages)
            });
            match incoming {
                Ok(messages) => replayed.extend(messages.into_iter().map(|m| Replayed {
                    time,
                    direction: Direction::In,
                    message: Ok(m),
                })),
                Err(text) => {
                    replayed.push(Replayed { time, direction: Direction::In, message: Err(text) });
                    break;
                }
            }
        }
    }
    Ok(replayed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{LIBUSB_ERROR_TIMEOUT, UsbError};

    #[derive(Default)]
    struct Framed {
        reads: Mutex<Vec<Result<Vec<u8>>>>,
    }

    impl Transport for Framed {
        fn read(&self, _: usize, _: u32) -> Result<Vec<u8>> {
            let mut reads = self.reads.lock().unwrap();
            if reads.is_empty() {
                Err(UsbError::new("bulk transfer", LIBUSB_ERROR_TIMEOUT).into())
            } else {
                reads.remove(0)
            }
        }
        fn write(&self, data: &[u8], _: u32) -> Result<usize> {
            Ok(data.len())
        }
        fn control_out(&self, _: u8, _: u16, _: u16, _: &[u8], _: u32) -> Result<usize> {
            Ok(0)
        }
        fn control_in(&self, _: u8, _: u16, _: u16, _: u16, _: u32) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn records_transfers_and_replays_messages() {
        let reply = Message::new(0x52, 1, 0, 1490, 0, Vec::new()).encode().unwrap();
        let mut frame = 1u32.to_le_bytes().to_vec();
        frame.extend(((reply.len() + 8) as u32).to_le_bytes());
        frame.extend(&reply);
        let partial = UsbError {
            partial_data: frame[..10].to_vec(),
            transferred: 10,
            ..UsbError::new("bulk transfer", LIBUSB_ERROR_TIMEOUT)
        };
        let transport = Arc::new(Framed { reads: Mutex::new(vec![Err(partial.into()), Ok(frame[10..].to_vec())]) });
        let directory = std::env::temp_dir().join(format!("tobii-traffic-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("raw.jsonl");
        let log = Arc::new(TrafficLog::create(&path).unwrap());
        let logging = Arc::new(LoggingTransport::new(transport, Arc::clone(&log)));
        let mut channel = crate::UsbChannel::new(logging, 0, 1000).unwrap();
        channel.open().unwrap();
        let request = Message::new(0x51, 1, 0, 1490, 0, Vec::new()).encode().unwrap();
        channel.write(&request, 3000).unwrap();
        assert!(channel.read(10).unwrap().is_empty());
        assert_eq!(channel.read(10).unwrap(), reply);
        channel.close().unwrap();
        log.close();
        let text = std::fs::read_to_string(&path).unwrap();
        let ops: Vec<String> = text
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["op"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ops, ["start", "control_out", "write", "read_error", "read", "control_out"]);
        let replayed: Vec<(&str, u32)> = replay_messages(&path, 0)
            .unwrap()
            .iter()
            .map(|r| (r.direction.name(), r.message.as_ref().unwrap().command))
            .collect();
        assert_eq!(replayed, [("out", 1490), ("in", 1490)]);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
