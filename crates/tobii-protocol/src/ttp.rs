//! Inner TTP framing: a six-word big-endian header and a typed payload.
//!
//! Derived from the installed platform runtime; see docs/usb-protocol.md. Unknown header
//! words and item tags are preserved.

use crate::error::{Result, fail};

pub const HEADER_LEN: usize = 24;
/// Local resource limit, not a claimed device limit.
pub const MAX_PAYLOAD: u32 = 4 * 1024 * 1024;
pub const PAYLOAD_PREFIX: [u8; 2] = [0, 0];
pub const UINT32_ARRAY_TAG: u8 = 0x17;
pub const HARDWARE_INFO_COMMAND: u32 = 0x848;
pub const HARDWARE_INFO_MIN_PROTOCOL: u32 = 0x10008;
pub const REQUEST_KIND: u32 = 0x51;
const PROTOCOL_UPGRADE_COMMAND: u32 = 1000;

pub(crate) fn be_u32(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(data[at..at + 4].try_into().expect("four bytes"))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Message {
    pub kind: u32,
    pub request_id: u32,
    pub word2: u32,
    pub command: u32,
    pub word4: u32,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn new(kind: u32, request_id: u32, word2: u32, command: u32, word4: u32, payload: Vec<u8>) -> Self {
        Self { kind, request_id, word2, command, word4, payload }
    }

    pub fn header_only(kind: u32, request_id: u32, command: u32) -> Self {
        Self::new(kind, request_id, 0, command, 0, Vec::new())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.encode_limited(MAX_PAYLOAD)
    }

    pub fn encode_limited(&self, max_payload: u32) -> Result<Vec<u8>> {
        if self.payload.len() as u64 > u64::from(max_payload) {
            return fail("Payload exceeds local size limit");
        }
        let mut data = Vec::with_capacity(HEADER_LEN + self.payload.len());
        for word in [self.kind, self.request_id, self.word2, self.command, self.word4, self.payload.len() as u32] {
            data.extend_from_slice(&word.to_be_bytes());
        }
        data.extend_from_slice(&self.payload);
        Ok(data)
    }
}

/// Incremental decoder, independent of how the transport chunks the stream.
///
/// On a framing error the decoder stays failed: discard it and reconnect. No
/// resynchronization marker has been established, so scanning for one would invent data.
#[derive(Debug, Clone)]
pub struct Decoder {
    max_payload: u32,
    pending: Vec<u8>,
    expected: usize,
    header: Option<[u32; 6]>,
    failed: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(MAX_PAYLOAD)
    }
}

impl Decoder {
    pub fn new(max_payload: u32) -> Self {
        Self { max_payload, pending: Vec::new(), expected: HEADER_LEN, header: None, failed: false }
    }

    pub fn max_payload(&self) -> u32 {
        self.max_payload
    }

    pub fn pending(&self) -> &[u8] {
        &self.pending
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<Message>> {
        if self.failed {
            return fail("Decoder failed; a new stream requires a new decoder");
        }
        let mut messages = Vec::new();
        let mut offset = 0;
        while offset < data.len() {
            let count = (self.expected - self.pending.len()).min(data.len() - offset);
            self.pending.extend_from_slice(&data[offset..offset + count]);
            offset += count;
            if self.pending.len() != self.expected {
                continue;
            }
            let header = match self.header {
                Some(header) => header,
                None => {
                    let header: [u32; 6] = std::array::from_fn(|i| be_u32(&self.pending, i * 4));
                    let length = header[5];
                    self.pending.clear();
                    if length > self.max_payload {
                        self.failed = true;
                        return fail("Declared payload exceeds local size limit");
                    }
                    self.header = Some(header);
                    self.expected = length as usize;
                    if length > 0 {
                        continue;
                    }
                    header
                }
            };
            let [kind, request_id, word2, command, word4, _] = header;
            messages.push(Message::new(kind, request_id, word2, command, word4, std::mem::take(&mut self.pending)));
            self.header = None;
            self.expected = HEADER_LEN;
        }
        Ok(messages)
    }

    pub fn finish(&self) -> Result<()> {
        if self.failed || !self.pending.is_empty() || self.header.is_some() {
            return fail("Stream ended with an invalid or incomplete message");
        }
        Ok(())
    }
}

/// One payload item: a tag byte, a big-endian length and the data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Field {
    pub tag: u8,
    pub data: Vec<u8>,
}

impl Field {
    pub fn new(tag: u8, data: Vec<u8>) -> Self {
        Self { tag, data }
    }

    pub fn encoded_len(&self) -> usize {
        5 + self.data.len()
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<()> {
        let Ok(length) = u32::try_from(self.data.len()) else {
            return fail("Expected an unsigned 32-bit integer");
        };
        out.push(self.tag);
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&self.data);
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.encode_into(&mut out)?;
        Ok(out)
    }
}

/// Decode the zero-prefixed typed payload.
///
/// The runtime's parser checks the two prefix bytes for zero; their meaning is not
/// established and they are not assumed to be a success code.
pub fn fields(payload: &[u8]) -> Result<Vec<Field>> {
    if payload.is_empty() {
        return Ok(Vec::new());
    }
    if payload.len() < 2 || payload[..2] != PAYLOAD_PREFIX {
        return fail("Unsupported payload prefix");
    }
    let mut result = Vec::new();
    let mut offset = 2;
    while offset < payload.len() {
        if payload.len() - offset < 5 {
            return fail("Truncated TLV header");
        }
        let tag = payload[offset];
        let length = be_u32(payload, offset + 1) as usize;
        offset += 5;
        if length > payload.len() - offset {
            return fail("Truncated TLV data");
        }
        result.push(Field::new(tag, payload[offset..offset + length].to_vec()));
        offset += length;
    }
    Ok(result)
}

/// Payload bytes for a list of items, with the zero prefix.
pub fn encode_payload<'a>(items: impl IntoIterator<Item = &'a Field>) -> Result<Vec<u8>> {
    let mut payload = PAYLOAD_PREFIX.to_vec();
    for item in items {
        item.encode_into(&mut payload)?;
    }
    Ok(payload)
}

pub fn uint32_array(values: &[u32]) -> Field {
    let mut data = Vec::with_capacity(4 + values.len() * 4);
    data.extend_from_slice(&(values.len() as u32).to_be_bytes());
    for value in values {
        data.extend_from_slice(&value.to_be_bytes());
    }
    Field::new(UINT32_ARRAY_TAG, data)
}

pub fn read_uint32_array(field: &Field) -> Result<Vec<u32>> {
    if field.tag != UINT32_ARRAY_TAG || field.data.len() < 4 {
        return fail("Not a uint32 array field");
    }
    let count = be_u32(&field.data, 0) as usize;
    if count != (field.data.len() - 4) / 4 || field.data.len() != 4 + count * 4 {
        return fail("Array count does not match byte length");
    }
    Ok((0..count).map(|i| be_u32(&field.data, 4 + i * 4)).collect())
}

/// The inner protocol upgrade request the runtime sends first (versions 0x10000 to 0x10008).
pub fn protocol_upgrade(request_id: u32) -> Message {
    let versions: Vec<u32> = (0x10000..0x10009).collect();
    let payload = encode_payload([&uint32_array(&versions)]).expect("small payload");
    Message::new(REQUEST_KIND, request_id, 0, PROTOCOL_UPGRADE_COMMAND, 0, payload)
}

/// Payload-free camera hardware-info request built by 0x1402c5c10.
///
/// The tracker answers it only after protocol 0x10008 or later was negotiated; the reply
/// carries up to four camera calibration records.
pub fn hardware_info_request(request_id: u32) -> Message {
    Message::header_only(REQUEST_KIND, request_id, HARDWARE_INFO_COMMAND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::hex;

    fn upgrade_bytes() -> Vec<u8> {
        hex("00000051 00000001 00000000 000003e8 00000000 0000002f
             0000 17 00000028 00000009
             00010000 00010001 00010002 00010003 00010004
             00010005 00010006 00010007 00010008")
    }

    #[test]
    fn observed_upgrade_encoding() {
        assert_eq!(protocol_upgrade(1).encode().unwrap(), upgrade_bytes());
    }

    #[test]
    fn every_split_point() {
        let upgrade = upgrade_bytes();
        for split in 0..=upgrade.len() {
            let mut decoder = Decoder::default();
            let mut messages = decoder.feed(&upgrade[..split]).unwrap();
            messages.extend(decoder.feed(&upgrade[split..]).unwrap());
            decoder.finish().unwrap();
            assert_eq!(messages, [protocol_upgrade(1)]);
        }
    }

    #[test]
    fn bytewise_and_coalesced_messages() {
        let empty = Message::new(0x52, 2, 3, 4, 5, Vec::new());
        let data = [upgrade_bytes(), empty.encode().unwrap(), upgrade_bytes()].concat();
        let mut decoder = Decoder::default();
        let mut messages = Vec::new();
        for byte in &data {
            messages.extend(decoder.feed(std::slice::from_ref(byte)).unwrap());
        }
        decoder.finish().unwrap();
        assert_eq!(messages, Decoder::default().feed(&data).unwrap());
        assert_eq!(messages, [protocol_upgrade(1), empty, protocol_upgrade(1)]);
    }

    #[test]
    fn eof_every_truncation() {
        let upgrade = upgrade_bytes();
        for length in 1..upgrade.len() {
            let mut decoder = Decoder::default();
            decoder.feed(&upgrade[..length]).unwrap();
            assert!(decoder.finish().is_err());
        }
    }

    #[test]
    fn oversize_header_poisons() {
        let mut decoder = Decoder::new(128);
        let header = Message::new(0x51, 1, 0, 1000, 0, Vec::new()).encode().unwrap();
        let mut oversize = header[..20].to_vec();
        oversize.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(decoder.feed(&oversize).is_err());
        assert!(decoder.pending().is_empty());
        assert!(decoder.feed(&upgrade_bytes()).is_err());
    }

    #[test]
    fn unknown_fields_preserved_and_arrays_read() {
        let opaque = Field::new(0xFE, b"opaque".to_vec());
        let payload = encode_payload([&opaque]).unwrap();
        assert_eq!(fields(&payload).unwrap(), [opaque]);
        let array = fields(&protocol_upgrade(1).payload).unwrap().remove(0);
        assert_eq!(read_uint32_array(&array).unwrap(), (0x10000..0x10009).collect::<Vec<_>>());
    }

    #[test]
    fn malformed_payloads() {
        for payload in [&b"\0"[..], b"\0\x01", b"\0\0\x17", b"\0\0\x17\0\0\0\x05abc"] {
            assert!(fields(payload).is_err());
        }
        assert!(read_uint32_array(&Field::new(0x17, vec![0xFF; 4])).is_err());
    }

    #[test]
    fn hardware_info_request_matches_original_bytes() {
        assert_eq!(
            hardware_info_request(0x1234_5678).encode().unwrap(),
            hex("00000051 12345678 00000000 00000848 00000000 00000000")
        );
    }
}
