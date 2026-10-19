//! Borrowing the tracker from a running `tobii-linux serve` (docs/control-channel.md).
//!
//! The service holds the tracker's USB device, so before opening it the setup asks the
//! service over its control channel to release it, and keeps that connection open for the
//! whole flow. Closing the connection, also by a crash, makes the service reacquire. With no
//! service listening the setup opens USB directly, as before there was one.

use std::net::SocketAddr;
use std::time::Duration;

use tobii_control::{ControlClient, Request};

use crate::flow::FlowError;

/// How long connecting to the service may take; a refused connection fails at once.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// How long the service may take to close USB before it replies to the release.
pub const RELEASE_TIMEOUT: Duration = Duration::from_secs(15);
/// The client name the setup introduces itself with.
pub const CLIENT: &str = "setup";

/// Whether the setup asks a running service for the tracker, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceChoice {
    Ask(SocketAddr),
    Skip,
}

/// The control connection over which the service lent the tracker; dropping it gives the
/// tracker back.
#[derive(Debug)]
pub struct Handover {
    _client: ControlClient,
}

/// Asks the service to release the tracker; `None` when skipped or when nothing listens.
pub fn borrow_tracker(service: ServiceChoice) -> Result<Option<Handover>, FlowError> {
    let ServiceChoice::Ask(addr) = service else { return Ok(None) };
    let mut client = match ControlClient::connect(addr, CLIENT, CONNECT_TIMEOUT) {
        Ok(client) => client,
        Err(error) => {
            log::info!("no tobii-linux serve at {addr} ({error}); opening the tracker directly");
            return Ok(None);
        }
    };
    client.request(Request::Release, RELEASE_TIMEOUT).map_err(FlowError::Handover)?;
    log::info!("tobii-linux serve at {addr} released the tracker");
    Ok(Some(Handover { _client: client }))
}

/// Borrows the tracker, then opens it with `open`; when `open` fails the service gets the
/// tracker back at once.
pub fn open_borrowed<S>(
    service: ServiceChoice,
    open: impl FnOnce() -> Result<S, FlowError>,
) -> Result<(S, Option<Handover>), FlowError> {
    let handover = borrow_tracker(service)?;
    Ok((open()?, handover))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_and_silence_open_directly() {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        for service in [ServiceChoice::Skip, ServiceChoice::Ask(addr)] {
            let (opened, handover) = open_borrowed(service, || Ok::<_, FlowError>("usb")).unwrap();
            assert_eq!((opened, handover.is_none()), ("usb", true), "{service:?}");
        }
    }
}
