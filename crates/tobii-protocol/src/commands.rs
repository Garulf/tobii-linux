//! TTP request encoders for device startup and streams, recovered from the runtime.
//!
//! tools/ttp_commands_oracle.py compares every encoder with the original builder in
//! emulation. See docs/usb-streams.md.

use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;

use crate::error::{Result, fail};
use crate::fields::{uint32_field, wire};
use crate::numeric::cvttss2si;
use crate::ttp::{Field, Message, PAYLOAD_PREFIX, REQUEST_KIND, uint32_array};

pub const RESPONSE_KIND: u32 = 0x52;
pub const STREAM_DATA_KIND: u32 = 0x53;
pub const NOTIFICATION_KIND: u32 = 0x4E;
pub const FREQUENCY_PAIR_STRUCT: u32 = 0x2AF8;
pub const REALM_DIGEST_SIZE: usize = 16;

pub mod command {
    pub const PROTOCOL_UPGRADE: u32 = 1000;
    pub const ENUMERATE_STREAM_TYPES: u32 = 1200;
    pub const ENUMERATE_STREAM_TYPE_COLUMNS: u32 = 1210;
    pub const SUBSCRIBE_STREAM: u32 = 1220;
    pub const UNSUBSCRIBE_STREAM: u32 = 1230;
    pub const GET_DEVICE_PROPERTIES: u32 = 1330;
    pub const GET_CAPABILITIES: u32 = 1350;
    pub const GET_CAPABILITIES_ALL: u32 = 1351;
    pub const GET_TRACK_BOX: u32 = 1400;
    pub const GET_DEVICE_INFO: u32 = 1420;
    pub const GET_DISPLAY_AREA: u32 = 1430;
    pub const GET_DEVICE_STATUS: u32 = 1490;
    pub const UNLOCK_PPU: u32 = 1600;
    pub const GET_OUTPUT_FREQUENCY: u32 = 1610;
    pub const SET_OUTPUT_FREQUENCY: u32 = 1620;
    pub const ENUMERATE_OUTPUT_FREQUENCIES: u32 = 1630;
    pub const GET_FREQUENCIES: u32 = 1650;
    pub const SET_FREQUENCIES: u32 = 1660;
    pub const ENUMERATE_FREQUENCIES: u32 = 1670;
    pub const GET_DEVICE_NAME: u32 = 1700;
    pub const ENUMERATE_EXTENSIONS: u32 = 1810;
    pub const ENUMERATE_ENABLED_EXTENSIONS: u32 = 1820;
    pub const OPEN_REALM_SALT: u32 = 1900;
    pub const OPEN_REALM_RESPONSE: u32 = 1910;
    pub const OPEN_REALM_RESPONSE_WITH_DATA: u32 = 1911;
    pub const CLOSE_REALM: u32 = 1915;
    pub const GET_LOW_BLINK_MODE: u32 = 1920;
    pub const GET_ILLUMINATION_MODE: u32 = 2010;
    pub const SET_ILLUMINATION_MODE: u32 = 2020;
    pub const ENUMERATE_ILLUMINATION_MODES: u32 = 2030;
    pub const GET_GEOMETRY_MOUNTING: u32 = 2110;
    pub const HARDWARE_CONFIGURATION_GET: u32 = 2120;
    pub const POWER_SAVE_ACTIVATE: u32 = 3000;
    pub const POWER_SAVE_DEACTIVATE: u32 = 3010;
    pub const PAUSE_DEVICE: u32 = 3100;
    pub const GET_ENABLED_EYE: u32 = 3170;
}

pub const HEADER_ONLY_COMMANDS: [u32; 24] = [
    command::ENUMERATE_STREAM_TYPES,
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
    command::GET_LOW_BLINK_MODE,
    command::GET_ILLUMINATION_MODE,
    command::ENUMERATE_ILLUMINATION_MODES,
    command::GET_GEOMETRY_MOUNTING,
    command::HARDWARE_CONFIGURATION_GET,
    command::POWER_SAVE_ACTIVATE,
    command::POWER_SAVE_DEACTIVATE,
    command::GET_ENABLED_EYE,
];

/// Runtime-internal stream index used by the tracker subscribe slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum Stream {
    Gaze = 1,
    Image = 2,
    Presence = 5,
    AlgoDebug = 8,
    DeviceLog = 9,
    ImageCollection = 13,
    WearableFoveated = 14,
    ImageCollectionB = 15,
    IntermediateAlgo = 16,
    PrimaryCameraImage = 17,
    ImageCollectionC = 18,
    CustomData = 20,
    Sync = 21,
}

impl Stream {
    pub const ALL: [Stream; 13] = [
        Stream::Gaze,
        Stream::Image,
        Stream::Presence,
        Stream::AlgoDebug,
        Stream::DeviceLog,
        Stream::ImageCollection,
        Stream::WearableFoveated,
        Stream::ImageCollectionB,
        Stream::IntermediateAlgo,
        Stream::PrimaryCameraImage,
        Stream::ImageCollectionC,
        Stream::CustomData,
        Stream::Sync,
    ];

    pub fn from_index(index: u32) -> Option<Stream> {
        Self::ALL.into_iter().find(|stream| *stream as u32 == index)
    }

    pub fn index(self) -> u32 {
        self as u32
    }

    /// Lower-case name, as the Python logs print it.
    pub fn name(self) -> &'static str {
        match self {
            Stream::Gaze => "gaze",
            Stream::Image => "image",
            Stream::Presence => "presence",
            Stream::AlgoDebug => "algo_debug",
            Stream::DeviceLog => "device_log",
            Stream::ImageCollection => "image_collection",
            Stream::WearableFoveated => "wearable_foveated",
            Stream::ImageCollectionB => "image_collection_b",
            Stream::IntermediateAlgo => "intermediate_algo",
            Stream::PrimaryCameraImage => "primary_camera_image",
            Stream::ImageCollectionC => "image_collection_c",
            Stream::CustomData => "custom_data",
            Stream::Sync => "sync",
        }
    }

    pub fn wire_id(self) -> u32 {
        wire_stream_id(self.index()).expect("every stream has a wire id")
    }
}

/// Stream index to the id the runtime writes (table at 0x1402c4500).
pub const WIRE_STREAM_IDS: [(u32, u32); 21] = [
    (1, 0x500),
    (2, 0x501),
    (3, 0x502),
    (4, 0x503),
    (5, 0x504),
    (6, 0x505),
    (7, 0x50A),
    (8, 0x1770),
    (9, 0x1772),
    (10, 0x506),
    (11, 0x50D),
    (12, 0x507),
    (13, 0x508),
    (14, 0x509),
    (15, 0x50B),
    (16, 0x50C),
    (17, 0x50E),
    (18, 0x50F),
    (19, 0x511),
    (20, 0x1774),
    (21, 0x1771),
];

pub fn wire_stream_id(index: u32) -> Result<u32> {
    match WIRE_STREAM_IDS.iter().find(|(i, _)| *i == index) {
        Some((_, id)) => Ok(*id),
        None => fail("Unknown stream index"),
    }
}

pub fn int32_field(value: i32) -> Field {
    crate::fields::int32_field(value)
}

/// Float32 times 65536, truncated as the runtime's cvttss2si (0x1403b0160).
pub fn fixed16_field(value: f64) -> Field {
    let raw = cvttss2si(value as f32 * 65536.0);
    Field::new(wire::FIXED16, raw.to_be_bytes().to_vec())
}

pub fn string_field(text: &str) -> Result<Field> {
    if text.as_bytes().contains(&0) {
        return fail("String fields are NUL-terminated in the runtime");
    }
    Ok(counted_bytes(wire::STRING, text.as_bytes()))
}

pub fn blob_field(data: &[u8]) -> Field {
    counted_bytes(wire::BLOB, data)
}

fn counted_bytes(tag: u8, data: &[u8]) -> Field {
    let mut bytes = Vec::with_capacity(4 + data.len());
    bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    bytes.extend_from_slice(data);
    Field::new(tag, bytes)
}

/// Tag 5 header; the members follow as sibling items.
pub fn struct_header(member_count: u32, struct_type: u32) -> Result<Field> {
    if member_count > 0xFFF || struct_type > 0xFFFF {
        return fail("Struct member count or type out of range");
    }
    Ok(Field::new(wire::STRUCT, (member_count << 16 | struct_type).to_be_bytes().to_vec()))
}

/// Request kind 0x51; non-empty item data gets the two zero prefix bytes.
pub fn request(request_id: u32, command: u32, fields: &[Field]) -> Result<Message> {
    let mut encoded = Vec::new();
    for field in fields {
        field.encode_into(&mut encoded)?;
    }
    let payload = if encoded.is_empty() { encoded } else { [&PAYLOAD_PREFIX[..], &encoded].concat() };
    Ok(Message::new(REQUEST_KIND, request_id, 0, command, 0, payload))
}

pub fn header_only_request(request_id: u32, command: u32) -> Result<Message> {
    if !HEADER_ONLY_COMMANDS.contains(&command) {
        return fail("Command is not built by a header-only wrapper");
    }
    request(request_id, command, &[])
}

/// Built by 0x1402c71b0; the subscribe slots pass no columns.
pub fn subscribe_stream(request_id: u32, stream: u32, columns: &[u32]) -> Result<Message> {
    request(request_id, command::SUBSCRIBE_STREAM, &[uint32_field(wire_stream_id(stream)?), uint32_array(columns)])
}

pub fn unsubscribe_stream(request_id: u32, stream: u32) -> Result<Message> {
    request(request_id, command::UNSUBSCRIBE_STREAM, &[uint32_field(wire_stream_id(stream)?)])
}

pub fn enumerate_stream_type_columns(request_id: u32, stream: u32) -> Result<Message> {
    request(request_id, command::ENUMERATE_STREAM_TYPE_COLUMNS, &[uint32_field(wire_stream_id(stream)?)])
}

/// First realm step (0x1402c4600); it passes the array `[0]`.
pub fn open_realm_salt(request_id: u32, realm: u32) -> Message {
    request(request_id, command::OPEN_REALM_SALT, &[uint32_field(realm), uint32_array(&[0])]).expect("small request")
}

/// HMAC-MD5 of the device salt, as computed by 0x1402c0ef0.
pub fn realm_digest(key: &[u8], salt: &[u8]) -> [u8; REALM_DIGEST_SIZE] {
    let mut mac = <Hmac<Md5> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(salt);
    mac.finalize().into_bytes().into()
}

/// Second realm step: 1910 without extra data, 1911 with it. `challenge` holds the two
/// integers of the salt reply in reply order.
pub fn open_realm_response(request_id: u32, challenge: (u32, u32), digest: &[u8], extra: &[u8]) -> Result<Message> {
    if digest.len() != REALM_DIGEST_SIZE {
        return fail("Realm digest must be 16 bytes");
    }
    let mut fields = vec![uint32_field(challenge.0), uint32_field(challenge.1), blob_field(digest)];
    let mut command = command::OPEN_REALM_RESPONSE;
    if !extra.is_empty() {
        fields.push(blob_field(extra));
        command = command::OPEN_REALM_RESPONSE_WITH_DATA;
    }
    request(request_id, command, &fields)
}

pub fn close_realm(request_id: u32, realm: u32) -> Message {
    request(request_id, command::CLOSE_REALM, &[uint32_field(realm)]).expect("small request")
}

pub fn pause_device(request_id: u32) -> Message {
    request(request_id, command::PAUSE_DEVICE, &[uint32_field(1)]).expect("small request")
}

pub fn resume_device(request_id: u32) -> Message {
    request(request_id, command::PAUSE_DEVICE, &[uint32_field(0)]).expect("small request")
}

pub fn set_illumination_mode(request_id: u32, mode: &str) -> Result<Message> {
    request(request_id, command::SET_ILLUMINATION_MODE, &[string_field(mode)?])
}

/// Command 1620 from slot 0x248, which has no log name; the name is inferred.
pub fn set_output_frequency(request_id: u32, frequency: f64) -> Message {
    request(request_id, command::SET_OUTPUT_FREQUENCY, &[fixed16_field(frequency)]).expect("small request")
}

/// Struct 0x2af8 with two uint32 members in wire order (0x1402c5ac0).
pub fn set_frequencies(request_id: u32, first: u32, second: u32) -> Message {
    let header = struct_header(2, FREQUENCY_PAIR_STRUCT).expect("constant header");
    request(request_id, command::SET_FREQUENCIES, &[header, uint32_field(first), uint32_field(second)])
        .expect("small request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::hex;
    use crate::ttp::{Decoder, fields, hardware_info_request};

    const DIGEST: [u8; 16] =
        [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f];

    fn check(message: Message, original: &str) {
        assert_eq!(message.encode().unwrap(), hex(original));
    }

    #[test]
    fn emulated_builder_vectors() {
        check(
            header_only_request(1, command::GET_DEVICE_INFO).unwrap(),
            "0000005100000001000000000000058c0000000000000000",
        );
        check(
            subscribe_stream(1, Stream::Image.index(), &[]).unwrap(),
            "000000510000000100000000000004c400000000000000140000020000000400000501170000000400000000",
        );
        check(
            subscribe_stream(9, Stream::Image.index(), &[1, 2, 5, 0x1020_3040]).unwrap(),
            "000000510000000900000000000004c40000000000000024000002000000040000050117000000140000000400000001000000020000000510203040",
        );
        check(
            unsubscribe_stream(1, Stream::PrimaryCameraImage.index()).unwrap(),
            "000000510000000100000000000004ce000000000000000b000002000000040000050e",
        );
        check(
            enumerate_stream_type_columns(1, Stream::AlgoDebug.index()).unwrap(),
            "000000510000000100000000000004ba000000000000000b0000020000000400001770",
        );
        check(
            open_realm_salt(1, 0x2712),
            "0000005100000001000000000000076c0000000000000018000002000000040000271217000000080000000100000000",
        );
        check(close_realm(1, 0x2712), "0000005100000001000000000000077b000000000000000b0000020000000400002712");
        check(
            open_realm_response(5, (7, 0x1234_5678), &DIGEST, b"").unwrap(),
            "00000051000000050000000000000776000000000000002d0000020000000400000007020000000412345678150000001400000010101112131415161718191a1b1c1d1e1f",
        );
        check(
            open_realm_response(6, (7, 0x1234_5678), &DIGEST, b"\x01\x02\x03\x04").unwrap(),
            "00000051000000060000000000000777000000000000003a0000020000000400000007020000000412345678150000001400000010101112131415161718191a1b1c1d1e1f15000000080000000401020304",
        );
        check(pause_device(1), "00000051000000010000000000000c1c000000000000000b0000020000000400000001");
        check(resume_device(1), "00000051000000010000000000000c1c000000000000000b0000020000000400000000");
        check(
            set_illumination_mode(3, "default").unwrap(),
            "000000510000000300000000000007e400000000000000120000140000000b0000000764656661756c74",
        );
        check(
            set_output_frequency(4, 33.333),
            "00000051000000040000000000000654000000000000000b000003000000040021553f",
        );
        check(
            set_frequencies(8, 1, 2),
            "0000005100000008000000000000067c000000000000001d0000050000000400022af8020000000400000001020000000400000002",
        );
    }

    #[test]
    fn header_only_matches_hardware_info_request_and_decodes() {
        assert_eq!(header_only_request(7, command::HARDWARE_CONFIGURATION_GET).unwrap(), hardware_info_request(7));
        let message = open_realm_response(6, (1, 2), &DIGEST, b"xyz").unwrap();
        let decoded = Decoder::default().feed(&message.encode().unwrap()).unwrap();
        assert_eq!(decoded, std::slice::from_ref(&message));
        let tags: Vec<u8> = fields(&message.payload).unwrap().iter().map(|f| f.tag).collect();
        assert_eq!(tags, [wire::UINT32, wire::UINT32, wire::BLOB, wire::BLOB]);
    }

    #[test]
    fn wire_stream_table() {
        assert_eq!(Stream::Image.wire_id(), 0x501);
        assert_eq!(Stream::PrimaryCameraImage.wire_id(), 0x50E);
        let mut ids: Vec<u32> = WIRE_STREAM_IDS.iter().map(|(_, id)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 21);
        assert!(wire_stream_id(22).is_err());
    }

    #[test]
    fn fixed_point_truncates_like_cvttss2si() {
        assert_eq!(fixed16_field(-1.5).data, hex("fffe8000"));
        assert_eq!(fixed16_field(40000.0).data, hex("80000000"));
        assert_eq!(fixed16_field(f64::NAN).data, hex("80000000"));
    }

    #[test]
    fn realm_digest_is_hmac_md5() {
        let key: Vec<u8> = (0..16).collect();
        let salt: Vec<u8> = (100..116).collect();
        assert_eq!(realm_digest(&key, &salt).to_vec(), hex("0cead0b99c4ccb9a30e2c5b8fbd55dfa"));
    }

    #[test]
    fn rejections() {
        assert!(header_only_request(1, command::SUBSCRIBE_STREAM).is_err());
        assert!(open_realm_response(1, (0, 0), b"short", b"").is_err());
        assert!(set_illumination_mode(1, "a\0b").is_err());
        assert!(struct_header(0x1000, 1).is_err());
    }
}
