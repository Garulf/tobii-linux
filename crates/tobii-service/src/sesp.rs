//! Stream Engine Services protocol (SESP): the head-pose source of the game DLL's tet-tcp
//! route.
//!
//! The legacy TTP platform module that `tet-tcp://` devices get (docs/tet-tcp.md) computes
//! no head pose. It asks Stream Engine Services for it over the named pipe
//! `streamengineservices`. The service side is `ses_windows.dll` of the Tobii Windows
//! software; the client side is `sesp_services.cpp` in the game DLL. docs/sesp.md has the
//! evidence.
//!
//! Transport: per channel, the client writes `u32 channel, reply pipe name, NUL` (command
//! channel 0x2711, subscription channel 0x2712) and the service answers on the reply pipe
//! with one u32, its slot index for that connection. The subscription channel's index is
//! the subscription id the client then sends in its initialize request.
//!
//! Messages: a 12-byte header `"sesp", u32 size, u32 size ^ 0x70736573`, then a FlatBuffers
//! buffer of that size laid out the way flatcc builds it (tables front to back after the
//! root offset, vtables clustered at the end, fields in the order the builder adds them,
//! fields equal to their default left out). The root table holds a sequence number (field
//! 0), the payload type (field 1, u8) and the payload table (field 2). The `encode_*`
//! functions reproduce the original service builders byte for byte (tools/sesp_oracle.py).

use std::collections::HashMap;

use crate::error::{Result, ServiceError, sesp};
use crate::prp::{le_u32, push_u32};

pub const PIPE_NAME: &str = "streamengineservices";
pub const COMMAND_CHANNEL: u32 = 0x2711;
pub const SUBSCRIPTION_CHANNEL: u32 = 0x2712;
pub const CHANNELS: std::ops::Range<u32> = 0x2711..0x2715;

pub const MAGIC: u32 = 0x7073_6573;
pub const HEADER_SIZE: usize = 12;
pub const MAX_MESSAGE: usize = 1 << 20;

pub const SIGNAL_INVALID_OPERATION: u8 = 1;
pub const REQUEST_INITIALIZE: u8 = 2;
pub const RESPONSE_INITIALIZE: u8 = 3;
pub const REQUEST_HEADPOSE_START: u8 = 4;
pub const RESPONSE_HEADPOSE_START: u8 = 5;
pub const REQUEST_HEADPOSE_STOP: u8 = 6;
pub const RESPONSE_HEADPOSE_STOP: u8 = 7;
pub const SUBSCRIPTION_HEADPOSE: u8 = 8;
pub const REQUEST_GET_DISPLAY_INFO: u8 = 9;
pub const RESPONSE_GET_DISPLAY_INFO: u8 = 10;
pub const REQUEST_LIST_DEVICES: u8 = 0x10;
pub const REQUEST_NOTIFY_STREAM_STATUS: u8 = 0x1A;
pub const RESPONSE_NOTIFY_STREAM_STATUS: u8 = 0x1B;
pub const REQUEST_DEVICE_LIST_CHANGES_START: u8 = 0x21;
pub const REQUEST_SET_FW_UPGRADE_ALLOWED: u8 = 0x26;
pub const REQUEST_SEND_STATISTICS: u8 = 0x32;
pub const RESPONSE_SEND_STATISTICS: u8 = 0x33;

pub const ERROR_NONE: u32 = 0;
pub const ERROR_INTERNAL: u32 = 1;
pub const ERROR_NOT_SUPPORTED: u32 = 6;
pub const ERROR_NOT_FOUND: u32 = 7;
/// The service maps every internal error above 7 to 1 on the wire (ses_windows 0x1800c2d10).
pub const WIRE_ERRORS: std::ops::Range<u32> = 0..8;

// Initialize response fields 1..15 are capability flags. The client keeps eight of them
// (0x18005c130); the ones it acts on:
/// Head-pose start, platmod head-pose stream (+0x2a24).
pub const CAPABILITY_HEAD_POSE: u32 = 1;
/// Get display info, platmod display id (+0x2a25).
pub const CAPABILITY_DISPLAY_INFO: u32 = 2;
/// Notify stream status (+0x2a26); always set by the service.
pub const CAPABILITY_STREAM_STATUS: u32 = 3;
/// Device list changes (+0x2a29).
pub const CAPABILITY_DEVICE_LIST: u32 = 9;
pub const CAPABILITY_COUNT: u32 = 15;

pub const VALID: i32 = 1;
pub const INVALID: i32 = 0;

// Framing ----------------------------------------------------------------------------------

pub fn frame(buffer: &[u8]) -> Vec<u8> {
    let size = buffer.len() as u32;
    let mut message = Vec::with_capacity(HEADER_SIZE + buffer.len());
    push_u32(&mut message, MAGIC);
    push_u32(&mut message, size);
    push_u32(&mut message, size ^ MAGIC);
    message.extend_from_slice(buffer);
    message
}

/// Reassembles messages from pipe reads of any size, as sesp_parse_message does.
#[derive(Debug, Clone)]
pub struct FrameDecoder {
    pending: Vec<u8>,
    limit: usize,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new(MAX_MESSAGE)
    }
}

impl FrameDecoder {
    pub fn new(limit: usize) -> Self {
        Self { pending: Vec::new(), limit }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn pending(&self) -> &[u8] {
        &self.pending
    }

    /// Complete buffers in `data` and what came before it. Buffers before a malformed
    /// header are consumed with it, as in the Python decoder.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.pending.extend_from_slice(data);
        let mut buffers = Vec::new();
        while self.pending.len() >= HEADER_SIZE {
            let (magic, size, check) = (le_u32(&self.pending, 0), le_u32(&self.pending, 4), le_u32(&self.pending, 8));
            if magic != MAGIC || size ^ MAGIC != check {
                return sesp("not a SESP message header");
            }
            let size = size as usize;
            if size > self.limit {
                return sesp("SESP message exceeds the local limit");
            }
            if self.pending.len() < HEADER_SIZE + size {
                break;
            }
            buffers.push(self.pending[HEADER_SIZE..HEADER_SIZE + size].to_vec());
            self.pending.drain(..HEADER_SIZE + size);
        }
        Ok(buffers)
    }
}

// flatcc-shaped buffer building ----------------------------------------------------------

/// A scalar field value with its FlatBuffers width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    F32(f32),
    U64(u64),
    I64(i64),
    F64(f64),
}

impl Value {
    pub fn size(&self) -> usize {
        match self {
            Value::U8(_) | Value::I8(_) => 1,
            Value::U16(_) | Value::I16(_) => 2,
            Value::U32(_) | Value::I32(_) | Value::F32(_) => 4,
            Value::U64(_) | Value::I64(_) | Value::F64(_) => 8,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        match *self {
            Value::U8(v) => v.to_le_bytes().to_vec(),
            Value::I8(v) => v.to_le_bytes().to_vec(),
            Value::U16(v) => v.to_le_bytes().to_vec(),
            Value::I16(v) => v.to_le_bytes().to_vec(),
            Value::U32(v) => v.to_le_bytes().to_vec(),
            Value::I32(v) => v.to_le_bytes().to_vec(),
            Value::F32(v) => v.to_le_bytes().to_vec(),
            Value::U64(v) => v.to_le_bytes().to_vec(),
            Value::I64(v) => v.to_le_bytes().to_vec(),
            Value::F64(v) => v.to_le_bytes().to_vec(),
        }
    }
}

/// One table field in the order the original builder adds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field {
    Scalar {
        index: usize,
        value: Value,
    },
    /// Offset field pointing at an object emitted earlier (`position` from the builder).
    Reference {
        index: usize,
        position: i64,
    },
}

impl Field {
    pub fn index(&self) -> usize {
        match self {
            Field::Scalar { index, .. } | Field::Reference { index, .. } => *index,
        }
    }
}

/// Back-to-front FlatBuffers builder with flatcc's layout for small tables.
///
/// Objects are prepended to the front part; positions are kept relative to the boundary
/// between the front part and the vtable cluster, which grows in creation order and
/// shares identical vtables.
#[derive(Debug, Default, Clone)]
pub struct FlatccBuilder {
    front: Vec<Vec<u8>>,
    front_size: usize,
    vtables: Vec<Vec<u8>>,
    vtable_size: usize,
    vtable_positions: HashMap<Vec<u8>, usize>,
}

impl FlatccBuilder {
    fn prepend(&mut self, mut data: Vec<u8>) -> i64 {
        data.resize(data.len().next_multiple_of(4), 0);
        self.front_size += data.len();
        self.front.push(data);
        -(self.front_size as i64)
    }

    pub fn string(&mut self, text: &[u8]) -> i64 {
        let mut data = (text.len() as u32).to_le_bytes().to_vec();
        data.extend_from_slice(text);
        data.push(0);
        self.prepend(data)
    }

    fn vtable(&mut self, entries: Vec<u8>) -> usize {
        if let Some(position) = self.vtable_positions.get(&entries) {
            return *position;
        }
        let position = self.vtable_size;
        self.vtable_size += entries.len();
        self.vtable_positions.insert(entries.clone(), position);
        self.vtables.push(entries);
        position
    }

    /// Table of `fields` in the order the original builder adds them.
    pub fn table(&mut self, fields: &[Field]) -> i64 {
        let mut layout = vec![0u8; 4];
        let mut slots: HashMap<usize, usize> = HashMap::new();
        let mut references = Vec::new();
        for field in fields {
            let size = match field {
                Field::Reference { .. } => 4,
                Field::Scalar { value, .. } => value.size(),
            };
            layout.resize(layout.len().next_multiple_of(size), 0);
            slots.insert(field.index(), layout.len());
            match field {
                Field::Reference { position, .. } => {
                    references.push((layout.len(), *position));
                    layout.extend_from_slice(&[0; 4]);
                }
                Field::Scalar { value, .. } => layout.extend_from_slice(&value.bytes()),
            }
        }
        let count = slots.keys().max().map_or(0, |index| index + 1);
        let mut entries = Vec::with_capacity(4 + 2 * count);
        entries.extend_from_slice(&((4 + 2 * count) as u16).to_le_bytes());
        entries.extend_from_slice(&(layout.len() as u16).to_le_bytes());
        for index in 0..count {
            entries.extend_from_slice(&(slots.get(&index).copied().unwrap_or(0) as u16).to_le_bytes());
        }
        let vtable = self.vtable(entries) as i64;
        let position = -((self.front_size + layout.len().next_multiple_of(4)) as i64);
        layout[..4].copy_from_slice(&((position - vtable) as i32).to_le_bytes());
        for (offset, target) in references {
            let relative = (target - (position + offset as i64)) as u32;
            layout[offset..offset + 4].copy_from_slice(&relative.to_le_bytes());
        }
        self.prepend(layout)
    }

    pub fn finish(mut self, root: i64) -> Vec<u8> {
        let position = self.prepend(vec![0; 4]);
        let mut buffer: Vec<u8> = self.front.iter().rev().flatten().copied().collect();
        buffer[..4].copy_from_slice(&((root - position) as u32).to_le_bytes());
        buffer.extend(self.vtables.iter().flatten());
        buffer
    }
}

/// Fields that differ from their default, as flatcc leaves the others out.
pub fn scalars(values: &[(usize, Value, Value)]) -> Vec<Field> {
    values
        .iter()
        .filter(|(_, value, default)| value != default)
        .map(|(index, value, _)| Field::Scalar { index: *index, value: *value })
        .collect()
}

/// One framed message: payload table, then the root table {sequence, type, payload}.
///
/// `strings` are (field index, bytes) emitted before the payload table, which then refers
/// to them.
pub fn message(sequence: u32, kind: u8, payload_fields: &[Field], strings: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut builder = FlatccBuilder::default();
    let mut fields: Vec<Field> = strings
        .iter()
        .map(|(index, text)| Field::Reference { index: *index, position: builder.string(text) })
        .collect();
    fields.extend_from_slice(payload_fields);
    fields.sort_by_key(Field::index);
    let payload = builder.table(&fields);
    let mut root = scalars(&[(0, Value::U32(sequence), Value::U32(0))]);
    root.push(Field::Reference { index: 2, position: payload });
    root.push(Field::Scalar { index: 1, value: Value::U8(kind) });
    let root = builder.table(&root);
    frame(&builder.finish(root))
}

// Service messages (ses_windows.dll builders) ---------------------------------------------

pub fn wire_error(error: i64) -> u32 {
    match u32::try_from(error) {
        Ok(error) if WIRE_ERRORS.contains(&error) => error,
        _ => ERROR_INTERNAL,
    }
}

/// Type 3: error, then capability flags 1..15 (`capabilities` lists the set ones).
pub fn encode_initialize_response(sequence: u32, error: i64, capabilities: &[u32]) -> Vec<u8> {
    let mut values = vec![(0, Value::U32(wire_error(error)), Value::U32(0))];
    for index in 1..=CAPABILITY_COUNT {
        let flag = u8::from(capabilities.contains(&index));
        values.push((index as usize, Value::U8(flag), Value::U8(0)));
    }
    message(sequence, RESPONSE_INITIALIZE, &scalars(&values), &[])
}

/// Responses that carry only an error: head-pose start (5) and stop (7), send statistics
/// (0x33) and the invalid-operation signal (1).
pub fn encode_error_response(sequence: u32, kind: u8, error: i64) -> Vec<u8> {
    message(sequence, kind, &scalars(&[(0, Value::U32(wire_error(error)), Value::U32(0))]), &[])
}

/// Type 0x1b, an empty table.
pub fn encode_stream_status_response(sequence: u32) -> Vec<u8> {
    message(sequence, RESPONSE_NOTIFY_STREAM_STATUS, &[], &[])
}

/// tobii_head_pose_t as the service forwards it; validity 1 means valid.
///
/// Coordinates are float32 on the wire and kept as `f64` holding the caller's values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadPose {
    pub timestamp_us: i64,
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub position_validity: i32,
    pub rotation_validity: [i32; 3],
}

/// Type 8: timestamp high and low words, position, rotation, then the validities.
pub fn encode_head_pose(sequence: u32, pose: &HeadPose) -> Vec<u8> {
    let timestamp = pose.timestamp_us as u64;
    let float = |value: f64| Value::F32(value as f32);
    let validity = |value: i32| Value::I32(i32::from(value == VALID));
    let mut values = vec![
        (0, Value::U32((timestamp >> 32) as u32), Value::U32(0)),
        (1, Value::U32(timestamp as u32), Value::U32(0)),
    ];
    values.extend(pose.position.iter().enumerate().map(|(i, v)| (2 + i, float(*v), Value::F32(0.0))));
    values.extend(pose.rotation.iter().enumerate().map(|(i, v)| (5 + i, float(*v), Value::F32(0.0))));
    values.push((8, validity(pose.position_validity), Value::I32(1)));
    values.extend(pose.rotation_validity.iter().enumerate().map(|(i, v)| (9 + i, validity(*v), Value::I32(1))));
    message(sequence, SUBSCRIPTION_HEADPOSE, &scalars(&values), &[])
}

// Reading ----------------------------------------------------------------------------------

/// The scalar types a table field is read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    U64,
    I64,
    F64,
}

impl Kind {
    pub fn size(self) -> usize {
        match self {
            Kind::U8 | Kind::I8 => 1,
            Kind::U16 | Kind::I16 => 2,
            Kind::U32 | Kind::I32 | Kind::F32 => 4,
            Kind::U64 | Kind::I64 | Kind::F64 => 8,
        }
    }

    fn read(self, data: &[u8]) -> Value {
        let bytes = |n: usize| &data[..n];
        match self {
            Kind::U8 => Value::U8(data[0]),
            Kind::I8 => Value::I8(data[0] as i8),
            Kind::U16 => Value::U16(u16::from_le_bytes(bytes(2).try_into().expect("two bytes"))),
            Kind::I16 => Value::I16(i16::from_le_bytes(bytes(2).try_into().expect("two bytes"))),
            Kind::U32 => Value::U32(u32::from_le_bytes(bytes(4).try_into().expect("four bytes"))),
            Kind::I32 => Value::I32(i32::from_le_bytes(bytes(4).try_into().expect("four bytes"))),
            Kind::F32 => Value::F32(f32::from_le_bytes(bytes(4).try_into().expect("four bytes"))),
            Kind::U64 => Value::U64(u64::from_le_bytes(bytes(8).try_into().expect("eight bytes"))),
            Kind::I64 => Value::I64(i64::from_le_bytes(bytes(8).try_into().expect("eight bytes"))),
            Kind::F64 => Value::F64(f64::from_le_bytes(bytes(8).try_into().expect("eight bytes"))),
        }
    }
}

/// Field access on one FlatBuffers table, bounds-checked.
#[derive(Debug, Clone, Copy)]
pub struct Table<'a> {
    buffer: &'a [u8],
    position: usize,
    vtable: usize,
    vtable_size: usize,
    table_size: usize,
}

impl<'a> Table<'a> {
    pub fn new(buffer: &'a [u8], position: i64) -> Result<Self> {
        if position < 0 || position + 4 > buffer.len() as i64 {
            return sesp("table outside the buffer");
        }
        let position = position as usize;
        let vtable = position as i64 - i64::from(le_u32(buffer, position) as i32);
        if vtable < 0 || vtable + 4 > buffer.len() as i64 {
            return sesp("vtable outside the buffer");
        }
        let vtable = vtable as usize;
        let vtable_size = usize::from(u16::from_le_bytes([buffer[vtable], buffer[vtable + 1]]));
        let table_size = usize::from(u16::from_le_bytes([buffer[vtable + 2], buffer[vtable + 3]]));
        if vtable + vtable_size > buffer.len() || position + table_size > buffer.len() {
            return sesp("vtable or table runs past the buffer");
        }
        Ok(Self { buffer, position, vtable, vtable_size, table_size })
    }

    pub fn buffer(&self) -> &'a [u8] {
        self.buffer
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn vtable(&self) -> usize {
        self.vtable
    }

    pub fn vtable_size(&self) -> usize {
        self.vtable_size
    }

    pub fn table_size(&self) -> usize {
        self.table_size
    }

    fn slot(&self, index: usize) -> Option<usize> {
        let entry = 4 + 2 * index;
        if entry + 2 > self.vtable_size {
            return None;
        }
        let at = self.vtable + entry;
        let offset = usize::from(u16::from_le_bytes([self.buffer[at], self.buffer[at + 1]]));
        (offset != 0).then_some(self.position + offset)
    }

    /// The field's value, `None` where the table leaves it at its default.
    pub fn scalar(&self, index: usize, kind: Kind) -> Result<Option<Value>> {
        let Some(slot) = self.slot(index) else {
            return Ok(None);
        };
        if slot + kind.size() > self.buffer.len() {
            return sesp("field runs past the buffer");
        }
        Ok(Some(kind.read(&self.buffer[slot..])))
    }

    pub fn u8_or(&self, index: usize, default: u8) -> Result<u8> {
        Ok(match self.scalar(index, Kind::U8)? {
            Some(Value::U8(value)) => value,
            _ => default,
        })
    }

    pub fn u32_or(&self, index: usize, default: u32) -> Result<u32> {
        Ok(match self.scalar(index, Kind::U32)? {
            Some(Value::U32(value)) => value,
            _ => default,
        })
    }

    pub fn i32_or(&self, index: usize, default: i32) -> Result<i32> {
        Ok(match self.scalar(index, Kind::I32)? {
            Some(Value::I32(value)) => value,
            _ => default,
        })
    }

    pub fn f32_or(&self, index: usize, default: f64) -> Result<f64> {
        Ok(match self.scalar(index, Kind::F32)? {
            Some(Value::F32(value)) => f64::from(value),
            _ => default,
        })
    }

    /// Position of the object an offset field points at.
    pub fn target(&self, index: usize) -> Result<Option<i64>> {
        let Some(slot) = self.slot(index) else {
            return Ok(None);
        };
        Ok(Some(slot as i64 + i64::from(self.u32_or(index, 0)?)))
    }

    pub fn table(&self, index: usize) -> Result<Option<Table<'a>>> {
        self.target(index)?.map(|target| Table::new(self.buffer, target)).transpose()
    }

    pub fn string(&self, index: usize) -> Result<Option<&'a [u8]>> {
        let Some(target) = self.target(index)? else {
            return Ok(None);
        };
        let target = target as usize;
        if target + 4 > self.buffer.len() {
            return sesp("string outside the buffer");
        }
        let size = le_u32(self.buffer, target) as usize;
        if target + 4 + size > self.buffer.len() {
            return sesp("string runs past the buffer");
        }
        Ok(Some(&self.buffer[target + 4..target + 4 + size]))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Message<'a> {
    pub sequence: u32,
    pub kind: u8,
    pub payload: Option<Table<'a>>,
}

pub fn decode(buffer: &[u8]) -> Result<Message<'_>> {
    if buffer.len() < 4 {
        return sesp("buffer too short for a root offset");
    }
    let root = Table::new(buffer, i64::from(le_u32(buffer, 0)))?;
    Ok(Message { sequence: root.u32_or(0, 0)?, kind: root.u8_or(1, 0)?, payload: root.table(2)? })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializeRequest {
    pub device_url: String,
    pub subscription_id: u32,
}

/// Type 2: the client's device URL (field 0) and its subscription channel id (field 1).
pub fn initialize_request(kind: u8, payload: Option<&Table>) -> Result<InitializeRequest> {
    let Some(table) = payload.filter(|_| kind == REQUEST_INITIALIZE) else {
        return sesp("not an initialize request");
    };
    let url = table.string(0)?.unwrap_or_default();
    Ok(InitializeRequest {
        device_url: String::from_utf8_lossy(url).into_owned(),
        subscription_id: table.u32_or(1, 0)?,
    })
}

/// Type 8 as the client reads it (0x18008b3a0): absent validities default to valid.
pub fn head_pose(kind: u8, payload: Option<&Table>) -> Result<HeadPose> {
    let Some(table) = payload.filter(|_| kind == SUBSCRIPTION_HEADPOSE) else {
        return sesp("not a head-pose message");
    };
    let timestamp = (u64::from(table.u32_or(0, 0)?) << 32) | u64::from(table.u32_or(1, 0)?);
    let floats = |start: usize| -> Result<[f64; 3]> {
        Ok([table.f32_or(start, 0.0)?, table.f32_or(start + 1, 0.0)?, table.f32_or(start + 2, 0.0)?])
    };
    Ok(HeadPose {
        timestamp_us: timestamp as i64,
        position: floats(2)?,
        rotation: floats(5)?,
        position_validity: table.i32_or(8, 1)?,
        rotation_validity: [table.i32_or(9, 1)?, table.i32_or(10, 1)?, table.i32_or(11, 1)?],
    })
}

/// Error and set capability flags of an initialize response.
pub fn capabilities(kind: u8, payload: Option<&Table>) -> Result<(u32, Vec<u32>)> {
    let Some(table) = payload.filter(|_| kind == RESPONSE_INITIALIZE) else {
        return sesp("not an initialize response");
    };
    let mut flags = Vec::new();
    for index in 1..=CAPABILITY_COUNT {
        if table.u8_or(index as usize, 0)? != 0 {
            flags.push(index);
        }
    }
    Ok((table.u32_or(0, 0)?, flags))
}

// Transport handshake ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub channel: u32,
    pub reply_pipe: String,
}

fn malformed_hello<T>() -> Result<T> {
    sesp("malformed SESP hello")
}

/// First pipe message of a channel: u32 channel, reply pipe name, NUL (0x18008f470, flag 0).
pub fn decode_hello(data: &[u8]) -> Result<Hello> {
    if data.len() < 5 || data.last() != Some(&0) {
        return malformed_hello();
    }
    let channel = le_u32(data, 0);
    let name = &data[4..data.len() - 1];
    if !CHANNELS.contains(&channel) || name.is_empty() || name.contains(&0) || !name.is_ascii() {
        return malformed_hello();
    }
    Ok(Hello { channel, reply_pipe: String::from_utf8(name.to_vec()).expect("ASCII") })
}

pub fn encode_hello(channel: u32, reply_pipe: &str) -> Result<Vec<u8>> {
    if !reply_pipe.is_ascii() {
        return Err(ServiceError::Invalid("reply pipe name must be ASCII".into()));
    }
    let mut data = channel.to_le_bytes().to_vec();
    data.extend_from_slice(reply_pipe.as_bytes());
    data.push(0);
    Ok(data)
}

/// The service's answer to a hello: its slot index for the connection.
pub fn encode_slot(index: u32) -> Vec<u8> {
    index.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests;
