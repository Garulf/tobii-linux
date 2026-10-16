//! The game DLL's tet-tcp:// tracker route: the read-only command policy, the production
//! endpoint's answers from the device's own replies, and stream restamping.
//!
//! The game DLL's discovery (0x180058010) reports `tet-tcp://127.0.0.1` when a TCP connect to
//! 127.0.0.1:4455 succeeds. device_create then builds its in-process legacy TTP platform
//! module, whose tracker client opens channel 1 on port 4455 and speaks plain TTP framing
//! with no USB envelope. Timesync uses a UDP socket to port 4457 (channel 3), and channel
//! 4 is TCP 4459 (docs/tet-tcp.md).
//!
//! Nothing a game sends reaches the tracker through the production endpoint: startup
//! replies and getters are the device's own replies captured once per tracker session,
//! subscribe and unsubscribe of gaze (0x500) and presence (0x504) only change which
//! connections receive the service's stream, and everything else, including every write,
//! is refused with the device's unsupported-operation status. Stream messages keep their
//! bytes except column 1, the device timestamp, which becomes the game's
//! QueryPerformanceCounter microseconds: the module passes it through unchanged because
//! timesync never connects under Wine. The sockets stay in `tobii_linux.tet_tcp` and
//! `tobii_linux.tet_tcp_service`.

use std::collections::{BTreeSet, HashMap};

use tobii_protocol::Message;
use tobii_protocol::commands::{RESPONSE_KIND, command};
use tobii_protocol::fields::{Value, value, wire};
use tobii_protocol::frames::{COLUMN_MEMBERS, COLUMN_STRUCT, FRAME_STRUCT, struct_code, take_value};
use tobii_protocol::ttp::{Field, REQUEST_KIND, encode_payload, fields};

use crate::error::{Result, ServiceError};

pub const COMMAND_PORT: u16 = 4455;
pub const CHANNEL_PORTS: [(u16, u32); 3] = [(4455, 1), (4457, 3), (4459, 4)];
pub const TIMESYNC_UDP_PORT: u16 = 4457;
pub const SERVICE_UNLOCK_KEY: &[u8; 32] = b"aaaaaaaabbbbbbbbccccccccdddddddd";
pub const UNSUPPORTED_OPERATION: u32 = 0x2000_0501;
pub const DISPLAY_AREA_WRITABLE: u32 = 1460;
pub const PERSISTENT_FILE_ENUMERATE: u32 = 3060;
pub const PERSISTENT_FILE_READ: u32 = 3080;

/// The only requests the recording proxy forwards to a tracker: none of them writes
/// calibration, display area, settings or files.
pub const READ_COMMANDS: [u32; 33] = [
    command::PROTOCOL_UPGRADE,
    command::ENUMERATE_STREAM_TYPES,
    command::ENUMERATE_STREAM_TYPE_COLUMNS,
    command::SUBSCRIBE_STREAM,
    command::UNSUBSCRIBE_STREAM,
    command::GET_DEVICE_PROPERTIES,
    command::GET_CAPABILITIES,
    command::GET_CAPABILITIES_ALL,
    command::GET_TRACK_BOX,
    command::GET_DEVICE_INFO,
    command::GET_DISPLAY_AREA,
    command::GET_DEVICE_STATUS,
    command::UNLOCK_PPU,
    command::GET_OUTPUT_FREQUENCY,
    command::ENUMERATE_OUTPUT_FREQUENCIES,
    command::GET_FREQUENCIES,
    command::ENUMERATE_FREQUENCIES,
    command::GET_DEVICE_NAME,
    command::ENUMERATE_EXTENSIONS,
    command::ENUMERATE_ENABLED_EXTENSIONS,
    command::OPEN_REALM_SALT,
    command::OPEN_REALM_RESPONSE,
    command::OPEN_REALM_RESPONSE_WITH_DATA,
    command::CLOSE_REALM,
    command::GET_LOW_BLINK_MODE,
    command::GET_ILLUMINATION_MODE,
    command::ENUMERATE_ILLUMINATION_MODES,
    command::GET_GEOMETRY_MOUNTING,
    command::HARDWARE_CONFIGURATION_GET,
    command::GET_ENABLED_EYE,
    DISPLAY_AREA_WRITABLE,
    PERSISTENT_FILE_ENUMERATE,
    PERSISTENT_FILE_READ,
];

pub const GAZE_STREAM: u32 = 0x500;
pub const PRESENCE_STREAM: u32 = 0x504;
pub const STREAMS: [u32; 2] = [GAZE_STREAM, PRESENCE_STREAM];
/// Getters the legacy module and the game layer sent during device start (docs/tet-tcp.md).
pub const CAPTURED_GETTERS: [u32; 12] = [
    command::ENUMERATE_STREAM_TYPES,
    command::GET_DEVICE_PROPERTIES,
    command::GET_CAPABILITIES,
    command::GET_TRACK_BOX,
    command::GET_DEVICE_INFO,
    command::GET_DISPLAY_AREA,
    DISPLAY_AREA_WRITABLE,
    command::GET_DEVICE_STATUS,
    command::GET_FREQUENCIES,
    command::GET_DEVICE_NAME,
    command::GET_GEOMETRY_MOUNTING,
    PERSISTENT_FILE_ENUMERATE,
];
/// Device replies to subscribe and unsubscribe carry this second header word and no payload.
pub const REPLY_WORD2: u32 = 1;
/// Unsent bytes above which stream messages are dropped for a slow connection.
pub const MAX_PENDING_BYTES: usize = 64 * 1024;
pub const TIMESTAMP_COLUMN: u32 = 1;

/// Wire stream id of a SUBSCRIBE_STREAM or UNSUBSCRIBE_STREAM request, if readable.
pub fn subscribed_stream(message: &Message) -> Option<u32> {
    let items = fields(&message.payload).ok()?;
    let first = items.first().filter(|first| first.tag == wire::UINT32)?;
    match value(first).ok()? {
        Value::UInt(stream) => u32::try_from(stream).ok(),
        _ => None,
    }
}

/// Stream payload with the int64 of column 1 replaced; every other byte unchanged.
pub fn restamp(payload: &[u8], timestamp_us: i64) -> Result<Vec<u8>> {
    let mut items = fields(payload)?;
    let Some(first) = items.first() else {
        return Err(protocol("stream payload does not start with a column set"));
    };
    let (count, kind) = struct_code(first)?;
    if kind != FRAME_STRUCT {
        return Err(protocol("stream payload does not start with a column set"));
    }
    let mut index = 1;
    for _ in 0..count {
        if index + 2 >= items.len() || struct_code(&items[index])? != (COLUMN_MEMBERS, COLUMN_STRUCT) {
            return Err(protocol("malformed column set"));
        }
        let (column, data) = (&items[index + 1], &items[index + 2]);
        if column.data == TIMESTAMP_COLUMN.to_be_bytes() && data.tag == wire::INT64 {
            items[index + 2] = Field::new(wire::INT64, timestamp_us.to_be_bytes().to_vec());
            return Ok(encode_payload(&items)?);
        }
        index = take_value(&items, index + 2)?;
    }
    Err(protocol("stream message without a timestamp column"))
}

fn protocol(message: &str) -> ServiceError {
    ServiceError::Protocol(tobii_protocol::ProtocolError::new(message))
}

/// The cached device reply, answered under the request's id and command.
pub fn reply_to(message: &Message, cached: &Message) -> Message {
    Message::new(RESPONSE_KIND, message.request_id, cached.word2, message.command, cached.word4, cached.payload.clone())
}

pub fn refusal(message: &Message) -> Message {
    Message::new(RESPONSE_KIND, message.request_id, 0, message.command, UNSUPPORTED_OPERATION, Vec::new())
}

/// What the production endpoint does with one message from a game connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Not a request; nothing is sent.
    Ignore,
    /// The device's own reply to this getter.
    Reply(Message),
    /// Refused with the unsupported-operation status; `stream` is set for a subscription
    /// to a stream the service does not serve.
    Refuse { reply: Message, stream: Option<u32>, subscription: bool },
    /// The connection now receives (or no longer receives) `stream`.
    Subscribe { reply: Message, stream: u32, subscribe: bool },
}

/// The answer to one game request, given the device's captured replies by command and the
/// wire streams the service holds subscribed on the device.
pub fn answer(message: &Message, replies: &HashMap<u32, Message>, served: &BTreeSet<u32>) -> Answer {
    if message.kind != REQUEST_KIND {
        return Answer::Ignore;
    }
    let subscribe = message.command == command::SUBSCRIBE_STREAM;
    if subscribe || message.command == command::UNSUBSCRIBE_STREAM {
        let stream = subscribed_stream(message);
        return match stream.filter(|stream| STREAMS.contains(stream) && served.contains(stream)) {
            Some(stream) => Answer::Subscribe {
                reply: Message::new(RESPONSE_KIND, message.request_id, REPLY_WORD2, message.command, 0, Vec::new()),
                stream,
                subscribe,
            },
            None => Answer::Refuse { reply: refusal(message), stream, subscription: true },
        };
    }
    match replies.get(&message.command) {
        Some(cached) => Answer::Reply(reply_to(message, cached)),
        None => Answer::Refuse { reply: refusal(message), stream: None, subscription: false },
    }
}

#[cfg(test)]
mod tests;
