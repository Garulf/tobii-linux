//! Request/response and stream demultiplexing over one channel.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tobii_protocol::camera_info::parse_hardware_info;
use tobii_protocol::commands::{
    RESPONSE_KIND, STREAM_DATA_KIND, Stream, close_realm, command, header_only_request, open_realm_response,
    open_realm_salt, realm_digest, subscribe_stream,
};
use tobii_protocol::fields::{Value, response_status, status_name, values};
use tobii_protocol::frames::{ImageFrame, image_frame};
use tobii_protocol::ttp::{Decoder, hardware_info_request, protocol_upgrade};
use tobii_protocol::{Message, ProtocolError};

use crate::channel::Channel;
use crate::error::{DeviceError, Result};

pub const UNLOCK_REALM_KEY: &[u8] = b"aaaaaaaabbbbbbbbccccccccdddddddd";
pub const HEAD_POSE_REALM: u32 = 0x2712;
pub const HEAD_POSE_REALM_KEY: [u8; 16] =
    [0xb8, 0x8c, 0xc2, 0x94, 0x17, 0xeb, 0xf4, 0x8e, 0xbc, 0x59, 0xd1, 0xb5, 0x9d, 0x7a, 0xc5, 0x73];
pub const HEAD_POSE_REALM_EXTRA: [u8; 4] = [0x81, 0x6d, 0x4d, 0xf2];
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Extrinsic of the selected camera record: translation and three angles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraCalibration {
    pub translation: [f64; 3],
    pub angles: [f64; 3],
}

type Observer = Box<dyn FnMut(&Message) + Send>;

/// Request/response and stream demultiplexing over one channel.
///
/// Stream data that arrives while a reply is awaited is kept in `pending_streams`. A
/// session becomes `dead` once the device stopped answering or the channel failed; such a
/// session is not asked for anything more (a tracker that stalls stays silent).
pub struct TrackerSession<C> {
    channel: C,
    decoder: Decoder,
    request_id: u32,
    pub pending_streams: Vec<Message>,
    dead: bool,
    startup_replies: Vec<(u32, Message)>,
    observer: Option<Observer>,
}

impl<C> std::fmt::Debug for TrackerSession<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackerSession")
            .field("request_id", &self.request_id)
            .field("pending_streams", &self.pending_streams.len())
            .field("dead", &self.dead)
            .finish()
    }
}

fn milliseconds(duration: Duration) -> u32 {
    duration.as_millis().clamp(1, u128::from(u32::MAX)) as u32
}

impl<C: Channel> TrackerSession<C> {
    pub fn new(channel: C) -> Self {
        Self {
            channel,
            decoder: Decoder::default(),
            request_id: 0,
            pending_streams: Vec::new(),
            dead: false,
            startup_replies: Vec::new(),
            observer: None,
        }
    }

    pub fn channel(&self) -> &C {
        &self.channel
    }

    pub fn channel_mut(&mut self) -> &mut C {
        &mut self.channel
    }

    pub fn into_channel(self) -> C {
        self.channel
    }

    pub fn decoder_mut(&mut self) -> &mut Decoder {
        &mut self.decoder
    }

    pub fn is_dead(&self) -> bool {
        self.dead
    }

    pub fn set_dead(&mut self, dead: bool) {
        self.dead = dead;
    }

    pub fn request_id(&self) -> u32 {
        self.request_id
    }

    /// Replies to protocol upgrade (1000) and unlock (1600), for clients that start the same way.
    pub fn startup_replies(&self) -> &[(u32, Message)] {
        &self.startup_replies
    }

    /// Called with every decoded message, replies and stream data alike.
    pub fn set_observer(&mut self, observer: Option<Observer>) {
        self.observer = observer;
    }

    pub fn take_pending_streams(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.pending_streams)
    }

    pub fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>> {
        self.channel.read(timeout_ms).map_err(|error| {
            self.dead |= error.fatal;
            error.into()
        })
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.channel.write(data).map_err(|error| {
            self.dead |= error.fatal;
            error.into()
        })
    }

    /// Feeds received bytes to the session's decoder.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<Message>> {
        let messages = self.decoder.feed(data)?;
        if let Some(observer) = self.observer.as_mut() {
            messages.iter().for_each(observer);
        }
        Ok(messages)
    }

    /// One read, decoded; empty when nothing arrived.
    pub fn receive(&mut self, timeout_ms: u32) -> Result<Vec<Message>> {
        let data = self.read(timeout_ms)?;
        if data.is_empty() { Ok(Vec::new()) } else { self.decode(&data) }
    }

    pub fn next_id(&mut self) -> u32 {
        self.request_id = self.request_id.wrapping_add(1);
        self.request_id
    }

    /// The reply to `request_id` among received messages; stream data before it goes to
    /// `pending_streams` and other messages are dropped, as are messages after the reply.
    pub fn route(&mut self, messages: Vec<Message>, request_id: u32) -> Option<Message> {
        for message in messages {
            if message.kind == RESPONSE_KIND && message.request_id == request_id {
                return Some(message);
            }
            if message.kind == STREAM_DATA_KIND {
                self.pending_streams.push(message);
            }
        }
        None
    }

    /// Sends `message` and waits for the reply with its request id; stream data arriving
    /// meanwhile goes to `pending_streams`, other messages are dropped.
    pub fn request(&mut self, message: &Message, timeout: Duration) -> Result<Option<Message>> {
        self.write(&message.encode()?)?;
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let messages = self.receive(milliseconds(deadline - now))?;
            if let Some(reply) = self.route(messages, message.request_id) {
                return Ok(Some(reply));
            }
        }
    }

    /// The outcome of a call: a missing reply kills the session, a nonzero status fails.
    pub fn finish_call(&mut self, message: &Message, reply: Option<Message>) -> Result<Message> {
        let Some(reply) = reply else {
            self.dead = true;
            return Err(DeviceError::Timeout { command: message.command });
        };
        let status = response_status(&reply);
        if status != 0 {
            return Err(DeviceError::Failed { command: message.command, status, name: status_name(i64::from(status)) });
        }
        Ok(reply)
    }

    /// The device's reply whatever its status; `None` when none arrives in time.
    pub fn exchange(&mut self, build: impl FnOnce(u32) -> Message, timeout: Duration) -> Result<Option<Message>> {
        let message = build(self.next_id());
        self.request(&message, timeout)
    }

    /// The reply to an already built request, failing on timeout or a nonzero status.
    pub fn call_message(&mut self, message: &Message, timeout: Duration) -> Result<Message> {
        let reply = self.request(message, timeout)?;
        self.finish_call(message, reply)
    }

    pub fn call(&mut self, build: impl FnOnce(u32) -> Message) -> Result<Message> {
        let message = build(self.next_id());
        self.call_message(&message, REQUEST_TIMEOUT)
    }

    pub fn try_call(&mut self, build: impl FnOnce(u32) -> Result<Message, ProtocolError>) -> Result<Message> {
        let message = build(self.next_id())?;
        self.call_message(&message, REQUEST_TIMEOUT)
    }

    pub fn reply_values(reply: &Message) -> Result<Vec<Value>> {
        Ok(values(&reply.payload)?)
    }

    pub fn open_realm(&mut self, realm: u32, key: &[u8], extra: &[u8]) -> Result<()> {
        let salt_reply = Self::reply_values(&self.call(|id| open_realm_salt(id, realm))?)?;
        let integers: Vec<i64> = salt_reply.iter().filter_map(Value::as_int).collect();
        let salt = salt_reply.iter().find_map(|value| match value {
            Value::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        });
        let Some(salt) = salt else {
            return Err(DeviceError::UnexpectedReply("The realm salt reply carries no salt".into()));
        };
        if integers.len() < 2 {
            return Err(DeviceError::UnexpectedReply("The realm salt reply carries no challenge".into()));
        }
        let challenge = (unsigned(integers[0])?, unsigned(integers[1])?);
        let digest = realm_digest(key, &salt);
        self.try_call(|id| open_realm_response(id, challenge, &digest, extra))?;
        Ok(())
    }

    /// Protocol upgrade, then unlock, opening the realm it requests (0x1402bc2b0).
    pub fn initialize(&mut self) -> Result<()> {
        let upgrade = self.call(protocol_upgrade)?;
        let unlock_reply = self.try_call(|id| header_only_request(id, command::UNLOCK_PPU))?;
        let unlock: Vec<i64> = Self::reply_values(&unlock_reply)?.iter().filter_map(Value::as_int).collect();
        self.startup_replies = vec![(command::PROTOCOL_UPGRADE, upgrade), (command::UNLOCK_PPU, unlock_reply)];
        if unlock.len() >= 3 && unlock[0] != 0 && unlock[2] == 0 {
            self.open_realm(unsigned(unlock[1])?, UNLOCK_REALM_KEY, b"")?;
        }
        Ok(())
    }

    /// Realm 0x2712, hardware info (2120), close realm, as head-pose start does.
    pub fn camera_calibration(&mut self, camera: usize) -> Result<CameraCalibration> {
        self.open_realm(HEAD_POSE_REALM, &HEAD_POSE_REALM_KEY, &HEAD_POSE_REALM_EXTRA)?;
        let info = self.call(hardware_info_request).and_then(|reply| Ok(parse_hardware_info(&reply)?));
        self.call(|id| close_realm(id, HEAD_POSE_REALM))?;
        let info = info?;
        let count = info.cameras_0x8.len();
        let record = info.cameras_0x8.get(camera).ok_or(DeviceError::NoCamera { index: camera, count })?;
        Ok(CameraCalibration {
            translation: record.triple_0x10,
            angles: [record.doubles_0x28[0], record.doubles_0x28[1], record.doubles_0x28[2]],
        })
    }

    pub fn subscribe(&mut self, stream: Stream) -> Result<Message> {
        self.try_call(|id| subscribe_stream(id, stream.index(), &[]))
    }

    pub fn subscribe_images(&mut self) -> Result<()> {
        self.subscribe(Stream::Image).map(drop)
    }

    /// Image frames of stream 0x501 as they arrive, until the device stays silent for `timeout`.
    pub fn frames(&mut self, timeout: Duration) -> SessionFrames<'_, C> {
        let frames = ImageFrames::new(self.take_pending_streams(), timeout);
        SessionFrames { session: self, frames }
    }
}

fn unsigned(value: i64) -> Result<u32> {
    u32::try_from(value).map_err(|_| ProtocolError::new("Expected an unsigned 32-bit integer").into())
}

/// The state of [`TrackerSession::frames`], separate from the session it reads.
#[derive(Debug)]
pub struct ImageFrames {
    pending: VecDeque<Message>,
    timeout: Duration,
    deadline: Instant,
    image_stream: u32,
}

impl ImageFrames {
    pub fn new(pending: Vec<Message>, timeout: Duration) -> Self {
        Self {
            pending: pending.into(),
            timeout,
            deadline: Instant::now() + timeout,
            image_stream: Stream::Image.wire_id(),
        }
    }

    /// The next image frame among messages already received.
    pub fn pending_frame(&mut self) -> Result<Option<ImageFrame>> {
        while let Some(message) = self.pending.pop_front() {
            if message.kind == STREAM_DATA_KIND && message.command == self.image_stream {
                return Ok(Some(image_frame(&message)?));
            }
        }
        Ok(None)
    }

    /// Milliseconds to wait for more data; `None` once the device stayed silent too long.
    pub fn remaining_ms(&self) -> Option<u32> {
        let now = Instant::now();
        (now < self.deadline).then(|| milliseconds(self.deadline - now))
    }

    /// Bytes from one read; any data restarts the silence deadline.
    pub fn received<C: Channel>(&mut self, session: &mut TrackerSession<C>, data: &[u8]) -> Result<()> {
        if !data.is_empty() {
            self.pending = session.decode(data)?.into();
            self.deadline = Instant::now() + self.timeout;
        }
        Ok(())
    }

    pub fn next<C: Channel>(&mut self, session: &mut TrackerSession<C>) -> Result<Option<ImageFrame>> {
        loop {
            if let Some(frame) = self.pending_frame()? {
                return Ok(Some(frame));
            }
            let Some(remaining) = self.remaining_ms() else {
                return Ok(None);
            };
            let data = session.read(remaining)?;
            self.received(session, &data)?;
        }
    }
}

#[derive(Debug)]
pub struct SessionFrames<'a, C> {
    session: &'a mut TrackerSession<C>,
    frames: ImageFrames,
}

impl<C: Channel> Iterator for SessionFrames<'_, C> {
    type Item = Result<ImageFrame>;

    fn next(&mut self) -> Option<Self::Item> {
        self.frames.next(self.session).transpose()
    }
}
