//! Typed TTP payload items and response decoding (docs/ttp-replies.md).
//!
//! Structures are not nested by length: a structure header item announces how many
//! sibling items follow it.

use crate::error::{Result, fail};
use crate::numeric::{FIXED16_ONE, FIXED42_ONE, fixed16, fixed42, round_f32, truncate_in_range};
use crate::ttp::{Field, Message, PAYLOAD_PREFIX, be_u32};

pub mod wire {
    pub const INT32: u8 = 0x01;
    pub const UINT32: u8 = 0x02;
    pub const FIXED16: u8 = 0x03;
    pub const FIXED42: u8 = 0x04;
    pub const STRUCT: u8 = 0x05;
    pub const INT64: u8 = 0x06;
    pub const UINT64: u8 = 0x07;
    pub const STRING: u8 = 0x14;
    pub const BLOB: u8 = 0x15;
    pub const UINT32_ARRAY: u8 = 0x17;
    pub const FIXED16_ARRAY: u8 = 0x18;
    pub const FIXED42_ARRAY: u8 = 0x19;
    pub const STRING_ARRAY: u8 = 0x1A;
}

pub const ARRAY_STRUCT_ID: u32 = 0x100;
pub const ARRAY_COUNT_MASK: u32 = 0xFFF;
pub const PAIR_FIXED42: u32 = 0x21F40;
pub const TRIPLE_FIXED42: u32 = 0x31F41;
pub const TRIPLE_FIXED16: u32 = 0x31F42;
pub const PAIR_FIXED16: u32 = 0x21F43;
pub const KIND_REQUEST: u32 = 0x51;
pub const KIND_RESPONSE: u32 = 0x52;
pub const PARSER_KINDS: [(u32, u32); 6] = [(0x51, 1), (0x52, 2), (0x53, 3), (0x4C, 4), (0x43, 5), (0x4E, 6)];
pub const UNKNOWN_STATUS: u32 = 2;

pub const PARSER_STATUS_NAMES: [&str; 22] = [
    "NO_ERROR",
    "NEED_MORE_DATA",
    "UNKNOWN",
    "BUFFER_TOO_SMALL",
    "MESSAGE_TOO_BIG",
    "PROTOCOL",
    "INVALID_PARAMETER",
    "PROTOCOL_LEGACY_BAD_FORMAT",
    "PROTOCOL_LEGACY_MISSING_UPGRADE",
    "PROTOCOL_UNKNOWN_OPERATION",
    "PROTOCOL_UNSUPPORTED_OPERATION",
    "PROTOCOL_OPERATION_FAILED",
    "PROTOCOL_INVALID_PAYLOAD",
    "PROTOCOL_UNKNOWN_ID",
    "PROTOCOL_UNAUTHORIZED",
    "PROTOCOL_EXTENSION_REQUIRED",
    "PROTOCOL_INTERNAL_ERROR",
    "PROTOCOL_STATE_ERROR",
    "PROTOCOL_INVALID_PARAMETER",
    "PROTOCOL_OPERATION_ABORTED",
    "PROTOCOL_NOT_ENABLED",
    "PROTOCOL_TOO_MANY_SUBSCRIBERS",
];

pub const DEVICE_ERROR_STATUS: [(u32, u32); 20] = [
    (0x2000_0402, 7),
    (0x2000_0407, 11),
    (0x2000_040A, 8),
    (0x2000_0500, 9),
    (0x2000_0501, 10),
    (0x2000_0502, 11),
    (0x2000_0503, 12),
    (0x2000_0504, 13),
    (0x2000_0505, 14),
    (0x2000_0506, 15),
    (0x2000_0507, 16),
    (0x2000_0508, 17),
    (0x2000_0509, 18),
    (0x2000_0510, 19),
    (0x2000_0511, 11),
    (0x2000_0512, 11),
    (0x2000_0513, 11),
    (0x2000_0514, 11),
    (0x2000_0515, 20),
    (0x2000_0516, 21),
];

/// Parsed item types of each reply schema, by command (the runtime's reply table).
pub const RESPONSE_FIELD_TYPES: [(u32, &[u8]); 48] = [
    (0x3E8, &[0x01]),
    (0x408, &[0x01]),
    (0x42F, &[0x01]),
    (0x442, &[0x0B]),
    (0x44C, &[0x0D]),
    (0x460, &[0x1C]),
    (0x4B0, &[0x0E]),
    (0x4BA, &[0x0F]),
    (0x51E, &[0x12]),
    (0x532, &[0x15]),
    (0x546, &[0x16]),
    (0x547, &[0x16]),
    (0x578, &[0x0A; 8]),
    (0x58C, &[0x07; 4]),
    (0x596, &[0x0A; 3]),
    (0x5AA, &[0x0A; 3]),
    (0x5B4, &[0x01]),
    (0x5C8, &[0x0A, 0x0A]),
    (0x5CD, &[0x01]),
    (0x5D2, &[0x15]),
    (0x5D3, &[0x01]),
    (0x5E6, &[0x0D]),
    (0x640, &[0x01, 0x01, 0x01]),
    (0x64A, &[0x05]),
    (0x65E, &[0x06]),
    (0x668, &[0x05]),
    (0x672, &[0x17]),
    (0x686, &[0x18]),
    (0x690, &[0x17]),
    (0x6A4, &[0x07]),
    (0x712, &[0x14]),
    (0x71C, &[0x14]),
    (0x76C, &[0x01, 0x01, 0x0D]),
    (0x780, &[0x01]),
    (0x7DA, &[0x07]),
    (0x7EE, &[0x08]),
    (0x83E, &[0x01, 0x05, 0x05, 0x0A, 0x0A]),
    (0x848, &[0x1B, 0x0C, 0x02]),
    (0xBF4, &[0x08]),
    (0xC08, &[0x07, 0x0D]),
    (0xC26, &[0x01]),
    (0xC62, &[0x01]),
    (0xC6C, &[0x01]),
    (0xC80, &[0x07]),
    (0xC8A, &[0x07]),
    (0xC94, &[0x01]),
    (0xD16, &[0x04, 0x01, 0x01, 0x01, 0x01, 0x1A, 0x01, 0x01, 0x01, 0x01, 0x19, 0x0D]),
    (0x1388, &[0x0D]),
];

/// Item count and identifier of a structure header (`count << 16 | id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructHeader {
    pub item_count: u32,
    pub struct_id: u32,
}

impl StructHeader {
    pub fn from_word(word: u32) -> Self {
        Self { item_count: word >> 16, struct_id: word & 0xFFFF }
    }

    pub fn word(&self) -> Result<u32> {
        let word = (u64::from(self.item_count) << 16) | u64::from(self.struct_id);
        u32::try_from(word).or_else(|_| fail("Expected an unsigned 32-bit integer"))
    }
}

/// A decoded item. Integers keep their signedness; fixed-point values are doubles.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    UInt(u64),
    Float(f64),
    Struct(StructHeader),
    Bytes(Vec<u8>),
    UIntArray(Vec<u32>),
    FloatArray(Vec<f64>),
    BytesArray(Vec<Vec<u8>>),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match *self {
            Value::Int(value) => Some(value),
            Value::UInt(value) => i64::try_from(value).ok(),
            _ => None,
        }
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Value::Int(_) | Value::UInt(_))
    }
}

pub fn scalar_length(tag: u8) -> Option<usize> {
    match tag {
        wire::INT32 | wire::UINT32 | wire::FIXED16 | wire::STRUCT => Some(4),
        wire::FIXED42 | wire::INT64 | wire::UINT64 => Some(8),
        _ => None,
    }
}

fn be_i32(data: &[u8], at: usize) -> i32 {
    be_u32(data, at) as i32
}

fn be_u64(data: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(data[at..at + 8].try_into().expect("eight bytes"))
}

fn be_i64(data: &[u8], at: usize) -> i64 {
    be_u64(data, at) as i64
}

fn scalar(field: &Field, length: usize) -> Result<Value> {
    if field.data.len() != length {
        return fail(format!("Wire type {:#x} needs {length} data bytes", field.tag));
    }
    let data = &field.data;
    Ok(match field.tag {
        wire::INT32 => Value::Int(i64::from(be_i32(data, 0))),
        wire::UINT32 => Value::UInt(u64::from(be_u32(data, 0))),
        wire::FIXED16 => Value::Float(fixed16(be_i32(data, 0))),
        wire::STRUCT => Value::Struct(StructHeader::from_word(be_u32(data, 0))),
        wire::FIXED42 => Value::Float(fixed42(be_i64(data, 0))),
        wire::INT64 => Value::Int(be_i64(data, 0)),
        _ => Value::UInt(be_u64(data, 0)),
    })
}

fn counted(field: &Field, item_size: usize) -> Result<(usize, &[u8])> {
    if field.data.len() < 4 {
        return fail("Counted field lacks its count");
    }
    let count = be_u32(&field.data, 0) as usize;
    if field.data.len() as u64 != 4 + count as u64 * item_size as u64 {
        return fail("Count does not match field length");
    }
    Ok((count, &field.data[4..]))
}

fn strings(field: &Field) -> Result<Vec<Vec<u8>>> {
    let data = &field.data;
    if data.len() < 4 {
        return fail("String array lacks its count");
    }
    let count = be_u32(data, 0);
    let mut items = Vec::new();
    let mut offset = 4;
    for _ in 0..count {
        if data.len() - offset < 4 {
            return fail("Truncated string length");
        }
        let size = be_u32(data, offset) as usize;
        offset += 4;
        if size > data.len() - offset {
            return fail("Truncated string data");
        }
        items.push(data[offset..offset + size].to_vec());
        offset += size;
    }
    if offset != data.len() {
        return fail("String array length does not match its items");
    }
    Ok(items)
}

/// Decode one item; unknown wire types are refused instead of guessing a layout.
///
/// The runtime's readers ignore the outer length of variable-size items and trust their
/// inner counts; here a disagreement is refused rather than resolved.
pub fn value(field: &Field) -> Result<Value> {
    if let Some(length) = scalar_length(field.tag) {
        return scalar(field, length);
    }
    match field.tag {
        wire::STRING | wire::BLOB => Ok(Value::Bytes(counted(field, 1)?.1.to_vec())),
        wire::UINT32_ARRAY => {
            let (count, data) = counted(field, 4)?;
            Ok(Value::UIntArray((0..count).map(|i| be_u32(data, i * 4)).collect()))
        }
        wire::FIXED16_ARRAY => {
            let (count, data) = counted(field, 4)?;
            Ok(Value::FloatArray((0..count).map(|i| fixed16(be_i32(data, i * 4))).collect()))
        }
        wire::FIXED42_ARRAY => {
            let (count, data) = counted(field, 8)?;
            Ok(Value::FloatArray((0..count).map(|i| fixed42(be_i64(data, i * 8))).collect()))
        }
        wire::STRING_ARRAY => Ok(Value::BytesArray(strings(field)?)),
        tag => fail(format!("Unknown wire type {tag:#x}")),
    }
}

/// Every item of a payload, decoded; the first undecodable item fails the whole reply.
pub fn values(payload: &[u8]) -> Result<Vec<Value>> {
    crate::ttp::fields(payload)?.iter().map(value).collect()
}

fn range_error<T>() -> Result<T> {
    fail("Value does not fit the fixed-point wire type")
}

pub fn fixed16_raw(number: f64) -> Result<i32> {
    match truncate_in_range(round_f32(number) * FIXED16_ONE, -2_147_483_648.0, 2_147_483_648.0) {
        Some(raw) => Ok(raw as i32),
        None => range_error(),
    }
}

pub fn fixed42_raw(number: f64) -> Result<i64> {
    match truncate_in_range(number * FIXED42_ONE, -9_223_372_036_854_775_808.0, 9_223_372_036_854_775_808.0) {
        Some(raw) => Ok(raw as i64),
        None => range_error(),
    }
}

pub fn int32_field(number: i32) -> Field {
    Field::new(wire::INT32, number.to_be_bytes().to_vec())
}

pub fn uint32_field(number: u32) -> Field {
    Field::new(wire::UINT32, number.to_be_bytes().to_vec())
}

pub fn int64_field(number: i64) -> Field {
    Field::new(wire::INT64, number.to_be_bytes().to_vec())
}

pub fn fixed16_field(number: f64) -> Result<Field> {
    Ok(Field::new(wire::FIXED16, fixed16_raw(number)?.to_be_bytes().to_vec()))
}

pub fn fixed42_field(number: f64) -> Result<Field> {
    Ok(Field::new(wire::FIXED42, fixed42_raw(number)?.to_be_bytes().to_vec()))
}

pub fn struct_field(item_count: u32, struct_id: u32) -> Result<Field> {
    let word = StructHeader { item_count, struct_id }.word()?;
    Ok(Field::new(wire::STRUCT, word.to_be_bytes().to_vec()))
}

/// Array header as built by 0x1403b1d70; the descriptor's meaning is unknown.
pub fn array_fields(count: u32, descriptor: u32) -> Result<[Field; 2]> {
    if count >= ARRAY_COUNT_MASK {
        return fail("Array count does not fit the header");
    }
    Ok([struct_field(count + 1, ARRAY_STRUCT_ID)?, uint32_field(descriptor)])
}

pub fn fixed42_array_field(numbers: &[f64]) -> Result<Field> {
    let mut data = Vec::with_capacity(4 + numbers.len() * 8);
    data.extend_from_slice(&(numbers.len() as u32).to_be_bytes());
    for &number in numbers {
        data.extend_from_slice(&fixed42_raw(number)?.to_be_bytes());
    }
    Ok(Field::new(wire::FIXED42_ARRAY, data))
}

pub fn triple_fixed42_fields(numbers: [f64; 3]) -> Result<[Field; 4]> {
    let [x, y, z] = numbers;
    Ok([struct_field(3, TRIPLE_FIXED42 & 0xFFFF)?, fixed42_field(x)?, fixed42_field(y)?, fixed42_field(z)?])
}

pub use crate::ttp::encode_payload;

/// Sequential typed reader over a response payload, in schema order.
///
/// Bytes after the last requested item are not inspected, as in the runtime.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(payload: &'a [u8]) -> Result<Self> {
        if payload.len() < 2 || payload[..2] != PAYLOAD_PREFIX {
            return fail("Unsupported payload prefix");
        }
        Ok(Self { payload, offset: 2 })
    }

    /// Resumes reading at an offset obtained from [`Reader::offset`].
    pub fn resume(payload: &'a [u8], offset: usize) -> Result<Self> {
        let mut reader = Self::new(payload)?;
        reader.offset = offset.clamp(2, payload.len());
        Ok(reader)
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn remaining(&self) -> usize {
        self.payload.len() - self.offset
    }

    pub fn item(&mut self, wire: u8) -> Result<Field> {
        if self.remaining() < 5 {
            return fail("Truncated item header");
        }
        let tag = self.payload[self.offset];
        let length = be_u32(self.payload, self.offset + 1) as usize;
        if tag != wire {
            return fail(format!("Expected wire type {wire:#x}, got {tag:#x}"));
        }
        if length > self.remaining() - 5 {
            return fail("Truncated item data");
        }
        let start = self.offset + 5;
        self.offset = start + length;
        Ok(Field::new(tag, self.payload[start..start + length].to_vec()))
    }

    fn integer(&mut self, wire: u8) -> Result<i64> {
        match value(&self.item(wire)?)? {
            Value::Int(n) => Ok(n),
            Value::UInt(n) => Ok(n as i64),
            _ => unreachable!("scalar integer wire type"),
        }
    }

    pub fn int32(&mut self) -> Result<i32> {
        Ok(self.integer(wire::INT32)? as i32)
    }

    pub fn uint32(&mut self) -> Result<u32> {
        Ok(self.integer(wire::UINT32)? as u32)
    }

    fn float(&mut self, wire: u8) -> Result<f64> {
        match value(&self.item(wire)?)? {
            Value::Float(n) => Ok(n),
            _ => unreachable!("scalar fixed-point wire type"),
        }
    }

    pub fn fixed16(&mut self) -> Result<f64> {
        self.float(wire::FIXED16)
    }

    pub fn fixed42(&mut self) -> Result<f64> {
        self.float(wire::FIXED42)
    }

    fn header(&mut self) -> Result<StructHeader> {
        match value(&self.item(wire::STRUCT)?)? {
            Value::Struct(header) => Ok(header),
            _ => unreachable!("structure wire type"),
        }
    }

    pub fn structure(&mut self, header: u32) -> Result<()> {
        let found = self.header()?.word()?;
        if found != header {
            return fail(format!("Expected structure header {header:#x}, got {found:#x}"));
        }
        Ok(())
    }

    /// Number of array elements that follow the array header.
    pub fn array(&mut self) -> Result<u32> {
        let header = self.header()?;
        let count = header.item_count & ARRAY_COUNT_MASK;
        if header.struct_id != ARRAY_STRUCT_ID || count == 0 {
            return fail("Expected an array header");
        }
        self.uint32()?;
        Ok(count - 1)
    }

    /// Keeps at most `limit` values; the runtime reads and drops the rest.
    pub fn fixed42_array(&mut self, limit: usize) -> Result<Vec<f64>> {
        match value(&self.item(wire::FIXED42_ARRAY)?)? {
            Value::FloatArray(mut numbers) => {
                numbers.truncate(limit);
                Ok(numbers)
            }
            _ => unreachable!("fixed42 array wire type"),
        }
    }

    pub fn triple_fixed42(&mut self) -> Result<[f64; 3]> {
        self.structure(TRIPLE_FIXED42)?;
        Ok([self.fixed42()?, self.fixed42()?, self.fixed42()?])
    }
}

/// Parser status for a response; payload items are only decoded when it is zero.
pub fn response_status(message: &Message) -> u32 {
    if message.word4 == 0 {
        return 0;
    }
    DEVICE_ERROR_STATUS.iter().find(|(code, _)| *code == message.word4).map_or(UNKNOWN_STATUS, |(_, status)| *status)
}

pub fn status_name(status: i64) -> String {
    usize::try_from(status)
        .ok()
        .and_then(|index| PARSER_STATUS_NAMES.get(index))
        .map_or_else(|| python_hex(status), |name| (*name).to_string())
}

fn python_hex(value: i64) -> String {
    if value < 0 { format!("-{:#x}", value.unsigned_abs()) } else { format!("{value:#x}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::hex;

    fn item(tag: u8, data: &str) -> Field {
        Field::new(tag, hex(data))
    }

    fn payload(items: &[Field]) -> Vec<u8> {
        encode_payload(items).unwrap()
    }

    #[test]
    fn scalars() {
        assert_eq!(value(&item(wire::INT32, "fffffffe")).unwrap(), Value::Int(-2));
        assert_eq!(value(&item(wire::UINT32, "fffffffe")).unwrap(), Value::UInt(0xFFFF_FFFE));
        assert_eq!(value(&item(wire::FIXED16, "00018000")).unwrap(), Value::Float(1.5));
        assert_eq!(value(&item(wire::FIXED16, "ffffc000")).unwrap(), Value::Float(-0.25));
        assert_eq!(value(&item(wire::FIXED42, "0000040000000000")).unwrap(), Value::Float(1.0));
        assert_eq!(value(&item(wire::INT64, "fffffffffffffffd")).unwrap(), Value::Int(-3));
        assert_eq!(value(&item(wire::UINT64, "fffffffffffffffd")).unwrap(), Value::UInt(u64::MAX - 2));
        assert_eq!(
            value(&item(wire::STRUCT, "001a332c")).unwrap(),
            Value::Struct(StructHeader { item_count: 0x1A, struct_id: 0x332C })
        );
        assert_eq!(value(&item(wire::FIXED16, "01000001")).unwrap(), Value::Float(256.0));
    }

    #[test]
    fn scalar_lengths_are_exact() {
        for (tag, size) in [(wire::INT32, 8), (wire::FIXED16, 2), (wire::FIXED42, 4), (wire::UINT64, 4)] {
            assert!(value(&Field::new(tag, vec![0; size])).is_err());
        }
        assert!(value(&Field::new(0x08, vec![0; 4])).is_err());
    }

    #[test]
    fn counted_values_and_extents() {
        assert_eq!(value(&item(wire::STRING, "00000003616263")).unwrap(), Value::Bytes(b"abc".to_vec()));
        assert_eq!(value(&item(wire::UINT32_ARRAY, "000000020000000100000002")).unwrap(), Value::UIntArray(vec![1, 2]));
        assert_eq!(value(&item(wire::FIXED16_ARRAY, "0000000100010000")).unwrap(), Value::FloatArray(vec![1.0]));
        assert_eq!(
            value(&item(wire::FIXED42_ARRAY, "00000001fffffc0000000000")).unwrap(),
            Value::FloatArray(vec![-1.0])
        );
        assert_eq!(
            value(&item(wire::STRING_ARRAY, "00000002000000016100000000")).unwrap(),
            Value::BytesArray(vec![b"a".to_vec(), Vec::new()])
        );
        for (tag, data) in [
            (wire::STRING, "0000000461"),
            (wire::FIXED42_ARRAY, "0000000200"),
            (wire::STRING_ARRAY, "000000010000000561"),
            (wire::STRING_ARRAY, "00000000ff"),
            (wire::BLOB, "000000"),
        ] {
            assert!(value(&item(tag, data)).is_err());
        }
    }

    #[test]
    fn encoders() {
        let encoded = |field: Field| field.encode().unwrap();
        assert_eq!(encoded(int32_field(-2)), hex("0100000004fffffffe"));
        assert_eq!(encoded(fixed16_field(1.5).unwrap()), hex("030000000400018000"));
        assert_eq!(encoded(fixed42_field(-1.0).unwrap()), hex("0400000008fffffc0000000000"));
        assert_eq!(encoded(struct_field(3, 0x1F41).unwrap()), hex("050000000400031f41"));
        assert_eq!(fixed16_raw(-1.000_000_1).unwrap(), -65536);
        assert_eq!(fixed42_raw(2.0f64.powi(-43)).unwrap(), 0);
        assert!(fixed16_field(32768.0).is_err());
        assert!(fixed42_field(2.0f64.powi(21)).is_err());
        let array: Vec<u8> = array_fields(2, 7).unwrap().iter().flat_map(|f| f.encode().unwrap()).collect();
        assert_eq!(array, hex("050000000400030100 020000000400000007"));
        assert!(array_fields(0xFFF, 0).is_err());
        assert_eq!(encoded(fixed42_array_field(&[0.5]).unwrap()), hex("190000000c000000010000020000000000"));
    }

    #[test]
    fn sequential_reads() {
        let mut items = array_fields(1, 0).unwrap().to_vec();
        items.extend(triple_fixed42_fields([1.0, -2.0, 0.5]).unwrap());
        items.extend([int32_field(9), uint32_field(1)]);
        let data = payload(&items);
        let mut reader = Reader::new(&data).unwrap();
        assert_eq!(reader.array().unwrap(), 1);
        assert_eq!(reader.triple_fixed42().unwrap(), [1.0, -2.0, 0.5]);
        assert_eq!(reader.int32().unwrap(), 9);
        assert_eq!(reader.remaining(), 9);
    }

    #[test]
    fn reader_rejections() {
        let data = payload(&[uint32_field(1)]);
        assert!(Reader::new(&data).unwrap().int32().is_err());
        let data = payload(&[struct_field(4, TRIPLE_FIXED42 & 0xFFFF).unwrap()]);
        assert!(Reader::new(&data).unwrap().triple_fixed42().is_err());
        let data = payload(&[struct_field(2, 0x101).unwrap(), uint32_field(0)]);
        assert!(Reader::new(&data).unwrap().array().is_err());
        let data = payload(&[struct_field(0, 0x100).unwrap(), uint32_field(0)]);
        assert!(Reader::new(&data).unwrap().array().is_err());
        let data = payload(&[struct_field(0xF003, 0x100).unwrap(), uint32_field(0)]);
        assert_eq!(Reader::new(&data).unwrap().array().unwrap(), 2);
        let data = payload(&[fixed42_array_field(&[1.0; 5]).unwrap(), int32_field(3)]);
        let mut reader = Reader::new(&data).unwrap();
        assert_eq!(reader.fixed42_array(2).unwrap(), [1.0, 1.0]);
        assert_eq!(reader.int32().unwrap(), 3);
        let one = payload(&[int32_field(1)]);
        assert!(Reader::new(&[&b"\0\x01"[..], &one[2..]].concat()).is_err());
        for cut in 3..one.len() {
            assert!(Reader::new(&one[..cut]).unwrap().int32().is_err());
        }
    }

    #[test]
    fn status_mapping() {
        let response = |word4| Message::new(0x52, 1, 0, 0x848, word4, Vec::new());
        assert_eq!(response_status(&response(0)), 0);
        assert_eq!(response_status(&response(0x2000_0503)), 12);
        assert_eq!(status_name(12), "PROTOCOL_INVALID_PAYLOAD");
        assert_eq!(response_status(&response(0x2000_050A)), 2);
        assert_eq!(status_name(2), "UNKNOWN");
        assert_eq!(status_name(99), "0x63");
    }
}
