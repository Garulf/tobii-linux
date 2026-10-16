//! One relayed TCP connection: buffered writes that never block the service, the relay frames
//! of the bridge, and the peer as the Python service logged it.

use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::rc::Rc;

use tobii_service::ServiceError;
use tobii_service::prp_service::{FRAME_MESSAGE, RELAY_HEADER_SIZE, decode_frame_header, encode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Notify, mpsc};

/// Why a relayed connection ended.
#[derive(Debug)]
pub enum Ended {
    /// The peer or the service closed it, or it failed at the transport level.
    Closed,
    /// It sent something the service refuses; logged as a warning.
    Refused(String),
}

impl From<std::io::Error> for Ended {
    fn from(_: std::io::Error) -> Self {
        Ended::Closed
    }
}

impl From<ServiceError> for Ended {
    fn from(error: ServiceError) -> Self {
        Ended::Refused(error.to_string())
    }
}

pub type Outcome<T = ()> = Result<T, Ended>;

pub fn refused<T>(message: impl Into<String>) -> Outcome<T> {
    Err(Ended::Refused(message.into()))
}

/// The peer address as Python prints `writer.get_extra_info("peername")`.
pub fn peer_repr(peer: Option<SocketAddr>) -> String {
    match peer {
        Some(SocketAddr::V4(address)) => format!("('{}', {})", address.ip(), address.port()),
        Some(SocketAddr::V6(address)) => {
            format!("('{}', {}, {}, {})", address.ip(), address.port(), address.flowinfo(), address.scope_id())
        }
        None => "None".into(),
    }
}

#[derive(Debug)]
struct Shared {
    outgoing: RefCell<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    pending: Cell<usize>,
    closing: Cell<bool>,
    closed: Notify,
}

/// The write side of a connection, like an asyncio stream writer.
///
/// Writes are queued and sent by a task of their own; `pending` counts the bytes not yet
/// handed to the socket. `close` sends what is queued, then shuts the connection down, and
/// wakes everything waiting in `closed` (the read side stops then too).
#[derive(Debug, Clone)]
pub struct Link {
    shared: Rc<Shared>,
}

impl Link {
    pub fn spawn(mut writer: OwnedWriteHalf) -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Vec<u8>>();
        let shared = Rc::new(Shared {
            outgoing: RefCell::new(Some(sender)),
            pending: Cell::new(0),
            closing: Cell::new(false),
            closed: Notify::new(),
        });
        let link = Self { shared };
        let task = link.clone();
        tokio::task::spawn_local(async move {
            while let Some(data) = receiver.recv().await {
                let written = writer.write_all(&data).await;
                task.shared.pending.set(task.shared.pending.get().saturating_sub(data.len()));
                if written.is_err() {
                    task.close();
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });
        link
    }

    pub fn write(&self, data: Vec<u8>) {
        if self.is_closing() {
            return;
        }
        if let Some(sender) = self.shared.outgoing.borrow().as_ref() {
            self.shared.pending.set(self.shared.pending.get() + data.len());
            let _ = sender.send(data);
        }
    }

    /// One relay frame.
    pub fn frame(&self, kind: u32, payload: &[u8]) {
        match encode_frame(kind, payload) {
            Ok(frame) => self.write(frame),
            Err(error) => log::warn!("dropping a relay frame: {error}"),
        }
    }

    /// A relayed pipe message.
    pub fn message(&self, message: &[u8]) {
        self.frame(FRAME_MESSAGE, message);
    }

    pub fn pending(&self) -> usize {
        self.shared.pending.get()
    }

    pub fn is_closing(&self) -> bool {
        self.shared.closing.get()
    }

    pub fn close(&self) {
        if self.shared.closing.replace(true) {
            return;
        }
        self.shared.outgoing.borrow_mut().take();
        self.shared.closed.notify_waiters();
    }

    pub async fn closed(&self) {
        let notified = self.shared.closed.notified();
        if self.is_closing() {
            return;
        }
        notified.await;
    }

    pub fn same(&self, other: &Link) -> bool {
        Rc::ptr_eq(&self.shared, &other.shared)
    }
}

/// The read side of a relayed connection; reads end when the link closes.
#[derive(Debug)]
pub struct Frames {
    reader: OwnedReadHalf,
    link: Link,
}

impl Frames {
    pub fn new(reader: OwnedReadHalf, link: Link) -> Self {
        Self { reader, link }
    }

    async fn read_frame(&mut self) -> Outcome<(u32, Vec<u8>)> {
        let mut header = [0u8; RELAY_HEADER_SIZE];
        self.reader.read_exact(&mut header).await?;
        let (kind, size) = decode_frame_header(&header)?;
        let mut payload = vec![0; size];
        self.reader.read_exact(&mut payload).await?;
        Ok((kind, payload))
    }

    /// The next relay frame, or `Closed` once the peer or the service closed the connection.
    pub async fn next(&mut self) -> Outcome<(u32, Vec<u8>)> {
        let link = self.link.clone();
        tokio::select! {
            frame = self.read_frame() => frame,
            () = link.closed() => Err(Ended::Closed),
        }
    }

    /// The next relayed pipe message.
    pub async fn message(&mut self, pipe: &str) -> Outcome<Vec<u8>> {
        let (kind, payload) = self.next().await?;
        if kind != FRAME_MESSAGE {
            return refused("expected a relayed pipe message");
        }
        log::debug!("{pipe} <- {}", hex(&payload));
        Ok(payload)
    }
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
