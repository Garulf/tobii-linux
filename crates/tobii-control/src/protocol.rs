//! Control channel messages: JSON requests, replies and events, and the binary gaze frame.
//!
//! Every message is one relay frame (kind, length, payload) on the service's relay port.
//! The layout and the order of messages are in docs/control-channel.md.

use serde::{Deserialize, Serialize};
use tobii_gaze::device::Eye;
pub use tobii_service::prp_service::{
    FRAME_CONTROL_EVENT, FRAME_CONTROL_GAZE, FRAME_CONTROL_HELLO, FRAME_CONTROL_REPLY, FRAME_CONTROL_REQUEST,
};
use tobii_service::prp_service::{RELAY_HEADER_SIZE, decode_frame_header, encode_frame};

use crate::ControlError;

pub const PROTOCOL_VERSION: u32 = 1;
/// Size of a [`FRAME_CONTROL_GAZE`] payload.
pub const GAZE_FRAME_SIZE: usize = 28;

/// First frame of a connection: the protocol version and who is asking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub client: String,
}

impl Hello {
    pub fn new(client: &str) -> Self {
        Self { version: PROTOCOL_VERSION, client: client.into() }
    }
}

/// Which eyes the tracker follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackedEyes {
    Both,
    Left,
    Right,
}

impl TrackedEyes {
    /// The value Tobii keeps in the registry: 0 both, 1 left, 2 right.
    pub fn registry_value(self) -> u32 {
        match self {
            TrackedEyes::Both => 0,
            TrackedEyes::Left => 1,
            TrackedEyes::Right => 2,
        }
    }

    /// The eye the device is told to track.
    pub fn device_eye(self) -> Eye {
        match self {
            TrackedEyes::Both => Eye::Both,
            TrackedEyes::Left => Eye::Left,
            TrackedEyes::Right => Eye::Right,
        }
    }
}

/// What a client asks of the service; every request is answered by one [`Reply`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    Status,
    SetEnabled(bool),
    SwitchProfile {
        name: String,
    },
    SetTrackedEyes(TrackedEyes),
    /// Stop the tracker session and close USB until `Reacquire` or until this connection closes.
    Release,
    Reacquire,
    SubscribeGaze(bool),
}

/// The service's state as the tray and the setup app see it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// A tracker session runs; false while released.
    pub attached: bool,
    pub enabled: bool,
    /// A client holds the tracker through `Release`.
    pub released: bool,
    /// The service has a display area, configured or read from the tracker.
    pub display_area: bool,
    pub profile: Option<String>,
    pub calibration_matches_profile: bool,
}

/// The answer to the request with the same `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "ReplyFrame", try_from = "ReplyFrame")]
pub struct Reply {
    pub id: u64,
    pub result: Result<Status, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplyFrame {
    id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl From<Reply> for ReplyFrame {
    fn from(reply: Reply) -> Self {
        match reply.result {
            Ok(status) => Self { id: reply.id, status: Some(status), error: None },
            Err(error) => Self { id: reply.id, status: None, error: Some(error) },
        }
    }
}

impl TryFrom<ReplyFrame> for Reply {
    type Error = String;

    fn try_from(frame: ReplyFrame) -> Result<Self, String> {
        let result = match (frame.status, frame.error) {
            (Some(status), None) => Ok(status),
            (None, Some(error)) => Err(error),
            _ => return Err("a reply carries either a status or an error".into()),
        };
        Ok(Self { id: frame.id, result })
    }
}

/// What the service sends without being asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Status(Status),
}

/// One gaze point in normalized display coordinates (0,0 top left, 1,1 bottom right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GazeSample {
    /// Host clock of the service, microseconds.
    pub timestamp_us: i64,
    pub x: f64,
    pub y: f64,
    pub valid: bool,
}

impl GazeSample {
    /// Little-endian i64 timestamp, f64 x, f64 y, u32 validity.
    pub fn encode(&self) -> [u8; GAZE_FRAME_SIZE] {
        let mut bytes = [0u8; GAZE_FRAME_SIZE];
        bytes[..8].copy_from_slice(&self.timestamp_us.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.x.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.y.to_le_bytes());
        bytes[24..].copy_from_slice(&u32::from(self.valid).to_le_bytes());
        bytes
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ControlError> {
        let bytes: &[u8; GAZE_FRAME_SIZE] = payload.try_into().map_err(|_| {
            ControlError::Invalid(format!("a gaze frame has {GAZE_FRAME_SIZE} bytes, not {}", payload.len()))
        })?;
        let word = |at: usize| <[u8; 8]>::try_from(&bytes[at..at + 8]).expect("eight bytes");
        Ok(Self {
            timestamp_us: i64::from_le_bytes(word(0)),
            x: f64::from_le_bytes(word(8)),
            y: f64::from_le_bytes(word(16)),
            valid: u32::from_le_bytes(bytes[24..].try_into().expect("four bytes")) != 0,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RequestFrame {
    id: u64,
    request: Request,
}

/// The JSON payload of `value`.
pub fn payload(value: &impl Serialize) -> Result<Vec<u8>, ControlError> {
    Ok(serde_json::to_vec(value)?)
}

/// One relay frame of `kind` carrying `value` as JSON.
pub fn encode(kind: u32, value: &impl Serialize) -> Result<Vec<u8>, ControlError> {
    Ok(encode_frame(kind, &payload(value)?)?)
}

pub fn encode_request(id: u64, request: &Request) -> Result<Vec<u8>, ControlError> {
    encode(FRAME_CONTROL_REQUEST, &RequestFrame { id, request: request.clone() })
}

pub fn decode_request(payload: &[u8]) -> Result<(u64, Request), ControlError> {
    let frame: RequestFrame = serde_json::from_slice(payload)?;
    Ok((frame.id, frame.request))
}

/// The id of a request whose body could not be decoded, so the refusal can name it.
pub fn request_id(payload: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(payload).ok()?.get("id")?.as_u64()
}

pub fn decode_reply(payload: &[u8]) -> Result<Reply, ControlError> {
    Ok(serde_json::from_slice(payload)?)
}

pub fn decode_event(payload: &[u8]) -> Result<Event, ControlError> {
    Ok(serde_json::from_slice(payload)?)
}

pub fn decode_hello(payload: &[u8]) -> Result<Hello, ControlError> {
    Ok(serde_json::from_slice(payload)?)
}

/// Kind and payload of the complete relay frame at the start of `buffer`, with its length.
pub fn split_frame(buffer: &[u8]) -> Result<Option<(u32, Vec<u8>, usize)>, ControlError> {
    let Some(header) = buffer.get(..RELAY_HEADER_SIZE) else { return Ok(None) };
    let (kind, size) = decode_frame_header(header)?;
    let end = RELAY_HEADER_SIZE + size;
    Ok(buffer.get(RELAY_HEADER_SIZE..end).map(|payload| (kind, payload.to_vec(), end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only_frame(frame: &[u8], kind: u32) -> Vec<u8> {
        let (found, payload, used) = split_frame(frame).unwrap().unwrap();
        assert_eq!((found, used), (kind, frame.len()));
        payload
    }

    #[test]
    fn request_round_trip() {
        let requests = [
            Request::Status,
            Request::SetEnabled(false),
            Request::SwitchProfile { name: "Guest".into() },
            Request::SetTrackedEyes(TrackedEyes::Left),
            Request::Release,
            Request::Reacquire,
            Request::SubscribeGaze(true),
        ];
        for (id, request) in requests.into_iter().enumerate() {
            let frame = encode_request(id as u64, &request).unwrap();
            let payload = only_frame(&frame, FRAME_CONTROL_REQUEST);
            assert_eq!(decode_request(&payload).unwrap(), (id as u64, request));
        }
    }

    #[test]
    fn requests_use_the_documented_json() {
        let text = |id, request| String::from_utf8(payload(&RequestFrame { id, request }).unwrap()).unwrap();
        assert_eq!(text(1, Request::Status), r#"{"id":1,"request":"status"}"#);
        assert_eq!(text(2, Request::SetEnabled(false)), r#"{"id":2,"request":{"set_enabled":false}}"#);
        assert_eq!(
            text(3, Request::SwitchProfile { name: "Guest".into() }),
            r#"{"id":3,"request":{"switch_profile":{"name":"Guest"}}}"#
        );
        assert_eq!(
            text(4, Request::SetTrackedEyes(TrackedEyes::Right)),
            r#"{"id":4,"request":{"set_tracked_eyes":"right"}}"#
        );
        assert_eq!(request_id(br#"{"id":9,"request":"teleport"}"#), Some(9));
        assert!(decode_request(br#"{"id":9,"request":"teleport"}"#).is_err());
    }

    #[test]
    fn replies_carry_a_status_or_an_error() {
        let status = Status { attached: true, enabled: true, profile: Some("Default".into()), ..Status::default() };
        let ok = Reply { id: 5, result: Ok(status) };
        let text = String::from_utf8(payload(&ok).unwrap()).unwrap();
        assert_eq!(
            text,
            r#"{"id":5,"status":{"attached":true,"enabled":true,"released":false,"display_area":false,"profile":"Default","calibration_matches_profile":false}}"#
        );
        assert_eq!(decode_reply(text.as_bytes()).unwrap(), ok);
        let refused = Reply { id: 6, result: Err("unsupported".into()) };
        assert_eq!(payload(&refused).unwrap(), br#"{"id":6,"error":"unsupported"}"#);
        assert_eq!(decode_reply(br#"{"id":6,"error":"unsupported"}"#).unwrap(), refused);
        assert!(decode_reply(br#"{"id":7}"#).is_err());
    }

    #[test]
    fn events_and_hello_round_trip() {
        let event = Event::Status(Status { released: true, ..Status::default() });
        let frame = encode(FRAME_CONTROL_EVENT, &event).unwrap();
        assert_eq!(decode_event(&only_frame(&frame, FRAME_CONTROL_EVENT)).unwrap(), event);
        let hello = encode(FRAME_CONTROL_HELLO, &Hello::new("setup")).unwrap();
        assert_eq!(only_frame(&hello, FRAME_CONTROL_HELLO), br#"{"version":1,"client":"setup"}"#);
    }

    #[test]
    fn gaze_frame_is_28_bytes() {
        let sample = GazeSample { timestamp_us: -2, x: 0.25, y: 0.75, valid: true };
        let bytes = sample.encode();
        assert_eq!(bytes.len(), 28);
        assert_eq!(bytes[..8], (-2i64).to_le_bytes());
        assert_eq!(bytes[8..16], 0.25f64.to_le_bytes());
        assert_eq!(bytes[16..24], 0.75f64.to_le_bytes());
        assert_eq!(bytes[24..], 1u32.to_le_bytes());
        assert_eq!(GazeSample::decode(&bytes).unwrap(), sample);
        assert!(GazeSample::decode(&bytes[..27]).is_err());
    }

    #[test]
    fn tracked_eyes_values() {
        let values: Vec<(u32, Eye)> = [TrackedEyes::Both, TrackedEyes::Left, TrackedEyes::Right]
            .map(|eyes| (eyes.registry_value(), eyes.device_eye()))
            .to_vec();
        assert_eq!(values, [(0, Eye::Both), (1, Eye::Left), (2, Eye::Right)]);
    }

    #[test]
    fn partial_frames_wait_for_more() {
        let frame = encode_request(1, &Request::Status).unwrap();
        assert!(split_frame(&frame[..frame.len() - 1]).unwrap().is_none());
        assert!(split_frame(&frame[..3]).unwrap().is_none());
    }
}
