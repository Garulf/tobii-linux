//! The bundled game DLL's PRP client session: the named-pipe handshake, transport framing,
//! and the command request/response messages the client sends to obtain head pose: connect
//! (1), stream start (3), stream stop (4) and timesync (22), and the property messages of
//! the game integration layer: get (operation 5, answered by operation 6), subscribe
//! (command 5) and unsubscribe (command 6).
//!
//! Pure codecs; they open no pipes or sockets. Recovered from DLL SHA256
//! 35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020;
//! docs/service-transport.md has locations, evidence and limits, and
//! tools/prp_session_oracle.py runs the original serializer, decoder and parsers.

use crate::error::{Result, ServiceError, prp};
use crate::prp::{DEFAULT_MAX_PAYLOAD, HEADER_SIZE, MAGIC, decode_envelope, encode_envelope, le_u32, push_u32};

pub const URL_SCHEME: &str = "tobii-prp://";
pub const SERVER_PIPE_PREFIX: &str = "TOBIIPRP-";

pub const COMMAND_CHANNEL: u32 = 0x4E21;
pub const SUBSCRIPTION_CHANNEL: u32 = 0x4E22;
pub const TIMESYNC_CHANNEL: u32 = 0x4E23;

pub const HANDSHAKE_VERSION: u32 = 1;
pub const HANDSHAKE_ACK: u32 = 1;
/// Server-side buffer for the handshake information message (0x18008f470).
pub const MAX_HANDSHAKE_INFO: usize = 0x100;
/// Client WriteFile and ReadFile chunk size (0x18004f250, 0x180090ad0).
pub const PIPE_CHUNK: usize = 0x1000;
const REPLY_PIPE_LIMIT: usize = 0xF8;

pub const OPERATION_STREAM_PACKAGE: u32 = 1;
pub const OPERATION_PROPERTY_NOTIFICATION: u32 = 2;
pub const OPERATION_PROPERTY_GET: u32 = 5;
pub const OPERATION_PROPERTY_GET_RESPONSE: u32 = 6;
pub const OPERATION_COMMAND_REQUEST: u32 = 9;
pub const OPERATION_COMMAND_RESPONSE: u32 = 10;

pub const COMMAND_CONNECT: u32 = 1;
pub const COMMAND_STREAM_START: u32 = 3;
pub const COMMAND_STREAM_STOP: u32 = 4;
pub const COMMAND_TIMESYNC: u32 = 0x16;
pub const COMMAND_PROPERTY_SUBSCRIBE: u32 = 5;
pub const COMMAND_PROPERTY_UNSUBSCRIBE: u32 = 6;
pub const WORD_RESPONSE_COMMANDS: [u32; 3] = [COMMAND_STREAM_START, COMMAND_STREAM_STOP, COMMAND_PROPERTY_UNSUBSCRIBE];

pub const STATUS_NAMES: [&str; 20] = [
    "NO_ERROR",
    "INTERNAL",
    "NOT_SUPPORTED",
    "CONNECTION_FAILED",
    "ALREADY_SUBSCRIBED",
    "NOT_SUBSCRIBED",
    "UNKNOWN_OPERATION",
    "OPERATION_FAILED",
    "INVALID_PARAMETER",
    "VERSION_MISMATCH",
    "ALLOCATION_FAILED",
    "NOT_AVAILABLE",
    "UNKNOWN_DEVICE_URL",
    "ALREADY_CREATED",
    "CALIBRATION_INPROGRESS",
    "CALIBRATION_NOT_STARTED",
    "TOO_MANY_SUBSCRIBERS",
    "INSUFFICIENT_LICENSE",
    "CONNECTION_FAILED_DRIVER",
    "UNAUTHORIZED",
];
pub const STATUS_OK: i32 = 0;
pub const STATUS_NOT_SUPPORTED: i32 = 2;

pub const CONNECT_REQUEST_BLOCK: usize = 0x3C;
pub const CONNECT_REQUEST_BLOCK_EXTENDED: usize = 0x40;
pub const CONNECT_RESPONSE_BLOCK: usize = 0x98;
pub const CONNECT_RESPONSE_BLOCK_EXTENDED: usize = 0xA8;

/// Decoder capacity limits for connect response lists (0x180055780, operation 10).
pub const LIMIT_LIST: usize = 0x19;
pub const LIMIT_STREAMS: usize = 0x20;
pub const LIMIT_SUPPORTED_COMMANDS: usize = 0x21;
pub const LIMIT_GROUPS: usize = 5;
pub const LIMIT_GROUP_VALUES: usize = 0x20;
pub const LIMIT_LICENSE_RESULTS: usize = 0x100;
pub const LIMIT_LIST_G: usize = 0x20;
pub const MAX_LICENSES: usize = 0x100;
pub const MAX_SUPPORTED_COMMAND: i32 = 0x20;
pub const STRING_LIMITS: [usize; 7] = [0x100, 0x100, 0x100, 0x100, 0x100, 0x80, 0x80];
pub const EXTENDED_STRING_LIMIT: usize = 0x80;
pub const BUILD_VERSION_LIMIT: usize = 0x100;

pub const PROPERTY_DISPLAY_AREA: u32 = 1;
pub const PROPERTY_DISPLAY_ID: u32 = 0x12;
pub const DISPLAY_AREA_FLOATS: usize = 9;
pub const STRING_PROPERTIES: [u32; 2] = [8, PROPERTY_DISPLAY_ID];
pub const STRING_PROPERTY_LIMIT: usize = 0x100;

fn signed(value: u32) -> i32 {
    value as i32
}

fn ascii(text: &str, limit: usize) -> Result<&[u8]> {
    let raw = text.as_bytes();
    if !text.is_ascii() || raw.contains(&0) || raw.len() >= limit {
        return prp("string must be ASCII without NUL and fit its buffer");
    }
    Ok(raw)
}

fn decode_ascii(raw: &[u8]) -> Result<String> {
    match std::str::from_utf8(raw) {
        Ok(text) if text.is_ascii() => Ok(text.to_string()),
        _ => prp("PRP string is not ASCII"),
    }
}

fn find_nul(data: &[u8], from: usize) -> Option<usize> {
    data.get(from..)?.iter().position(|byte| *byte == 0).map(|index| from + index)
}

// Pipe naming and handshake ------------------------------------------------------------

/// Pipe names the client tries for a tobii-prp:// URL, in order.
///
/// The client looks for TOBIIPRP-<name> in a `\\.\pipe\*` listing; if it is not listed it
/// opens <name> unchanged (0x18004e8a0, 0x18008fb20).
pub fn server_pipe_names(url: &str) -> Result<(String, String)> {
    match url.strip_prefix(URL_SCHEME) {
        Some(name) if !name.is_empty() => Ok((format!("{SERVER_PIPE_PREFIX}{name}"), name.to_string())),
        _ => prp("expected a non-empty tobii-prp:// URL"),
    }
}

/// Name of the inbound pipe the client creates for one channel.
pub fn client_pipe_name(server_pipe: &str, pid: u32, counter: u64) -> String {
    format!("client_{server_pipe}_{pid:08x}_{counter:016x}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHello {
    pub channel: u32,
    pub reply_pipe: String,
    pub version: u32,
}

pub fn encode_client_hello(hello: &ClientHello) -> Result<Vec<u8>> {
    let name = ascii(&hello.reply_pipe, REPLY_PIPE_LIMIT)?;
    let mut message = Vec::with_capacity(9 + name.len());
    push_u32(&mut message, hello.version);
    push_u32(&mut message, hello.channel);
    message.extend_from_slice(name);
    message.push(0);
    Ok(message)
}

/// Parse the first client message as the PRP-mode server does (0x18008f470).
pub fn decode_client_hello(message: &[u8]) -> Result<ClientHello> {
    if message.len() < 9 {
        return prp("truncated client hello");
    }
    let (version, channel) = (le_u32(message, 0), le_u32(message, 4));
    let Some(end) = find_nul(message, 8) else {
        return prp("unterminated client pipe name");
    };
    if version == 0 {
        return prp("server rejects handshake version 0");
    }
    Ok(ClientHello { channel, reply_pipe: decode_ascii(&message[8..end])?, version })
}

pub fn encode_handshake_ack() -> Vec<u8> {
    HANDSHAKE_ACK.to_le_bytes().to_vec()
}

/// The client accepts exactly one four-byte value 1 (0x18004f300, 0x18004f00b).
pub fn check_handshake_ack(message: &[u8]) -> Result<()> {
    if message.len() != 4 || le_u32(message, 0) != HANDSHAKE_ACK {
        return prp("client rejects handshake acknowledgement");
    }
    Ok(())
}

pub fn encode_client_pid(pid: u32) -> Vec<u8> {
    pid.to_le_bytes().to_vec()
}

pub fn decode_client_pid(message: &[u8]) -> Result<u32> {
    if message.len() < 4 {
        return prp("truncated client process id");
    }
    Ok(le_u32(message, 0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeInfo {
    pub client_id: i32,
    pub subscription_url: String,
    pub timesync_url: String,
}

pub fn encode_handshake_info(info: &HandshakeInfo) -> Result<Vec<u8>> {
    let first = ascii(&info.subscription_url, MAX_HANDSHAKE_INFO)?;
    let second = ascii(&info.timesync_url, MAX_HANDSHAKE_INFO)?;
    let mut message = info.client_id.to_le_bytes().to_vec();
    for text in [first, second] {
        message.extend_from_slice(text);
        message.push(0);
    }
    if message.len() > MAX_HANDSHAKE_INFO {
        return prp("handshake information exceeds the server buffer");
    }
    Ok(message)
}

/// Parse with the client's rule: id plus two NUL-terminated strings (0x18004f360).
pub fn decode_handshake_info(message: &[u8]) -> Result<HandshakeInfo> {
    if message.len() < 6 {
        return prp("handshake information needs an id and two strings");
    }
    let first = find_nul(message, 4);
    let second = first.and_then(|first| find_nul(message, first + 1));
    let (Some(first), Some(second)) = (first, second) else {
        return prp("unterminated handshake string");
    };
    Ok(HandshakeInfo {
        client_id: signed(le_u32(message, 0)),
        subscription_url: decode_ascii(&message[4..first])?,
        timesync_url: decode_ascii(&message[first + 1..second])?,
    })
}

// Transport framing --------------------------------------------------------------------

/// Split one packet into the client's WriteFile chunks (one pipe message each).
pub fn pipe_chunks(packet: &[u8]) -> Vec<&[u8]> {
    if packet.is_empty() {
        return vec![packet];
    }
    packet.chunks(PIPE_CHUNK).collect()
}

/// Complete TPRP packets from reassembled bytes and the incomplete tail.
pub fn split_packets(stream: &[u8], max_payload: u64) -> Result<(Vec<&[u8]>, &[u8])> {
    let mut packets = Vec::new();
    let mut offset = 0;
    while stream.len() - offset >= HEADER_SIZE {
        let (magic, size, check) = (le_u32(stream, offset), le_u32(stream, offset + 4), le_u32(stream, offset + 8));
        if magic != MAGIC || check != size ^ MAGIC || u64::from(size) > max_payload {
            return prp("invalid PRP header in stream");
        }
        let end = offset + HEADER_SIZE + size as usize;
        if stream.len() < end {
            break;
        }
        packets.push(&stream[offset..end]);
        offset = end;
    }
    Ok((packets, &stream[offset..]))
}

// Block writer and reader ---------------------------------------------------------------

/// Fixed blocks, each followed by a length-prefixed variable region (0x18004fcc0).
#[derive(Debug, Default)]
struct Writer {
    buf: Vec<u8>,
    region: Option<usize>,
}

impl Writer {
    fn message(tag: u32, operation: u32) -> Self {
        let mut writer = Self::default();
        let header = writer.block(3);
        writer.set(header, 1, tag);
        writer.set(header, 2, operation);
        writer
    }

    fn block(&mut self, words: usize) -> usize {
        self.close();
        let start = self.buf.len();
        push_u32(&mut self.buf, (4 * words) as u32);
        self.buf.resize(start + 4 * words, 0);
        self.region = Some(self.buf.len());
        self.buf.extend_from_slice(&[0; 4]);
        start
    }

    fn set(&mut self, block: usize, index: usize, value: u32) {
        self.put(block + 4 * index, &value.to_le_bytes());
    }

    fn put(&mut self, at: usize, data: &[u8]) {
        self.buf[at..at + data.len()].copy_from_slice(data);
    }

    fn region(&self) -> usize {
        self.region.expect("a block was written")
    }

    fn data(&mut self, raw: &[u8]) -> u32 {
        let offset = self.buf.len() - self.region();
        self.buf.extend_from_slice(raw);
        self.buf.resize(self.buf.len() + raw.len().wrapping_neg() % 4, 0);
        offset as u32
    }

    fn words(&mut self, values: &[i32]) -> u32 {
        let raw: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
        self.data(&raw)
    }

    fn string(&mut self, text: &str, limit: usize) -> Result<u32> {
        let mut raw = ascii(text, limit)?.to_vec();
        raw.push(0);
        Ok(self.data(&raw))
    }

    fn reserve(&mut self, size: usize) -> usize {
        let offset = self.buf.len() - self.region();
        self.buf.resize(self.buf.len() + size, 0);
        offset
    }

    /// Writes at `offset` within the current region.
    fn put_region(&mut self, offset: usize, words: &[u32]) {
        let at = self.region() + offset;
        let raw: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        self.put(at, &raw);
    }

    /// Append an already encoded block and its region.
    fn raw(&mut self, block: &[u8], region: &[u8]) {
        self.close();
        self.buf.extend_from_slice(block);
        self.region = Some(self.buf.len());
        self.buf.extend_from_slice(region);
    }

    fn close(&mut self) {
        if let Some(region) = self.region {
            let size = (self.buf.len() - region) as u32;
            self.put(region, &size.to_le_bytes());
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.close();
        self.buf.extend_from_slice(&[0; 4]);
        encode_envelope(&self.buf, DEFAULT_MAX_PAYLOAD).expect("session messages fit the default limit")
    }
}

/// One block and the region that follows it.
type Pair<'a> = (&'a [u8], &'a [u8]);

/// Split a payload into (block, region) pairs as validated by 0x180053d10.
fn chain(payload: &[u8]) -> Result<Vec<Pair<'_>>> {
    let mut pairs = Vec::new();
    let mut offset = 0;
    loop {
        if offset + 4 > payload.len() {
            return prp("PRP block chain overruns the payload");
        }
        let size = le_u32(payload, offset) as usize;
        if size == 0 {
            if offset + 4 != payload.len() {
                return prp("PRP block chain terminator is not final");
            }
            break;
        }
        let region = offset + size;
        if size < 4 || region + 4 > payload.len() {
            return prp("PRP block overruns the payload");
        }
        let region_size = le_u32(payload, region) as usize;
        if region_size < 4 || region + region_size > payload.len() {
            return prp("PRP region overruns the payload");
        }
        pairs.push((&payload[offset..region], &payload[region..region + region_size]));
        offset = region + region_size;
    }
    if pairs.len() < 2 {
        return prp("PRP message lacks header or operation block");
    }
    Ok(pairs)
}

fn words(block: &[u8], minimum: usize) -> Result<Vec<u32>> {
    if block.len() < minimum {
        return prp("PRP block is shorter than required");
    }
    Ok(block.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect())
}

fn bytes_at(region: &[u8], length: u32, offset: u32) -> Result<Vec<u8>> {
    if length == 0 || offset == 0 {
        return Ok(Vec::new());
    }
    let (start, end) = (offset as usize, offset as usize + length as usize);
    if end > region.len() {
        return prp("PRP field overruns its region");
    }
    Ok(region[start..end].to_vec())
}

fn string_at(region: &[u8], offset: u32, limit: usize) -> Result<String> {
    let offset = offset as usize;
    if offset >= region.len() {
        return prp("PRP string overruns its region");
    }
    let Some(end) = find_nul(region, offset) else {
        return prp("unterminated PRP string");
    };
    decode_ascii(&region[offset..end.min(offset + limit - 1)])
}

fn ints_at(region: &[u8], count: u32, offset: u32, limit: i32) -> Result<Vec<i32>> {
    let count = signed(count).min(limit);
    if count <= 0 {
        return Ok(Vec::new());
    }
    let (start, count) = (offset as usize, count as usize);
    if start + 4 * count > region.len() {
        return prp("PRP list overruns its region");
    }
    Ok((0..count).map(|index| signed(le_u32(region, start + 4 * index))).collect())
}

/// Words of a fixed layout read with `struct.unpack_from` by the Python codec.
fn unpack_words(region: &[u8], offset: usize, count: usize) -> Result<Vec<u32>> {
    if offset + 4 * count > region.len() {
        return Err(ServiceError::Truncated(format!(
            "unpack_from requires a buffer of at least {} bytes for unpacking {} bytes at offset {offset} \
             (actual buffer size is {})",
            offset + 4 * count,
            4 * count,
            region.len()
        )));
    }
    Ok((0..count).map(|index| le_u32(region, offset + 4 * index)).collect())
}

/// Tag, operation and the pairs after the header of one packet.
fn header(packet: &[u8]) -> Result<(u32, u32, Vec<Pair<'_>>)> {
    let mut pairs = chain(decode_envelope(packet, DEFAULT_MAX_PAYLOAD)?)?;
    let header = words(pairs[0].0, 12)?;
    pairs.remove(0);
    Ok((header[1], header[2], pairs))
}

// Command requests ---------------------------------------------------------------------

/// Command 1 body serialized by 0x18004fdd0 from the native request at 0x18001fed0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    pub subscription_client_id: i32,
    pub licenses: Vec<Vec<u8>>,
    pub signer_public_key: Vec<u8>,
    pub client_version: Vec<i32>,
    pub process_id: u32,
    pub image_directory: Vec<u8>,
    pub image_name: Vec<u8>,
    pub field_of_use: i32,
}

impl Default for ConnectRequest {
    fn default() -> Self {
        Self {
            subscription_client_id: 0,
            licenses: Vec::new(),
            signer_public_key: Vec::new(),
            client_version: vec![4, 17, 0, 6],
            process_id: 0,
            image_directory: Vec::new(),
            image_name: Vec::new(),
            field_of_use: 1,
        }
    }
}

/// A command request as the server sees it; `stream` for stream start and stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub tag: u32,
    pub command: u32,
    pub stream: Option<i32>,
    pub connect: Option<ConnectRequest>,
}

pub fn encode_connect_request(tag: u32, request: &ConnectRequest, extended: bool) -> Result<Vec<u8>> {
    if request.licenses.len() > MAX_LICENSES || request.client_version.len() != 4 {
        return prp("connect request has too many licenses or a bad version tuple");
    }
    let mut writer = Writer::message(tag, OPERATION_COMMAND_REQUEST);
    let command = writer.block(2);
    writer.set(command, 1, COMMAND_CONNECT);
    let size = if extended { CONNECT_REQUEST_BLOCK_EXTENDED } else { CONNECT_REQUEST_BLOCK };
    let body = writer.block(size / 4);
    writer.set(body, 1, request.subscription_client_id as u32);
    writer.set(body, 2, request.licenses.len() as u32);
    let table = writer.reserve(8 * request.licenses.len());
    writer.set(body, 3, table as u32);
    for (index, license) in request.licenses.iter().enumerate() {
        let offset = if license.is_empty() { 0 } else { writer.data(license) };
        writer.put_region(table + 8 * index, &[license.len() as u32, offset]);
    }
    for (slot, raw) in [(4, &request.signer_public_key), (12, &request.image_name), (10, &request.image_directory)] {
        if !raw.is_empty() {
            writer.set(body, slot, raw.len() as u32);
            let offset = writer.data(raw);
            writer.set(body, slot + 1, offset);
        }
    }
    for (index, value) in request.client_version.iter().enumerate() {
        writer.set(body, 6 + index, *value as u32);
    }
    writer.set(body, 14, request.process_id);
    if extended {
        writer.set(body, 15, request.field_of_use as u32);
    }
    Ok(writer.finish())
}

fn is_stream_command(command: u32) -> bool {
    command == COMMAND_STREAM_START || command == COMMAND_STREAM_STOP
}

/// Stream start (3) or stop (4) carrying one PRP stream id.
pub fn encode_stream_request(tag: u32, command: u32, stream: i32) -> Result<Vec<u8>> {
    if !is_stream_command(command) {
        return prp("not a stream command");
    }
    let mut writer = Writer::message(tag, OPERATION_COMMAND_REQUEST);
    let block = writer.block(2);
    writer.set(block, 1, command);
    let block = writer.block(2);
    writer.set(block, 1, stream as u32);
    Ok(writer.finish())
}

pub fn encode_timesync_request(tag: u32) -> Vec<u8> {
    let mut writer = Writer::message(tag, OPERATION_COMMAND_REQUEST);
    let block = writer.block(2);
    writer.set(block, 1, COMMAND_TIMESYNC);
    writer.block(1);
    writer.finish()
}

fn session_subset<T>() -> Result<T> {
    prp("command is outside the head-pose session subset")
}

/// Server-side view of the commands this client sends during head-pose sessions.
pub fn decode_command_request(packet: &[u8], extended: bool) -> Result<CommandRequest> {
    let (tag, operation, pairs) = header(packet)?;
    if operation != OPERATION_COMMAND_REQUEST || pairs.len() < 2 {
        return prp("not a PRP command request");
    }
    let command = words(pairs[0].0, 8)?[1];
    let (block, region) = pairs[1];
    let request = |stream, connect| CommandRequest { tag, command, stream, connect };
    if is_stream_command(command) {
        return Ok(request(Some(signed(words(block, 8)?[1])), None));
    }
    if command == COMMAND_TIMESYNC {
        words(block, 4)?;
        return Ok(request(None, None));
    }
    if command != COMMAND_CONNECT {
        return session_subset();
    }
    let w = words(block, CONNECT_REQUEST_BLOCK)?;
    let count = signed(w[2]).min(MAX_LICENSES as i32).max(0) as usize;
    let mut licenses = Vec::with_capacity(count);
    for index in 0..count {
        let entry = unpack_words(region, w[3] as usize + 8 * index, 2)?;
        licenses.push(bytes_at(region, entry[0], entry[1])?);
    }
    let connect = ConnectRequest {
        subscription_client_id: signed(w[1]),
        licenses,
        signer_public_key: bytes_at(region, w[4], w[5])?,
        client_version: w[6..10].iter().map(|value| signed(*value)).collect(),
        process_id: w[14],
        image_directory: bytes_at(region, w[10], w[11])?,
        image_name: bytes_at(region, w[12], w[13])?,
        field_of_use: if extended && block.len() >= CONNECT_REQUEST_BLOCK_EXTENDED { signed(w[15]) } else { 0 },
    };
    Ok(request(None, Some(connect)))
}

// Command responses --------------------------------------------------------------------

/// Command 1 response body, serialized by 0x180050370 and stored by the device.
///
/// Field names follow observed consumers; lists without a recovered consumer keep neutral
/// names. `streams` holds PRP stream ids checked by stream support (0x18002c6d0);
/// `license_results` must have one entry per requested license (0x18002a6a0);
/// `firmware_upgrade_in_progress` must be false for device creation. `supported_commands`
/// is compared with PRP command ids: 0x16 (timesync) makes device creation run up to four
/// timesync rounds (0x180030630), and capability queries search it (0x18002cd30). Treating
/// it as a command list is an inference. `strings`, `words` and `extended_strings` hold 7,
/// 9 and 4 entries; other counts are refused on encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectResponse {
    pub build_version: String,
    pub firmware_upgrade_in_progress: bool,
    pub list_a: Vec<i32>,
    pub list_b: Vec<i32>,
    pub list_c: Vec<i32>,
    pub list_d: Vec<i32>,
    pub streams: Vec<i32>,
    pub supported_commands: Vec<i32>,
    pub groups: Vec<(i32, Vec<i32>)>,
    pub strings: Vec<String>,
    pub words: Vec<i32>,
    pub license_results: Vec<i32>,
    pub mode: i32,
    pub list_g: Vec<i32>,
    pub extended_strings: Vec<String>,
}

impl Default for ConnectResponse {
    fn default() -> Self {
        Self {
            build_version: String::new(),
            firmware_upgrade_in_progress: false,
            list_a: Vec::new(),
            list_b: Vec::new(),
            list_c: Vec::new(),
            list_d: Vec::new(),
            streams: Vec::new(),
            supported_commands: Vec::new(),
            groups: Vec::new(),
            strings: vec![String::new(); 7],
            words: vec![0; 9],
            license_results: Vec::new(),
            mode: 0,
            list_g: Vec::new(),
            extended_strings: vec![String::new(); 4],
        }
    }
}

/// The body each command's response carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseBody {
    Connect(Box<ConnectResponse>),
    /// Stream start and stop, property unsubscribe.
    Word(i32),
    Timesync(u64),
}

/// How the response to `command` carries its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseKind {
    Connect,
    Word,
    Timesync,
}

pub fn response_kind(command: u32) -> Result<ResponseKind> {
    match command {
        COMMAND_CONNECT => Ok(ResponseKind::Connect),
        COMMAND_TIMESYNC => Ok(ResponseKind::Timesync),
        command if WORD_RESPONSE_COMMANDS.contains(&command) => Ok(ResponseKind::Word),
        _ => session_subset(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResponse {
    pub tag: u32,
    pub command: u32,
    pub status: i32,
    pub body: ResponseBody,
}

fn check_connect(response: &ConnectResponse) -> Result<()> {
    let lists = [
        (response.list_a.len(), LIMIT_LIST),
        (response.list_b.len(), LIMIT_LIST),
        (response.list_c.len(), LIMIT_LIST),
        (response.list_d.len(), LIMIT_LIST),
        (response.streams.len(), LIMIT_STREAMS),
        (response.supported_commands.len(), LIMIT_SUPPORTED_COMMANDS),
        (response.license_results.len(), LIMIT_LICENSE_RESULTS),
        (response.list_g.len(), LIMIT_LIST_G),
        (response.groups.len(), LIMIT_GROUPS),
    ];
    if lists.iter().any(|(length, limit)| length > limit) {
        return prp("connect response list exceeds the client buffer");
    }
    if response.groups.iter().any(|(kind, values)| values.len() > LIMIT_GROUP_VALUES || !(0..5).contains(kind)) {
        return prp("connect response group exceeds the client buffer");
    }
    if response.supported_commands.iter().any(|value| !(0..=MAX_SUPPORTED_COMMAND).contains(value)) {
        return prp("client discards supported command values above 0x20");
    }
    if (response.strings.len(), response.words.len(), response.extended_strings.len()) != (7, 9, 4) {
        return prp("connect response has the wrong number of fixed fields");
    }
    Ok(())
}

fn encode_connect_body(writer: &mut Writer, response: &ConnectResponse, extended: bool) -> Result<()> {
    check_connect(response)?;
    let size = if extended { CONNECT_RESPONSE_BLOCK_EXTENDED } else { CONNECT_RESPONSE_BLOCK };
    let body = writer.block(size / 4);
    let offset = writer.string(&response.build_version, BUILD_VERSION_LIMIT)?;
    writer.set(body, 1, offset);
    writer.set(body, 2, u32::from(response.firmware_upgrade_in_progress));
    for (slot, values) in [
        (3, &response.list_a),
        (5, &response.list_b),
        (7, &response.list_c),
        (9, &response.list_d),
        (11, &response.streams),
        (13, &response.supported_commands),
    ] {
        writer.set(body, slot, values.len() as u32);
        let offset = writer.words(values);
        writer.set(body, slot + 1, offset);
    }
    writer.set(body, 15, response.groups.len() as u32);
    let table = writer.reserve(12 * response.groups.len());
    writer.set(body, 16, table as u32);
    for (index, (kind, values)) in response.groups.iter().enumerate() {
        let offset = writer.words(values);
        writer.put_region(table + 12 * index, &[*kind as u32, values.len() as u32, offset]);
    }
    for (index, (text, limit)) in response.strings.iter().zip(STRING_LIMITS).enumerate() {
        let offset = writer.string(text, limit)?;
        writer.set(body, 17 + index, offset);
    }
    for (index, value) in response.words.iter().enumerate() {
        writer.set(body, 24 + index, *value as u32);
    }
    writer.set(body, 33, response.license_results.len() as u32);
    let offset = writer.words(&response.license_results);
    writer.set(body, 34, offset);
    writer.set(body, 35, response.mode as u32);
    writer.set(body, 36, response.list_g.len() as u32);
    let offset = writer.words(&response.list_g);
    writer.set(body, 37, offset);
    if extended {
        for (index, text) in response.extended_strings.iter().enumerate() {
            let offset = writer.string(text, EXTENDED_STRING_LIMIT)?;
            writer.set(body, 38 + index, offset);
        }
    }
    Ok(())
}

/// Serialize operation 10 for connect, stream start/stop, timesync and unsubscribe.
pub fn encode_command_response(response: &CommandResponse, extended: bool) -> Result<Vec<u8>> {
    let mut writer = Writer::message(response.tag, OPERATION_COMMAND_RESPONSE);
    let header = writer.block(3);
    writer.set(header, 1, response.status as u32);
    writer.set(header, 2, response.command);
    match &response.body {
        ResponseBody::Connect(connect) => encode_connect_body(&mut writer, connect, extended)?,
        ResponseBody::Word(value) => {
            let block = writer.block(2);
            writer.set(block, 1, *value as u32);
        }
        ResponseBody::Timesync(value) => {
            let block = writer.block(3);
            writer.set(block, 1, (value >> 32) as u32);
            writer.set(block, 2, *value as u32);
        }
    }
    Ok(writer.finish())
}

fn decode_connect_body(block: &[u8], region: &[u8], extended: bool) -> Result<ConnectResponse> {
    let w = words(block, CONNECT_RESPONSE_BLOCK)?;
    let mut lists = Vec::with_capacity(5);
    for (slot, limit) in [(3, LIMIT_LIST), (5, LIMIT_LIST), (7, LIMIT_LIST), (9, LIMIT_LIST), (11, LIMIT_STREAMS)] {
        lists.push(ints_at(region, w[slot], w[slot + 1], limit as i32)?);
    }
    let mut supported_commands: Vec<i32> =
        ints_at(region, w[13], w[14], i32::MAX)?.into_iter().filter(|value| *value <= MAX_SUPPORTED_COMMAND).collect();
    supported_commands.truncate(LIMIT_SUPPORTED_COMMANDS);
    let mut groups = Vec::new();
    for index in 0..signed(w[15]).max(0) as usize {
        let entry = unpack_words(region, w[16] as usize + 12 * index, 3)?;
        if signed(entry[0]) < 5 && groups.len() < LIMIT_GROUPS {
            groups.push((signed(entry[0]), ints_at(region, entry[1], entry[2], LIMIT_GROUP_VALUES as i32)?));
        }
    }
    let strings = STRING_LIMITS
        .iter()
        .enumerate()
        .map(|(index, limit)| string_at(region, w[17 + index], *limit))
        .collect::<Result<Vec<_>>>()?;
    let build_version = string_at(region, w[1], BUILD_VERSION_LIMIT)?;
    let license_results = ints_at(region, w[33], w[34], LIMIT_LICENSE_RESULTS as i32)?;
    let list_g = ints_at(region, w[36], w[37], LIMIT_LIST_G as i32)?;
    let extended_strings = if extended && block.len() >= CONNECT_RESPONSE_BLOCK_EXTENDED {
        (0..4).map(|index| string_at(region, w[38 + index], EXTENDED_STRING_LIMIT)).collect::<Result<Vec<_>>>()?
    } else {
        vec![String::new(); 4]
    };
    let [list_a, list_b, list_c, list_d, streams]: [Vec<i32>; 5] = lists.try_into().expect("five lists");
    Ok(ConnectResponse {
        build_version,
        firmware_upgrade_in_progress: w[2] != 0,
        list_a,
        list_b,
        list_c,
        list_d,
        streams,
        supported_commands,
        groups,
        strings,
        words: w[24..33].iter().map(|value| signed(*value)).collect(),
        license_results,
        mode: signed(w[35]),
        list_g,
        extended_strings,
    })
}

/// Client-side view of operation 10 for the head-pose session commands.
pub fn decode_command_response(packet: &[u8], extended: bool) -> Result<CommandResponse> {
    let (tag, operation, pairs) = header(packet)?;
    if operation != OPERATION_COMMAND_RESPONSE || pairs.len() < 2 {
        return prp("not a PRP command response");
    }
    let header = words(pairs[0].0, 12)?;
    let (status, command) = (signed(header[1]), header[2]);
    let (block, region) = pairs[1];
    let body = match response_kind(command)? {
        ResponseKind::Connect => ResponseBody::Connect(Box::new(decode_connect_body(block, region, extended)?)),
        ResponseKind::Word => ResponseBody::Word(signed(words(block, 8)?[1])),
        ResponseKind::Timesync => {
            let w = words(block, 12)?;
            ResponseBody::Timesync((u64::from(w[1]) << 32) | u64::from(w[2]))
        }
    };
    Ok(CommandResponse { tag, command, status, body })
}

// Property messages ---------------------------------------------------------------------

/// Operation 5 as the client sends it (0x180019f60): one block holding the property id.
pub fn encode_property_request(tag: u32, property_id: u32) -> Vec<u8> {
    let mut writer = Writer::message(tag, OPERATION_PROPERTY_GET);
    let block = writer.block(2);
    writer.set(block, 1, property_id);
    writer.finish()
}

fn is_property_subscription(command: u32) -> bool {
    command == COMMAND_PROPERTY_SUBSCRIBE || command == COMMAND_PROPERTY_UNSUBSCRIBE
}

/// Command 5 (subscribe, 0x180019830) or 6 (unsubscribe) with the property id.
pub fn check_property_subscription_command(command: i64) -> Result<u32> {
    match u32::try_from(command) {
        Ok(command) if is_property_subscription(command) => Ok(command),
        _ => prp("not a property subscription command"),
    }
}

pub fn encode_property_subscribe_request(tag: u32, property_id: u32, command: u32) -> Result<Vec<u8>> {
    let command = check_property_subscription_command(i64::from(command))?;
    let mut writer = Writer::message(tag, OPERATION_COMMAND_REQUEST);
    let block = writer.block(2);
    writer.set(block, 1, command);
    let block = writer.block(2);
    writer.set(block, 1, property_id);
    Ok(writer.finish())
}

/// Tag, command (5 or 6) and property id of a property subscription request.
pub fn decode_property_subscribe_request(packet: &[u8]) -> Result<(u32, u32, u32)> {
    let (tag, operation, pairs) = header(packet)?;
    let command = words(pairs[0].0, 8)?[1];
    if operation != OPERATION_COMMAND_REQUEST || pairs.len() < 2 || !is_property_subscription(command) {
        return prp("not a PRP property subscription request");
    }
    Ok((tag, command, words(pairs[1].0, 8)?[1]))
}

/// Tag and property id of an operation 5 request.
pub fn decode_property_request(packet: &[u8]) -> Result<(u32, u32)> {
    let (tag, operation, pairs) = header(packet)?;
    if operation != OPERATION_PROPERTY_GET {
        return prp("not a PRP property get");
    }
    Ok((tag, words(pairs[0].0, 8)?[1]))
}

/// A property value with its status, as the raw value block and region.
///
/// Sent as operation 6 for a get (operation 5) and inside the command 5 response for a
/// subscribe; the value block layout depends on the property id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyResponse {
    pub tag: u32,
    pub status: i32,
    pub property_id: u32,
    pub block: Vec<u8>,
    pub region: Vec<u8>,
}

/// The empty value block and region: a size word each.
pub fn empty_value() -> (Vec<u8>, Vec<u8>) {
    (4u32.to_le_bytes().to_vec(), 4u32.to_le_bytes().to_vec())
}

pub fn encode_property_response(response: &PropertyResponse) -> Vec<u8> {
    let mut writer = Writer::message(response.tag, OPERATION_PROPERTY_GET_RESPONSE);
    let header = writer.block(3);
    writer.set(header, 1, response.status as u32);
    writer.set(header, 2, response.property_id);
    writer.raw(&response.block, &response.region);
    writer.finish()
}

/// Command 5 response: status, property id, then its current value (decoder 0x180055780).
pub fn encode_property_subscribe_response(response: &PropertyResponse) -> Vec<u8> {
    let mut writer = Writer::message(response.tag, OPERATION_COMMAND_RESPONSE);
    let header = writer.block(3);
    writer.set(header, 1, response.status as u32);
    writer.set(header, 2, COMMAND_PROPERTY_SUBSCRIBE);
    let block = writer.block(2);
    writer.set(block, 1, response.property_id);
    writer.raw(&response.block, &response.region);
    writer.finish()
}

/// Operation 2 on the subscription channel: {8, property} then the value as in operation 6.
///
/// Layout from the game DLL's serializer 0x18004f730 and decoder 0x180055780 (emulated).
pub fn encode_property_notification(tag: u32, property_id: u32, block: &[u8], region: &[u8]) -> Vec<u8> {
    let mut writer = Writer::message(tag, OPERATION_PROPERTY_NOTIFICATION);
    let header = writer.block(2);
    writer.set(header, 1, property_id);
    writer.raw(block, region);
    writer.finish()
}

pub fn decode_property_response(packet: &[u8]) -> Result<PropertyResponse> {
    let (tag, operation, pairs) = header(packet)?;
    if operation != OPERATION_PROPERTY_GET_RESPONSE || pairs.len() < 2 {
        return prp("not a PRP property get response");
    }
    let header = words(pairs[0].0, 12)?;
    let (block, region) = pairs[1];
    Ok(PropertyResponse {
        tag,
        status: signed(header[1]),
        property_id: header[2],
        block: block.to_vec(),
        region: region.to_vec(),
    })
}

pub fn decode_property_subscribe_response(packet: &[u8]) -> Result<PropertyResponse> {
    let (tag, operation, pairs) = header(packet)?;
    if operation != OPERATION_COMMAND_RESPONSE || pairs.len() < 3 {
        return prp("not a PRP command response with a property value");
    }
    let header = words(pairs[0].0, 12)?;
    if header[2] != COMMAND_PROPERTY_SUBSCRIBE {
        return prp("not a PRP property subscribe response");
    }
    let (block, region) = pairs[2];
    Ok(PropertyResponse {
        tag,
        status: signed(header[1]),
        property_id: words(pairs[1].0, 8)?[1],
        block: block.to_vec(),
        region: region.to_vec(),
    })
}

fn floats_block(values: &[f64]) -> Vec<u8> {
    let mut block = Vec::with_capacity(4 + 4 * values.len());
    push_u32(&mut block, (4 + 4 * values.len()) as u32);
    values.iter().for_each(|value| crate::prp::push_f32(&mut block, *value));
    block
}

/// Value block of float32 values, as property 1 and the track box carry them.
pub fn floats_value(values: &[f64]) -> (Vec<u8>, Vec<u8>) {
    (floats_block(values), 4u32.to_le_bytes().to_vec())
}

/// Value block for property 1: nine floats in millimetres (decoder 0x180055780).
pub fn display_area_value(corners: &[f64]) -> Result<(Vec<u8>, Vec<u8>)> {
    if corners.len() != DISPLAY_AREA_FLOATS {
        return prp("display area needs three corners of three coordinates");
    }
    Ok(floats_value(corners))
}

/// Value block for string properties: a region offset, then the NUL-terminated text.
pub fn string_value(text: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut raw = ascii(text, STRING_PROPERTY_LIMIT)?.to_vec();
    raw.push(0);
    raw.resize(raw.len().next_multiple_of(4), 0);
    let mut region = Vec::with_capacity(4 + raw.len());
    push_u32(&mut region, (4 + raw.len()) as u32);
    region.extend_from_slice(&raw);
    Ok(([8u32.to_le_bytes(), 4u32.to_le_bytes()].concat(), region))
}

pub fn decode_display_area(response: &PropertyResponse) -> Result<[f64; DISPLAY_AREA_FLOATS]> {
    if response.block.len() < 4 + 4 * DISPLAY_AREA_FLOATS {
        return prp("display area value block is too short");
    }
    Ok(std::array::from_fn(|index| crate::prp::le_f32(&response.block, 4 + 4 * index)))
}

pub fn decode_string_value(response: &PropertyResponse) -> Result<String> {
    let offset = words(&response.block, 8)?[1];
    string_at(&response.region, offset, STRING_PROPERTY_LIMIT)
}

/// Connect response satisfying the device_create checks recovered statically.
///
/// Only the build version string, the firmware-upgrade flag, the stream list and the
/// license-result count are known to be checked. `supported_commands` is left empty, so
/// device creation skips its timesync rounds. All other fields are zero or empty; their
/// effect on later API calls is not established.
pub fn minimal_connect_response(build_version: &str, license_count: usize, streams: &[i32]) -> ConnectResponse {
    ConnectResponse {
        build_version: build_version.to_string(),
        streams: streams.to_vec(),
        license_results: vec![0; license_count],
        ..ConnectResponse::default()
    }
}

#[cfg(test)]
mod tests;
