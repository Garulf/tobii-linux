use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;

use super::*;
use crate::error::{LIBUSB_ERROR_NO_DEVICE, LIBUSB_ERROR_TIMEOUT, UsbError};

fn inbound(mask: u32, payload: &[u8]) -> Vec<u8> {
    let mut data = mask.to_le_bytes().to_vec();
    data.extend_from_slice(&((payload.len() + 8) as u32).to_le_bytes());
    data.extend_from_slice(payload);
    data
}

#[derive(Default)]
struct FakeTransport {
    controls: Mutex<Vec<(u8, u16)>>,
    writes: Mutex<Vec<Vec<u8>>>,
    reads: Mutex<VecDeque<Result<Vec<u8>>>>,
    short: bool,
    stream: Mutex<Option<FakeStream>>,
}

impl Transport for FakeTransport {
    fn read(&self, _size: usize, _timeout_ms: u32) -> Result<Vec<u8>> {
        self.reads.lock().unwrap().pop_front().unwrap_or(Ok(Vec::new()))
    }

    fn write(&self, data: &[u8], _timeout_ms: u32) -> Result<usize> {
        self.writes.lock().unwrap().push(data.to_vec());
        Ok(if self.short { 3 } else { data.len() })
    }

    fn control_out(&self, request: u8, value: u16, _index: u16, _data: &[u8], _timeout_ms: u32) -> Result<usize> {
        self.controls.lock().unwrap().push((request, value));
        Ok(0)
    }

    fn control_in(&self, _request: u8, _value: u16, _index: u16, _length: u16, _timeout_ms: u32) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn continuous_source(&self) -> Option<Box<dyn ContinuousSource>> {
        self.stream.lock().unwrap().take().map(|stream| Box::new(stream) as Box<dyn ContinuousSource>)
    }
}

#[test]
fn explicit_lifecycle_and_demultiplex() {
    let transport = Arc::new(FakeTransport::default());
    let wire = [inbound(2, b"ignore"), inbound(1, b"ab")[..9].to_vec()].concat();
    transport.reads.lock().unwrap().extend([Ok(wire), Ok(b"b".to_vec())]);
    let mut channel = UsbChannel::new(transport.clone(), 0, 1000).unwrap();
    assert!(transport.controls.lock().unwrap().is_empty());
    channel.open().unwrap();
    assert!(!channel.is_continuous());
    assert_eq!(channel.write(b"abc", 3000).unwrap(), 3);
    assert_eq!(channel.read(1000).unwrap(), b"");
    assert_eq!(channel.read(1000).unwrap(), b"ab");
    channel.close().unwrap();
    channel.close().unwrap();
    assert_eq!(*transport.controls.lock().unwrap(), [(0x41, 0), (0x42, 0)]);
    assert!(channel.open().is_err());
}

#[test]
fn timeout_keeps_partial_bytes_and_errors_fail_the_channel() {
    let transport = Arc::new(FakeTransport::default());
    let frame = inbound(1, b"hello");
    let timed_out =
        UsbError { partial_data: frame[..10].to_vec(), ..UsbError::new("bulk transfer", LIBUSB_ERROR_TIMEOUT) };
    transport.reads.lock().unwrap().extend([
        Err(timed_out.into()),
        Ok(frame[10..].to_vec()),
        Err(UsbError::new("bulk transfer", LIBUSB_ERROR_NO_DEVICE).into()),
    ]);
    let mut channel = UsbChannel::new(transport, 0, 1000).unwrap();
    channel.open().unwrap();
    assert_eq!(channel.read(10).unwrap(), b"");
    assert_eq!(channel.read(10).unwrap(), b"hello");
    assert!(matches!(channel.read(10), Err(Error::Usb(_))));
    assert!(matches!(channel.read(10), Err(Error::State(_))));
}

#[test]
fn short_write_is_not_retried_and_close_still_happens() {
    let transport = Arc::new(FakeTransport { short: true, ..FakeTransport::default() });
    let mut channel = UsbChannel::new(transport.clone(), 7, 1000).unwrap();
    channel.open().unwrap();
    assert!(matches!(channel.write(&[b'x'; 8192], 3000), Err(Error::ShortWrite { transferred: 3, expected: 4095 })));
    assert!(matches!(channel.write(b"retry", 3000), Err(Error::State(_))));
    channel.close().unwrap();
    assert_eq!(transport.writes.lock().unwrap().len(), 1);
    assert_eq!(*transport.controls.lock().unwrap(), [(0x41, 7), (0x42, 7)]);
}

#[test]
fn argument_limits() {
    let transport: Arc<dyn Transport> = Arc::new(FakeTransport::default());
    assert!(UsbChannel::new(transport.clone(), 12, 1000).is_err());
    assert!(UsbChannel::new(transport.clone(), 0, 0).is_err());
    let mut channel = UsbChannel::new(transport, 0, 1000).unwrap();
    assert!(matches!(channel.write(b"x", 1000), Err(Error::State(_))));
    channel.open().unwrap();
    assert!(matches!(channel.write(b"x", 0), Err(Error::Invalid(_))));
    assert!(!channel.is_failed());
}

/// Endless frames for channel 0, counting how many chunks the reader took.
struct FakeStream {
    taken: Arc<AtomicUsize>,
    limit: usize,
    failure: Option<Error>,
}

impl ContinuousSource for FakeStream {
    fn next_chunk(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>> {
        let taken = self.taken.fetch_add(1, Ordering::SeqCst);
        if taken >= self.limit {
            return match self.failure.take() {
                Some(error) => Err(error),
                None => {
                    std::thread::sleep(Duration::from_millis(1));
                    Ok(None)
                }
            };
        }
        Ok(Some(inbound(1, &[taken as u8; 4])))
    }

    fn shutdown(self: Box<Self>) {}
}

#[test]
fn continuous_reader_drains_the_endpoint_while_the_consumer_is_busy() {
    let taken = Arc::new(AtomicUsize::new(0));
    let stream = FakeStream { taken: taken.clone(), limit: 200, failure: None };
    let transport = Arc::new(FakeTransport { stream: Mutex::new(Some(stream)), ..FakeTransport::default() });
    let mut channel = UsbChannel::new(transport.clone(), 0, 1000).unwrap();
    channel.open().unwrap();
    assert!(channel.is_continuous());
    let deadline = Instant::now() + Duration::from_secs(5);
    while taken.load(Ordering::SeqCst) < 200 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(taken.load(Ordering::SeqCst) >= 200, "the reader kept reading without a consumer");
    let data = channel.read(100).unwrap();
    assert_eq!(data.len(), 200 * 4);
    assert_eq!(&data[..8], &[0, 0, 0, 0, 1, 1, 1, 1]);
    assert_eq!(channel.read(20).unwrap(), b"");
    channel.close().unwrap();
    assert_eq!(*transport.controls.lock().unwrap(), [(0x41, 0), (0x42, 0)]);
}

#[test]
fn continuous_reader_reports_a_transport_error_after_the_data_before_it() {
    let stream = FakeStream {
        taken: Arc::new(AtomicUsize::new(0)),
        limit: 2,
        failure: Some(UsbError::new("bulk transfer", LIBUSB_ERROR_NO_DEVICE).into()),
    };
    let transport = Arc::new(FakeTransport { stream: Mutex::new(Some(stream)), ..FakeTransport::default() });
    let mut channel = UsbChannel::new(transport, 0, 1000).unwrap();
    channel.open().unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(channel.read(100).unwrap().len(), 8);
    assert!(matches!(channel.read(100), Err(Error::Usb(error)) if error.code == LIBUSB_ERROR_NO_DEVICE));
    assert!(channel.is_failed());
    channel.close().unwrap();
}
