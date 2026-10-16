//! The real device through `nusb` (usbfs on Linux).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use nusb::MaybeFuture;
use nusb::transfer::{Buffer, Bulk, Completion, ControlIn, ControlOut, ControlType, In, Out, Recipient, TransferError};

use crate::descriptors::{Endpoint, ProbedDevice, bulk_pair, configuration, parse_descriptors};
use crate::error::*;
use crate::transport::{ContinuousSource, Transport};
use crate::{MAX_TRANSFER, PID, VID};

const CONTROL_LIMIT: usize = 4096;
const STREAM_TRANSFER_SIZE: usize = 16384;
const STREAM_TRANSFERS_IN_FLIGHT: usize = 4;
const CANCEL_WAIT: Duration = Duration::from_secs(1);

fn transfer_code(error: TransferError) -> i32 {
    match error {
        TransferError::Cancelled => LIBUSB_ERROR_TIMEOUT,
        TransferError::Stall => LIBUSB_ERROR_PIPE,
        TransferError::Disconnected => LIBUSB_ERROR_NO_DEVICE,
        TransferError::Fault => LIBUSB_ERROR_IO,
        TransferError::InvalidArgument => LIBUSB_ERROR_INVALID_PARAM,
        TransferError::Unknown(_) => LIBUSB_ERROR_OTHER,
    }
}

fn os_code(error: &nusb::Error) -> i32 {
    match error.kind() {
        nusb::ErrorKind::Disconnected => LIBUSB_ERROR_NO_DEVICE,
        nusb::ErrorKind::Busy => LIBUSB_ERROR_BUSY,
        nusb::ErrorKind::PermissionDenied => LIBUSB_ERROR_ACCESS,
        nusb::ErrorKind::NotFound => LIBUSB_ERROR_NOT_FOUND,
        nusb::ErrorKind::Unsupported => LIBUSB_ERROR_NOT_SUPPORTED,
        _ => LIBUSB_ERROR_OTHER,
    }
}

fn os_error(operation: &str, error: &nusb::Error) -> Error {
    UsbError::new(operation, os_code(error)).into()
}

fn received(completion: &Completion) -> &[u8] {
    &completion.buffer[..completion.actual_len.min(completion.buffer.len())]
}

fn check_timeout(timeout_ms: u32) -> Result<Duration> {
    if !(1..=60_000).contains(&timeout_ms) {
        return Err(Error::invalid("timeout_ms must be between 1 and 60000"));
    }
    Ok(Duration::from_millis(u64::from(timeout_ms)))
}

/// Eye Tracker 5 devices by standard descriptors only, read from sysfs without opening them.
pub fn probe() -> Result<Vec<ProbedDevice>> {
    let devices = nusb::list_devices().wait().map_err(|e| os_error("enumerate", &e))?;
    let mut found = Vec::new();
    for info in devices.filter(|info| (info.vendor_id(), info.product_id()) == (VID, PID)) {
        let path = info.sysfs_path().join("descriptors");
        let bytes = std::fs::read(&path).map_err(|_| UsbError::new("device descriptor", LIBUSB_ERROR_IO))?;
        let (device, configurations) =
            parse_descriptors(&bytes).ok_or_else(|| UsbError::new("configuration descriptor", LIBUSB_ERROR_IO))?;
        found.push(ProbedDevice { bus: info.busnum(), address: info.device_address(), device, configurations });
    }
    Ok(found)
}

/// Opens the tracker at `bus`/`address` and claims interface 0 only.
///
/// Bus and address select a currently connected device, not a persistent identity. Nothing
/// is sent and the configuration is not changed.
pub fn open_transport(bus: u8, address: u8) -> Result<BulkTransport> {
    let devices = nusb::list_devices().wait().map_err(|e| os_error("enumerate", &e))?;
    let Some(info) = devices.into_iter().find(|info| (info.busnum(), info.device_address()) == (bus, address)) else {
        return Err(Error::invalid("Selected Eye Tracker 5 is not connected"));
    };
    if (info.vendor_id(), info.product_id()) != (VID, PID) {
        return Err(Error::invalid("Selected device is not Eye Tracker 5"));
    }
    let device = info.open().wait().map_err(|e| os_error("open", &e))?;
    let active =
        device.active_configuration().map_err(|_| UsbError::new("configuration descriptor", LIBUSB_ERROR_NOT_FOUND))?;
    let (incoming, outgoing) = bulk_pair(&configuration(&active).interfaces)?;
    let interface = device.claim_interface(0).wait().map_err(|e| os_error("claim interface 0", &e))?;
    let bulk_in = interface.endpoint::<Bulk, In>(incoming.address).map_err(|e| os_error("endpoint", &e))?;
    let bulk_out = interface.endpoint::<Bulk, Out>(outgoing.address).map_err(|e| os_error("endpoint", &e))?;
    Ok(BulkTransport {
        incoming,
        outgoing,
        interface: Mutex::new(Some(interface)),
        bulk_in: Arc::new(Mutex::new(Some(bulk_in))),
        bulk_out: Mutex::new(Some(bulk_out)),
        closed: Arc::new(AtomicBool::new(false)),
    })
}

type InEndpoint = nusb::Endpoint<Bulk, In>;
type OutEndpoint = nusb::Endpoint<Bulk, Out>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Bulk IN 0x83 and OUT 0x05 of interface 0, usable from two threads at once.
#[derive(Debug)]
pub struct BulkTransport {
    pub incoming: Endpoint,
    pub outgoing: Endpoint,
    interface: Mutex<Option<nusb::Interface>>,
    bulk_in: Arc<Mutex<Option<InEndpoint>>>,
    bulk_out: Mutex<Option<OutEndpoint>>,
    closed: Arc<AtomicBool>,
}

impl BulkTransport {
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Releases interface 0; a disconnected device is closed all the same. Idempotent.
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            lock(&self.bulk_in).take();
            lock(&self.bulk_out).take();
            lock(&self.interface).take();
        }
    }

    fn require_open(&self) -> Result<()> {
        if self.is_closed() { Err(Error::state("USB transport is closed")) } else { Ok(()) }
    }

    fn interface(&self) -> Result<nusb::Interface> {
        self.require_open()?;
        lock(&self.interface).clone().ok_or_else(|| Error::state("USB transport is closed"))
    }

    fn check_control(data_len: usize, timeout_ms: u32) -> Result<Duration> {
        let timeout = check_timeout(timeout_ms)?;
        if data_len > CONTROL_LIMIT {
            return Err(Error::invalid("Control transfer exceeds local 4096-byte limit"));
        }
        Ok(timeout)
    }
}

impl Drop for BulkTransport {
    fn drop(&mut self) {
        self.close();
    }
}

impl Transport for BulkTransport {
    fn read(&self, size: usize, timeout_ms: u32) -> Result<Vec<u8>> {
        if !(1..=MAX_TRANSFER).contains(&size) {
            return Err(Error::invalid("Invalid read size"));
        }
        let packet = usize::from(self.incoming.max_packet_size & 0x7FF);
        if packet == 0 || !size.is_multiple_of(packet) {
            return Err(Error::invalid("Read size must be a multiple of endpoint max packet size"));
        }
        self.require_open()?;
        let timeout = check_timeout(timeout_ms)?;
        let mut guard = lock(&self.bulk_in);
        let endpoint = guard.as_mut().ok_or_else(|| Error::state("The IN endpoint streams to a channel reader"))?;
        if endpoint.pending() > 0 {
            return Err(Error::state("Cancelled IN transfers have not returned yet"));
        }
        let completion = endpoint.transfer_blocking(Buffer::new(size), timeout);
        let data = received(&completion).to_vec();
        match completion.status {
            Ok(()) => Ok(data),
            Err(error) => Err(UsbError {
                operation: "bulk transfer".into(),
                code: transfer_code(error),
                transferred: data.len(),
                partial_data: data,
            }
            .into()),
        }
    }

    fn write(&self, data: &[u8], timeout_ms: u32) -> Result<usize> {
        if !(1..=MAX_TRANSFER).contains(&data.len()) {
            return Err(Error::invalid("Invalid write size"));
        }
        self.require_open()?;
        let timeout = check_timeout(timeout_ms)?;
        let mut guard = lock(&self.bulk_out);
        let endpoint = guard.as_mut().ok_or_else(|| Error::state("USB transport is closed"))?;
        let completion = endpoint.transfer_blocking(Buffer::from(data.to_vec()), timeout);
        match completion.status {
            Ok(()) => Ok(completion.actual_len),
            Err(error) => Err(UsbError {
                operation: "bulk transfer".into(),
                code: transfer_code(error),
                transferred: completion.actual_len,
                partial_data: Vec::new(),
            }
            .into()),
        }
    }

    fn control_out(&self, request: u8, value: u16, index: u16, data: &[u8], timeout_ms: u32) -> Result<usize> {
        let timeout = Self::check_control(data.len(), timeout_ms)?;
        let setup = ControlOut {
            control_type: ControlType::Vendor,
            recipient: Recipient::Interface,
            request,
            value,
            index,
            data,
        };
        self.interface()?
            .control_out(setup, timeout)
            .wait()
            .map_err(|error| UsbError::new("control transfer", transfer_code(error)))?;
        Ok(data.len())
    }

    fn control_in(&self, request: u8, value: u16, index: u16, length: u16, timeout_ms: u32) -> Result<Vec<u8>> {
        let timeout = Self::check_control(usize::from(length), timeout_ms)?;
        let setup = ControlIn {
            control_type: ControlType::Vendor,
            recipient: Recipient::Interface,
            request,
            value,
            index,
            length,
        };
        Ok(self
            .interface()?
            .control_in(setup, timeout)
            .wait()
            .map_err(|error| UsbError::new("control transfer", transfer_code(error)))?)
    }

    fn continuous_source(&self) -> Option<Box<dyn ContinuousSource>> {
        if self.is_closed() {
            return None;
        }
        let endpoint = lock(&self.bulk_in).take()?;
        Some(Box::new(BulkInStream {
            endpoint: Some(endpoint),
            home: Arc::clone(&self.bulk_in),
            closed: Arc::clone(&self.closed),
        }))
    }
}

/// Keeps several IN transfers queued so the tracker's endpoint is drained continuously.
struct BulkInStream {
    endpoint: Option<InEndpoint>,
    home: Arc<Mutex<Option<InEndpoint>>>,
    closed: Arc<AtomicBool>,
}

impl BulkInStream {
    fn endpoint(&mut self) -> &mut InEndpoint {
        self.endpoint.as_mut().expect("endpoint present until shutdown")
    }
}

impl ContinuousSource for BulkInStream {
    fn next_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let endpoint = self.endpoint();
        while endpoint.pending() < STREAM_TRANSFERS_IN_FLIGHT {
            let buffer = endpoint.allocate(STREAM_TRANSFER_SIZE);
            endpoint.submit(buffer);
        }
        let Some(completion) = endpoint.wait_next_complete(timeout) else {
            return Ok(None);
        };
        let data = received(&completion).to_vec();
        let status = completion.status;
        let mut buffer = completion.buffer;
        buffer.clear();
        buffer.set_requested_len(STREAM_TRANSFER_SIZE);
        endpoint.submit(buffer);
        match status {
            Ok(()) => Ok(Some(data)),
            Err(error) => Err(UsbError {
                operation: "bulk transfer".into(),
                code: transfer_code(error),
                transferred: data.len(),
                partial_data: data,
            }
            .into()),
        }
    }

    fn shutdown(mut self: Box<Self>) {
        let mut endpoint = self.endpoint.take().expect("endpoint present until shutdown");
        endpoint.cancel_all();
        while endpoint.pending() > 0 && endpoint.wait_next_complete(CANCEL_WAIT).is_some() {}
        let mut home = lock(&self.home);
        if home.is_none() && !self.closed.load(Ordering::SeqCst) {
            *home = Some(endpoint);
        }
    }
}
