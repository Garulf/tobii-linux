//! What the Linux PRP service answers: relay framing to the Windows pipe bridge, the
//! connect response, property values and refusals, and the user presence property.
//!
//! A Windows bridge inside the Wine prefix (tools/prp_bridge) owns the named pipes and
//! forwards each pipe connection over its own loopback TCP connection. The asyncio service
//! that holds those connections stays in `tobii_linux.prp_service`; this module holds its
//! messages and decisions. Relay framing and real-service comparison: docs/prp-bridge.md,
//! docs/gaze-host.md and docs/service-behaviour.md.

use tobii_protocol::gaze::{PRESENCE_PRESENT, PROPERTY_USER_PRESENCE, prp_presence};

use crate::error::{Result, ServiceError, prp};
use crate::prp::{HEAD_POSE, encode_envelope, le_u32, push_u32};
use crate::prp_session::{
    COMMAND_CONNECT, COMMAND_STREAM_START, COMMAND_STREAM_STOP, COMMAND_TIMESYNC, ConnectResponse,
    OPERATION_COMMAND_REQUEST, OPERATION_COMMAND_RESPONSE, PROPERTY_DISPLAY_AREA, PROPERTY_DISPLAY_ID,
    PropertyResponse, STATUS_NOT_SUPPORTED, STATUS_OK, display_area_value, encode_property_notification, floats_value,
    string_value,
};
use crate::prp_streams::{PRP_EYE_OPENNESS, PRP_GAZE_DATA, PRP_GAZE_ORIGIN, PRP_GAZE_POINT, PRP_RESPONSIVE_GAZE_POINT};

pub const RELAY_VERSION: u32 = 1;
pub const RELAY_HEADER_SIZE: usize = 8;
pub const FRAME_BRIDGE_HELLO: u32 = 1;
pub const FRAME_PIPES: u32 = 2;
pub const FRAME_PIPE_OPEN: u32 = 3;
pub const FRAME_REPLY_PIPE: u32 = 4;
pub const FRAME_MESSAGE: u32 = 5;
pub const FRAME_DISPLAY_ID: u32 = 6;
pub const FRAME_QPC: u32 = 7;
pub const MAX_FRAME: usize = 1 << 20;
pub const DEFAULT_PORT: u16 = 7760;
pub const DEFAULT_NAME: &str = "ET5-LINUX";
pub const INTERNAL_URL_SCHEME: &str = "prp-internal://";

pub const STATUS_CONNECTION_FAILED: i32 = 3;
pub const STATUS_NOT_SUBSCRIBED: i32 = 5;
pub const STATUS_INVALID_PARAMETER: i32 = 8;
pub const STATUS_INSUFFICIENT_LICENSE: i32 = 17;
pub const GAZE_STREAMS: [u32; 3] = [PRP_GAZE_POINT, PRP_RESPONSIVE_GAZE_POINT, PRP_GAZE_ORIGIN];
/// The real service refused these with INSUFFICIENT_LICENSE to a client without license keys.
pub const LICENSED_STREAMS: [u32; 2] = [PRP_GAZE_DATA, PRP_EYE_OPENNESS];
/// Stream list of the real service's connect response, without the streams this service lacks.
pub const GAZE_SERVICE_STREAMS: [u32; 5] =
    [PRP_GAZE_DATA, PRP_GAZE_POINT, PRP_RESPONSIVE_GAZE_POINT, PRP_GAZE_ORIGIN, HEAD_POSE];
/// Generic error body: four zero words satisfy every fixed-size response branch of the
/// client decoder except connect (0x180055780, operation 10).
pub const ERROR_BODY_WORDS: usize = 4;
pub const PROPERTY_MODEL: u32 = 8;
/// Power save active (2), device paused (4), exclusive mode (5), calibration active (7) and
/// 0x10: the real service answered subscribe with 0 (and get, where tried: 2, 4, 5). The
/// game layer drops its notification subscriptions when one of them is refused, and its
/// reconnect then stalls re-subscribing the refused one.
pub const PROPERTY_ZERO_WORDS: [u32; 5] = [2, 4, 5, 7, 0x10];
pub const PROPERTY_CALIBRATION_ID: u32 = 6;
pub const PROPERTY_TRACK_BOX: u32 = 0xF;
pub const PROPERTY_FIELD_OF_USE: u32 = 0x11;
pub const PROPERTY_FAULTS: u32 = 0x15;
pub const PROPERTY_WARNINGS: u32 = 0x16;
pub const PROPERTY_LENS_CONFIGURATION: u32 = 0xC;
/// Subscribe and unsubscribe answers of the real service (live, Windows,
/// evidence/private/robust-live/subscribe-all.jsonl): these are refused with
/// INSUFFICIENT_LICENSE, 3, 8 and 9 with NOT_SUPPORTED, and unsubscribing anything not
/// subscribed gives NOT_SUBSCRIBED.
pub const LICENSED_SUBSCRIPTIONS: [u32; 8] = [0xA, 0xC, 0xD, 0xE, 0x13, 0x14, 0x17, 0x18];
pub const SUBSCRIBE_ONLY_PROPERTIES: [u32; 6] = [
    PROPERTY_USER_PRESENCE,
    PROPERTY_CALIBRATION_ID,
    PROPERTY_TRACK_BOX,
    PROPERTY_FIELD_OF_USE,
    PROPERTY_FAULTS,
    PROPERTY_WARNINGS,
];
/// Track box corners in mm as the reference tracker reported them: front plane at 450 mm,
/// back plane at 900 mm (property 0xF, live). The device getter 1400 is not queried.
pub const TRACK_BOX_MM: [f64; 24] = [
    125.0, 100.0, 450.0, -125.0, 100.0, 450.0, -125.0, -100.0, 450.0, 125.0, -100.0, 450.0, 250.0, 200.0, 900.0,
    -250.0, 200.0, 900.0, -250.0, -200.0, 900.0, 250.0, -200.0, 900.0,
];
pub const TRACKER_STATUS_TEXT: &str = "ok";
pub const MODEL_NAME: &str = "IS5_Large_Eyetracker_5";
/// Subscribing presence on the device emits an unknown record this much before the
/// tracker's current time when no presence data has arrived yet (0x140276dd0).
pub const PRESENCE_SUBSCRIBE_LEAD_US: i64 = 30000;
/// Real service notifications carry message tag 0, as its stream packages do.
pub const NOTIFICATION_TAG: u32 = 0;
/// Smallest value block the client decoder accepts per property id (0x180055780, operation
/// 6); ids not listed need [`DEFAULT_PROPERTY_BLOCK_SIZE`] bytes. Error replies carry a zero
/// block of this size, as the real service did for the properties it refused.
pub const PROPERTY_BLOCK_SIZES: [(u32, usize); 11] = [
    (0, 4),
    (1, 0x28),
    (0xA, 0x1C),
    (0xB, 0x10),
    (0xC, 0x18),
    (0xD, 0xC),
    (0xF, 0x64),
    (0x13, 0x488),
    (0x14, 0x10),
    (0x17, 0x34),
    (0x18, 0x10),
];
pub const DEFAULT_PROPERTY_BLOCK_SIZE: usize = 8;

fn float_word(value: f32) -> i32 {
    value.to_bits() as i32
}

/// Connect response modelled on the real ET5 service's reply (albedo, build 1.37.4.0).
///
/// Only the license-result count, firmware flag and stream list are known to be checked
/// by device_create. The other values are copied from the real reply so later queries
/// see a plausible device; their meaning is not established.
pub fn device_connect_response(serial: &str, license_count: usize, streams: &[i32]) -> ConnectResponse {
    let strings = [serial, "IS5_Large_Eyetracker_5", "IS5", "tobii-linux", "Peripheral", "IS5LEYETRACKER5", ""];
    ConnectResponse {
        build_version: "1.37.4.0_tobii_linux".into(),
        list_a: vec![8, 12, 22, 21, 1, 15, 18, 2, 5, 4, 9, 16, 7, 6, 19, 17],
        list_b: vec![1, 18, 2, 4, 16],
        list_c: vec![0, 1, 2, 0, 4, 5, 6, 7, 0, 0, 0, 11, 0, 0, 0, 15, 16, 17, 18, 0, 0, 21, 22, 0, 0],
        list_d: vec![9],
        streams: streams.to_vec(),
        supported_commands: [COMMAND_CONNECT, COMMAND_STREAM_START, COMMAND_STREAM_STOP, COMMAND_TIMESYNC]
            .map(|command| command as i32)
            .to_vec(),
        groups: vec![(2, vec![17, 18])],
        strings: strings.map(String::from).to_vec(),
        words: vec![
            2,
            float_word(184.0),
            float_word(20.0),
            0,
            float_word(-0.16),
            float_word(13.85),
            0,
            float_word(5.38),
            float_word(9.86),
        ],
        license_results: vec![0; license_count],
        mode: 2,
        ..ConnectResponse::default()
    }
}

fn region_size_word() -> Vec<u8> {
    4u32.to_le_bytes().to_vec()
}

pub fn word_value(value: u32) -> (Vec<u8>, Vec<u8>) {
    ([8u32.to_le_bytes(), value.to_le_bytes()].concat(), region_size_word())
}

/// Property 0xB: time of the last presence change (high, low word), then the wire value.
///
/// The real service answered two subscribes minutes apart with the same timestamp, so the
/// time is that of the last change, not of the request.
pub fn presence_value(value: u32, since_us: i64) -> (Vec<u8>, Vec<u8>) {
    let since = since_us as u64;
    let mut block = Vec::with_capacity(16);
    for word in [0x10, (since >> 32) as u32, since as u32, value] {
        push_u32(&mut block, word);
    }
    (block, region_size_word())
}

fn property_block_size(property_id: u32) -> usize {
    PROPERTY_BLOCK_SIZES
        .iter()
        .find(|(id, _)| *id == property_id)
        .map_or(DEFAULT_PROPERTY_BLOCK_SIZE, |(_, size)| *size)
}

/// Refusal with a zero value block, so the client decoder still accepts the packet.
pub fn property_error(tag: u32, property_id: u32, status: i32) -> PropertyResponse {
    let size = property_block_size(property_id);
    let mut block = (size as u32).to_le_bytes().to_vec();
    block.resize(size, 0);
    PropertyResponse { tag, status, property_id, block, region: region_size_word() }
}

/// Refused command 5, carrying the empty value the real service sent for that property.
pub fn subscribe_error(tag: u32, property_id: u32, status: i32) -> PropertyResponse {
    let response = |(block, region)| PropertyResponse { tag, status, property_id, block, region };
    match property_id {
        PROPERTY_MODEL => response(string_value("").expect("empty text fits")),
        PROPERTY_LENS_CONFIGURATION => {
            let block = [0x18u32, 0, 4, 0, 4, 0].iter().flat_map(|word| word.to_le_bytes()).collect();
            response((block, region_size_word()))
        }
        _ => property_error(tag, property_id, status),
    }
}

// Relay framing ------------------------------------------------------------------------

fn relay_frame_limit<T>() -> Result<T> {
    prp("relay frame exceeds the limit")
}

pub fn encode_frame(kind: u32, payload: &[u8]) -> Result<Vec<u8>> {
    if payload.len() > MAX_FRAME {
        return relay_frame_limit();
    }
    let mut frame = Vec::with_capacity(RELAY_HEADER_SIZE + payload.len());
    push_u32(&mut frame, kind);
    push_u32(&mut frame, payload.len() as u32);
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Kind and payload size of a relay frame header.
pub fn decode_frame_header(header: &[u8]) -> Result<(u32, usize)> {
    if header.len() != RELAY_HEADER_SIZE {
        return Err(ServiceError::Truncated(format!("unpack requires a buffer of {RELAY_HEADER_SIZE} bytes")));
    }
    let size = le_u32(header, 4) as usize;
    if size > MAX_FRAME {
        return relay_frame_limit();
    }
    Ok((le_u32(header, 0), size))
}

pub fn encode_names<S: AsRef<str>>(names: &[S]) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    for name in names {
        let name = name.as_ref();
        if !name.is_ascii() {
            return prp("relay names must be ASCII");
        }
        payload.extend_from_slice(name.as_bytes());
        payload.push(0);
    }
    Ok(payload)
}

pub fn decode_names(payload: &[u8]) -> Result<Vec<String>> {
    if payload.last().is_some_and(|last| *last != 0) {
        return prp("unterminated relay name");
    }
    let Some((_, names)) = payload.split_last() else {
        return Ok(Vec::new());
    };
    names
        .split(|byte| *byte == 0)
        .map(|name| match std::str::from_utf8(name) {
            Ok(name) if name.is_ascii() => Ok(name.to_string()),
            _ => prp("relay names must be ASCII"),
        })
        .collect()
}

pub fn encode_pipe_open(client_pid: u32, pipe: &str) -> Result<Vec<u8>> {
    let mut payload = [RELAY_VERSION.to_le_bytes(), client_pid.to_le_bytes()].concat();
    payload.extend_from_slice(&encode_names(&[pipe])?);
    Ok(payload)
}

pub fn decode_pipe_open(payload: &[u8]) -> Result<(u32, String)> {
    if payload.len() < 9 {
        return prp("truncated pipe open frame");
    }
    let (version, client_pid) = (le_u32(payload, 0), le_u32(payload, 4));
    let mut names = decode_names(&payload[8..])?;
    if version != RELAY_VERSION || names.len() != 1 {
        return prp("unsupported pipe open frame");
    }
    Ok((client_pid, names.remove(0)))
}

// Command handling -----------------------------------------------------------------------

pub fn packet_operation(packet: &[u8]) -> Result<u32> {
    let payload = crate::prp::decode_envelope(packet, crate::prp::DEFAULT_MAX_PAYLOAD)?;
    if payload.len() < 12 {
        return prp("truncated PRP header");
    }
    Ok(le_u32(payload, 8))
}

/// Tag and command id of any operation 9 request, including commands without a codec.
pub fn request_header(packet: &[u8]) -> Result<(u32, u32)> {
    let payload = crate::prp::decode_envelope(packet, crate::prp::DEFAULT_MAX_PAYLOAD)?;
    if payload.len() < 16 {
        return prp("truncated PRP request");
    }
    let (size, tag, operation) = (le_u32(payload, 0) as usize, le_u32(payload, 4), le_u32(payload, 8));
    let command_block = if size == 12 { size + le_u32(payload, size) as usize } else { 0 };
    if operation != OPERATION_COMMAND_REQUEST || command_block == 0 || command_block + 8 > payload.len() {
        return prp("not a PRP command request");
    }
    let (block_size, command) = (le_u32(payload, command_block), le_u32(payload, command_block + 4));
    if block_size < 8 {
        return prp("PRP command block is too short");
    }
    Ok((tag, command))
}

/// Nonzero-status reply for commands this service does not implement.
pub fn error_response(tag: u32, command: u32, status: i32) -> Vec<u8> {
    let mut payload = Vec::new();
    for word in [12, tag, OPERATION_COMMAND_RESPONSE, 4, 12, status as u32, command, 4, (4 * ERROR_BODY_WORDS) as u32] {
        push_u32(&mut payload, word);
    }
    payload.resize(payload.len() + 4 * ERROR_BODY_WORDS - 4, 0);
    push_u32(&mut payload, 4);
    push_u32(&mut payload, 0);
    encode_envelope(&payload, crate::prp::DEFAULT_MAX_PAYLOAD).expect("error responses fit the default limit")
}

/// Time of the unknown record a presence subscription emits (0x140276dd0).
pub fn presence_subscribe_time(tracker_time_us: i64) -> i64 {
    if tracker_time_us >= PRESENCE_SUBSCRIBE_LEAD_US {
        tracker_time_us - PRESENCE_SUBSCRIBE_LEAD_US
    } else {
        tracker_time_us
    }
}

/// Property 0xB as the service caches it (0x1400fd220, cache 0x14010e930).
///
/// Every platmod presence record replaces the cached property record; it is pushed to
/// subscribed clients unless it is the first record since the cache was cleared. There is
/// no comparison with the previous value: the device only sends changes (and its current
/// value when subscribed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UserPresence {
    pub value: Option<u32>,
    pub since_us: i64,
}

impl UserPresence {
    /// Forget the record, as the service does when a device is added again (0x14010e8d0).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Cache one platmod record; true when the service would push it as a notification.
    pub fn record(&mut self, timestamp_us: i64, platmod_value: i64) -> bool {
        let notify = self.value.is_some();
        self.value = Some(prp_presence(platmod_value));
        self.since_us = timestamp_us;
        notify
    }

    pub fn present(&self) -> bool {
        self.value == Some(PRESENCE_PRESENT)
    }

    pub fn value_block(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        match self.value {
            Some(value) => Ok(presence_value(value, self.since_us)),
            None => Err(ServiceError::Invalid("no user presence record".into())),
        }
    }

    pub fn notification(&self) -> Result<Vec<u8>> {
        let (block, region) = self.value_block()?;
        Ok(encode_property_notification(NOTIFICATION_TAG, PROPERTY_USER_PRESENCE, &block, &region))
    }
}

/// The service state a property answer reads.
#[derive(Debug, Clone, Copy)]
pub struct PropertySource<'a> {
    /// Top-left, top-right and bottom-left display corners in mm, when configured.
    pub display_area: Option<&'a [f64]>,
    /// The configured display id, else the one the bridge reported.
    pub display_id: Option<&'a str>,
    pub calibration_id: u32,
    pub presence: &'a UserPresence,
    /// Field of use of the client's connect request; `None` without a subscription.
    pub field_of_use: Option<i32>,
}

impl PropertySource<'_> {
    fn value(&self, property_id: u32, subscribe: bool) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        if SUBSCRIBE_ONLY_PROPERTIES.contains(&property_id) && !subscribe {
            return self.get_value(property_id, false);
        }
        Ok(Some(match property_id {
            PROPERTY_CALIBRATION_ID => word_value(self.calibration_id),
            PROPERTY_TRACK_BOX => floats_value(&TRACK_BOX_MM),
            PROPERTY_FIELD_OF_USE => word_value(self.field_of_use.unwrap_or(0) as u32),
            PROPERTY_FAULTS | PROPERTY_WARNINGS => string_value(TRACKER_STATUS_TEXT)?,
            _ => return self.get_value(property_id, subscribe),
        }))
    }

    fn get_value(&self, property_id: u32, subscribe: bool) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        match property_id {
            PROPERTY_DISPLAY_AREA if self.display_area.is_some() => {
                display_area_value(self.display_area.expect("checked")).map(Some)
            }
            PROPERTY_DISPLAY_ID if self.display_id.is_some_and(|id| !id.is_empty()) => {
                string_value(self.display_id.expect("checked")).map(Some)
            }
            PROPERTY_MODEL if !subscribe => string_value(MODEL_NAME).map(Some),
            id if PROPERTY_ZERO_WORDS.contains(&id) => Ok(Some(word_value(0))),
            PROPERTY_USER_PRESENCE if subscribe && self.presence.value.is_some() => {
                self.presence.value_block().map(Some)
            }
            _ => Ok(None),
        }
    }
}

/// Value of a property for a get, or for a subscribe (command 5), or a refusal.
///
/// Served: display area and display id when configured, the model name, properties 2
/// (power save active), 4 (device paused) and 5 (exclusive mode), all 0 as the real service
/// sent them, and user presence for subscribes only (the real service refused the plain
/// get).
pub fn answer_property(
    source: &PropertySource,
    tag: u32,
    property_id: u32,
    subscribe: bool,
) -> Result<PropertyResponse> {
    if subscribe && LICENSED_SUBSCRIPTIONS.contains(&property_id) {
        return Ok(subscribe_error(tag, property_id, STATUS_INSUFFICIENT_LICENSE));
    }
    Ok(match source.value(property_id, subscribe)? {
        Some((block, region)) => PropertyResponse { tag, status: STATUS_OK, property_id, block, region },
        None if subscribe => subscribe_error(tag, property_id, STATUS_NOT_SUPPORTED),
        None => property_error(tag, property_id, STATUS_NOT_SUPPORTED),
    })
}

#[cfg(test)]
mod tests;
